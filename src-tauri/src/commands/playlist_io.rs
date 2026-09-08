use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use solmusic_application::{domain::SongId, PlaylistItemSource, SunnySongApp};
use tauri::State;
use uuid::Uuid;

const MAX_M3U_BYTES: u64 = 10 * 1024 * 1024;
const MAX_M3U_ENTRIES: usize = 10_000;
const MAX_M3U_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistExportStatus {
    pub path: String,
    pub exported: usize,
    pub portable: usize,
    pub preserved_as_metadata: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistImportReason {
    pub reason: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistImportStatus {
    pub playlist_id: String,
    pub playlist_name: String,
    pub imported: usize,
    pub skipped: usize,
    pub reasons: Vec<PlaylistImportReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedEntry {
    location: String,
    sunny_source: Option<String>,
    sunny_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImportReference {
    YouTube(String),
    Local(PathBuf),
    Unsupported(&'static str),
}

#[tauri::command]
pub fn export_playlist_m3u(
    app: State<'_, SunnySongApp>,
    playlist_id: String,
    path: String,
) -> Result<PlaylistExportStatus, String> {
    ensure_desktop()?;
    let destination = validate_m3u_path(&path, false)?;
    let playlist = app
        .playlists()
        .map_err(error)?
        .into_iter()
        .find(|playlist| playlist.id == playlist_id)
        .ok_or("Playlist was not found")?;
    let tracks = app.playlist_export_tracks(&playlist_id).map_err(error)?;

    let mut output = String::from("#EXTM3U\n");
    output.push_str(&format!("#PLAYLIST:{}\n", safe_metadata(&playlist.name)));
    let mut portable = 0;
    let mut preserved_as_metadata = 0;
    for track in &tracks {
        let seconds = track
            .song
            .duration_ms
            .map(|duration| duration.div_ceil(1_000))
            .and_then(|duration| i64::try_from(duration).ok())
            .unwrap_or(-1);
        output.push_str(&format!(
            "#EXTINF:{seconds},{} - {}\n",
            safe_metadata(&track.song.artist.name),
            safe_metadata(&track.song.title)
        ));
        match &track.source {
            PlaylistItemSource::Local { path, available } => {
                let location = path
                    .to_str()
                    .filter(|value| !value.contains(['\r', '\n']))
                    .ok_or("A local track path cannot be represented safely in M3U")?;
                if !available {
                    output.push_str(&format!(
                        "#SUNNYSONG:SOURCE=LOCAL;AVAILABLE=0;ID={};PATH={}\n",
                        safe_metadata(track.song.id.as_str()),
                        safe_metadata(location)
                    ));
                    preserved_as_metadata += 1;
                } else {
                    portable += 1;
                }
                output.push_str(location);
                output.push('\n');
            }
            PlaylistItemSource::YouTube => {
                portable += 1;
                output.push_str(&youtube_url(track.song.id.as_str())?);
                output.push('\n');
            }
            PlaylistItemSource::Jellyfin => {
                preserved_as_metadata += 1;
                output.push_str(&format!(
                    "#SUNNYSONG:SOURCE=JELLYFIN;AVAILABLE=0;ID={}\n",
                    safe_metadata(track.song.id.as_str())
                ));
                output.push_str(&format!("sunnysong:track:{}\n", track.song.id.as_str()));
            }
            PlaylistItemSource::Unavailable => {
                preserved_as_metadata += 1;
                output.push_str(&format!(
                    "#SUNNYSONG:SOURCE=UNAVAILABLE;AVAILABLE=0;ID={}\n",
                    safe_metadata(track.song.id.as_str())
                ));
                output.push_str(&format!("sunnysong:track:{}\n", track.song.id.as_str()));
            }
        }
    }

    atomic_write(&destination, output.as_bytes())?;
    Ok(PlaylistExportStatus {
        path: display_path(&destination)?,
        exported: tracks.len(),
        portable,
        preserved_as_metadata,
    })
}

#[tauri::command]
pub fn import_playlist_m3u(
    app: State<'_, SunnySongApp>,
    path: String,
    name: String,
) -> Result<PlaylistImportStatus, String> {
    ensure_desktop()?;
    let source = validate_m3u_path(&path, true)?;
    let input = read_m3u(&source)?;
    let entries = parse_m3u(&input)?;
    let base = source.parent().unwrap_or_else(|| Path::new("."));
    let mut reasons = HashMap::<String, usize>::new();
    let mut ids = Vec::new();
    let mut seen = HashSet::new();

    for entry in entries {
        let resolved = match classify_entry(&entry, base) {
            Ok(ImportReference::YouTube(id)) => {
                let id = SongId::new(id).ok_or("Parsed an empty YouTube ID")?;
                app.song_by_id(&id).map_err(error)?
            }
            Ok(ImportReference::Local(path)) => app.song_by_local_path(&path).map_err(error)?,
            Ok(ImportReference::Unsupported(reason)) => {
                add_reason(&mut reasons, reason);
                continue;
            }
            Err(reason) => {
                add_reason(&mut reasons, &reason);
                continue;
            }
        };
        let Some(song) = resolved else {
            add_reason(&mut reasons, "Not found in the indexed library");
            continue;
        };
        if !seen.insert(song.id.as_str().to_owned()) {
            add_reason(&mut reasons, "Duplicate track");
            continue;
        }
        ids.push(song.id);
    }

    let skipped = reasons.values().sum();
    let playlist = app
        .create_playlist_from_songs(&name, &ids, current_time_ms()?)
        .map_err(error)?;
    let mut reasons = reasons
        .into_iter()
        .map(|(reason, count)| PlaylistImportReason { reason, count })
        .collect::<Vec<_>>();
    reasons.sort_by(|left, right| left.reason.cmp(&right.reason));
    Ok(PlaylistImportStatus {
        playlist_id: playlist.id,
        playlist_name: playlist.name,
        imported: ids.len(),
        skipped,
        reasons,
    })
}

fn read_m3u(path: &Path) -> Result<String, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|cause| format!("Could not inspect playlist file: {cause}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Playlist input must be a regular file, not a link or folder".into());
    }
    if metadata.len() > MAX_M3U_BYTES {
        return Err("Playlist is too large (maximum 10 MiB)".into());
    }
    let bytes = fs::read(path).map_err(|cause| format!("Could not read playlist: {cause}"))?;
    String::from_utf8(bytes).map_err(|_| "Playlist must be valid UTF-8 (M3U8)".into())
}

fn parse_m3u(input: &str) -> Result<Vec<ParsedEntry>, String> {
    if input.contains('\0') {
        return Err("Playlist contains unsafe NUL bytes".into());
    }
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    if let Some((index, _)) = input
        .lines()
        .enumerate()
        .find(|(_, line)| line.len() > MAX_M3U_LINE_BYTES)
    {
        return Err(format!("Playlist line {} is too long", index + 1));
    }
    let mut lines = input.lines().enumerate();
    let first = lines
        .by_ref()
        .find(|(_, line)| !line.trim().is_empty())
        .ok_or("Playlist is empty")?;
    if first.1.trim() != "#EXTM3U" {
        return Err("Malformed playlist: the first non-empty line must be #EXTM3U".into());
    }

    let mut entries = Vec::new();
    let mut sunny_source = None;
    let mut sunny_id = None;
    let mut extinf_line = None;
    for (index, raw_line) in lines {
        let line = raw_line.trim().trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.strip_prefix("#EXTINF:") {
            if !value.contains(',') || extinf_line.is_some() {
                return Err(format!("Malformed #EXTINF on line {}", index + 1));
            }
            extinf_line = Some(index + 1);
            continue;
        }
        if let Some(value) = line.strip_prefix("#SUNNYSONG:") {
            sunny_source = metadata_value(value, "SOURCE");
            sunny_id = metadata_value(value, "ID");
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if entries.len() >= MAX_M3U_ENTRIES {
            return Err(format!("Playlist has more than {MAX_M3U_ENTRIES} entries"));
        }
        entries.push(ParsedEntry {
            location: line.to_owned(),
            sunny_source: sunny_source.take(),
            sunny_id: sunny_id.take(),
        });
        extinf_line = None;
    }
    if let Some(line) = extinf_line {
        return Err(format!(
            "Malformed playlist: #EXTINF on line {line} has no entry"
        ));
    }
    Ok(entries)
}

fn metadata_value(input: &str, key: &str) -> Option<String> {
    input.split(';').find_map(|field| {
        let (field_key, value) = field.split_once('=')?;
        field_key
            .eq_ignore_ascii_case(key)
            .then(|| value.to_owned())
    })
}

fn classify_entry(entry: &ParsedEntry, base: &Path) -> Result<ImportReference, String> {
    if entry
        .sunny_source
        .as_deref()
        .is_some_and(|source| source.eq_ignore_ascii_case("JELLYFIN"))
    {
        return Ok(ImportReference::Unsupported(
            "Jellyfin entries are metadata-only and cannot be imported portably",
        ));
    }
    if let (Some(source), Some(id)) = (&entry.sunny_source, &entry.sunny_id) {
        if source.eq_ignore_ascii_case("YOUTUBE") && is_youtube_video_id(id) {
            return Ok(ImportReference::YouTube(id.clone()));
        }
    }

    let value = entry.location.trim();
    if value.starts_with("sunnysong:") {
        return Ok(ImportReference::Unsupported(
            "Unavailable SunnySong metadata entry",
        ));
    }
    if let Ok(url) = reqwest::Url::parse(value) {
        return classify_url(url);
    }
    if is_youtube_video_id(value) && !value.contains(['/', '\\', '.']) {
        return Ok(ImportReference::YouTube(value.to_owned()));
    }

    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    path.canonicalize()
        .map(ImportReference::Local)
        .map_err(|_| "Local file does not exist or cannot be resolved".into())
}

fn classify_url(url: reqwest::Url) -> Result<ImportReference, String> {
    if url.scheme() == "file" {
        return url
            .to_file_path()
            .map_err(|_| String::from("Invalid local file URL"))?
            .canonicalize()
            .map(ImportReference::Local)
            .map_err(|_| "Local file does not exist or cannot be resolved".into());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let id = if host == "youtu.be" {
        url.path_segments()
            .and_then(|mut parts| parts.next())
            .map(str::to_owned)
    } else if matches!(
        host.as_str(),
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "music.youtube.com"
    ) && url.path() == "/watch"
    {
        url.query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, value)| value.into_owned())
    } else {
        None
    };
    match id.filter(|id| is_youtube_video_id(id)) {
        Some(id) => Ok(ImportReference::YouTube(id)),
        None if host.ends_with("youtube.com") || host == "youtu.be" => {
            Err("Malformed YouTube playlist entry".into())
        }
        None => Ok(ImportReference::Unsupported("Unsupported remote URL")),
    }
}

fn validate_m3u_path(value: &str, must_exist: bool) -> Result<PathBuf, String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err("Choose an M3U or M3U8 file".into());
    }
    let path = PathBuf::from(value);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !extension.eq_ignore_ascii_case("m3u") && !extension.eq_ignore_ascii_case("m3u8") {
        return Err("Playlist file must use the .m3u or .m3u8 extension".into());
    }
    if must_exist && !path.exists() {
        return Err("Playlist file does not exist".into());
    }
    if !must_exist
        && !path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .is_some_and(Path::is_dir)
    {
        return Err("Playlist destination folder does not exist".into());
    }
    if !must_exist && path.exists() && !path.is_file() {
        return Err("Playlist destination must be a regular file".into());
    }
    Ok(path)
}

fn youtube_url(id: &str) -> Result<String, String> {
    if !is_youtube_video_id(id) {
        return Err(format!("Cannot export invalid YouTube track ID {id}"));
    }
    let mut url = reqwest::Url::parse("https://music.youtube.com/watch").expect("static URL");
    url.query_pairs_mut().append_pair("v", id);
    Ok(url.into())
}

fn is_youtube_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn safe_metadata(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

fn atomic_write(destination: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or("Playlist destination must have a parent folder")?;
    let temporary = parent.join(format!(".sunnysong-playlist-{}.tmp", Uuid::new_v4()));
    let previous = parent.join(format!(".sunnysong-playlist-{}.old", Uuid::new_v4()));
    let result = (|| {
        let mut file = File::create_new(&temporary)
            .map_err(|cause| format!("Could not create playlist: {cause}"))?;
        file.write_all(bytes)
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|cause| format!("Could not write playlist: {cause}"))?;
        let replacing = destination.exists();
        if replacing {
            fs::rename(destination, &previous)
                .map_err(|cause| format!("Could not preserve existing playlist: {cause}"))?;
        }
        if let Err(cause) = fs::rename(&temporary, destination) {
            if replacing {
                let _ = fs::rename(&previous, destination);
            }
            return Err(format!("Could not finalize playlist: {cause}"));
        }
        if replacing {
            let _ = fs::remove_file(&previous);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn add_reason(reasons: &mut HashMap<String, usize>, reason: &str) {
    *reasons.entry(reason.to_owned()).or_default() += 1;
}

fn current_time_ms() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)
        .map(|duration| duration.as_millis() as i64)
}

fn display_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| "Selected path is not valid UTF-8".into())
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn ensure_desktop() -> Result<(), String> {
    if cfg!(desktop) {
        Ok(())
    } else {
        Err("M3U filesystem import and export are currently supported on desktop only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_entry, parse_m3u, ImportReference, ParsedEntry};
    use std::fs;

    #[test]
    fn parses_extended_m3u_and_sunnysong_metadata() {
        let parsed = parse_m3u(
            "\u{feff}#EXTM3U\n#EXTINF:123,Artist - Song\nhttps://music.youtube.com/watch?v=dQw4w9WgXcQ\n#SUNNYSONG:SOURCE=JELLYFIN;ID=jellyfin:1:2\nsunnysong:track:jellyfin:1:2\n",
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1].sunny_source.as_deref(), Some("JELLYFIN"));
    }

    #[test]
    fn rejects_missing_header_and_malformed_extinf() {
        assert!(parse_m3u("song.mp3\n").unwrap_err().contains("#EXTM3U"));
        assert!(parse_m3u("#EXTM3U\n#EXTINF:123\nsong.mp3\n")
            .unwrap_err()
            .contains("#EXTINF"));
    }

    #[test]
    fn resolves_youtube_variants_to_video_ids() {
        for location in [
            "dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=1",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=x",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
        ] {
            let entry = ParsedEntry {
                location: location.into(),
                sunny_source: None,
                sunny_id: None,
            };
            assert_eq!(
                classify_entry(&entry, std::path::Path::new(".")).unwrap(),
                ImportReference::YouTube("dQw4w9WgXcQ".into())
            );
        }
    }

    #[test]
    fn resolves_relative_local_paths_against_playlist_folder() {
        let root =
            std::env::temp_dir().join(format!("sunnysong-m3u-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("music")).unwrap();
        fs::write(root.join("music/song.mp3"), b"audio").unwrap();
        let entry = ParsedEntry {
            location: "music/song.mp3".into(),
            sunny_source: None,
            sunny_id: None,
        };
        let resolved = classify_entry(&entry, &root).unwrap();
        assert_eq!(
            resolved,
            ImportReference::Local(root.join("music/song.mp3").canonicalize().unwrap())
        );
        let _ = fs::remove_dir_all(root);
    }
}
