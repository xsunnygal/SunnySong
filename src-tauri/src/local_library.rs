use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use lofty::{
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey},
};
use sha2::{Digest, Sha256};
use solmusic_application::{
    domain::{ArtistRef, Song, SongId},
    MusicDirectory, ScannedLocalTrack,
};
use tracing::{debug, warn};
use walkdir::WalkDir;

pub struct ScanBatch {
    pub tracks: Vec<ScannedLocalTrack>,
    pub skipped_files: usize,
    pub complete: bool,
    pub duration_ms: u64,
}

#[derive(Clone)]
pub struct LocalArtworkStore {
    root: PathBuf,
}

impl LocalArtworkStore {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("could not create local artwork cache: {error}"))?;
        Ok(Self { root })
    }

    fn cache(&self, bytes: &[u8]) -> Option<String> {
        let extension = image_extension(bytes)?;
        let digest = Sha256::digest(bytes);
        let path = self.root.join(format!("{:x}.{extension}", digest));
        if !path.exists() && std::fs::write(&path, bytes).is_err() {
            return None;
        }
        Some(format!("local-artwork:{}", path.to_string_lossy()))
    }
}

pub fn scan_directory(
    directory: &MusicDirectory,
    artwork: &LocalArtworkStore,
) -> Result<ScanBatch, String> {
    let started = Instant::now();
    tracing::info!(category = "SCANNER", event = "directory_scan_started", directory_id = directory.id, path = %directory.path);
    let root = PathBuf::from(&directory.path)
        .canonicalize()
        .map_err(|error| format!("could not access music folder {}: {error}", directory.path))?;
    if !root.is_dir() {
        return Err(format!(
            "music folder is not a directory: {}",
            root.display()
        ));
    }

    let first_seen_at_ms = current_time_ms();
    let mut tracks = Vec::new();
    let mut skipped_files = 0;
    for entry in WalkDir::new(&root).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                skipped_files += 1;
                warn!(category = "LOCAL_LIBRARY", event = "scan_entry_failed", reason = %error);
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(mime_type) = supported_mime_type(entry.path()) else {
            continue;
        };
        match scan_track(&root, entry.path(), mime_type, first_seen_at_ms, artwork) {
            Ok(track) => tracks.push(track),
            Err(error) => {
                skipped_files += 1;
                warn!(category = "LOCAL_LIBRARY", event = "track_scan_failed", path = %entry.path().display(), reason = %error);
            }
        }
    }
    let complete = skipped_files == 0;
    let duration_ms = started.elapsed().as_millis() as u64;
    tracing::info!(
        category = "SCANNER",
        event = if complete {
            "directory_scan_completed"
        } else {
            "directory_scan_partial"
        },
        directory_id = directory.id,
        indexed_tracks = tracks.len(),
        skipped_files,
        duration_ms
    );
    Ok(ScanBatch {
        tracks,
        skipped_files,
        complete,
        duration_ms,
    })
}

pub fn read_embedded_lyrics(path: &Path) -> Result<Option<String>, String> {
    let tagged = Probe::open(path)
        .and_then(|probe| probe.read())
        .map_err(|error| format!("could not read audio metadata: {error}"))?;
    let value = tagged
        .primary_tag()
        .and_then(|tag| tag.get_string(&ItemKey::Lyrics))
        .or_else(|| {
            tagged
                .tags()
                .iter()
                .find_map(|tag| tag.get_string(&ItemKey::Lyrics))
        });
    Ok(value.and_then(normalize_lyrics))
}

fn normalize_lyrics(value: &str) -> Option<String> {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn scan_track(
    root: &Path,
    path: &Path,
    mime_type: &'static str,
    first_seen_at_ms: i64,
    artwork: &LocalArtworkStore,
) -> Result<ScannedLocalTrack, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("could not canonicalize file: {error}"))?;
    if !canonical.starts_with(root) {
        return Err("resolved file escaped the configured music folder".into());
    }
    let canonical_path = canonical
        .to_str()
        .ok_or("non-UTF-8 music paths are not supported")?
        .to_owned();
    let relative_path = canonical
        .strip_prefix(root)
        .map_err(|_| "could not create relative music path")?
        .to_str()
        .ok_or("non-UTF-8 music paths are not supported")?
        .to_owned();
    let metadata = canonical
        .metadata()
        .map_err(|error| format!("could not read file metadata: {error}"))?;
    let file_size_bytes = metadata.len();
    let source_identity = filesystem_source_identity(&metadata);
    let modified_at_ms = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0);
    let song_id = SongId::new(format!(
        "local:{}",
        content_identity(&canonical, file_size_bytes)?
    ))
    .expect("generated local song ID is nonblank");

    let tagged = Probe::open(&canonical)
        .and_then(|probe| probe.read())
        .map_err(|error| format!("could not read audio metadata: {error}"));
    let fallback_title = canonical
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Unknown Track")
        .to_owned();

    let (title, track_artist, album_artist, album_name, duration_ms, embedded_artwork) =
        match tagged {
            Ok(tagged) => {
                let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
                let title = tag
                    .and_then(|tag| tag.title())
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty())
                    .unwrap_or(fallback_title);
                let track_artist = tag
                    .and_then(|tag| tag.artist())
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty());
                let album_artist = tag
                    .and_then(|tag| tag.get_string(&ItemKey::AlbumArtist))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                let album_name = tag
                    .and_then(|tag| tag.album())
                    .map(|value| value.trim().to_owned())
                    .filter(|value| !value.is_empty());
                let duration = tagged.properties().duration().as_millis();
                let duration_ms = (duration > 0).then_some(duration.min(u64::MAX as u128) as u64);
                let embedded_artwork = tag
                    .and_then(|tag| tag.pictures().first())
                    .and_then(|picture| artwork.cache(picture.data()));
                (
                    title,
                    track_artist,
                    album_artist,
                    album_name,
                    duration_ms,
                    embedded_artwork,
                )
            }
            Err(error) => {
                debug!(category = "LOCAL_LIBRARY", event = "metadata_fallback_used", path = %canonical.display(), reason = %error);
                (fallback_title, None, None, None, None, None)
            }
        };

    let display_artist = track_artist
        .as_deref()
        .or(album_artist.as_deref())
        .unwrap_or("Unknown Artist")
        .to_owned();
    let mut artist_names = Vec::new();
    if let Some(value) = album_artist.as_deref() {
        append_artist_credits(&mut artist_names, value);
    }
    if let Some(value) = track_artist.as_deref() {
        append_artist_credits(&mut artist_names, value);
    }
    if artist_names.is_empty() {
        artist_names.push("Unknown Artist".into());
    }

    let album_id = album_name.as_ref().map(|album| {
        let mut digest = Sha256::new();
        digest.update(album.to_lowercase().as_bytes());
        digest.update([0]);
        digest.update(
            album_artist
                .as_deref()
                .unwrap_or(&display_artist)
                .to_lowercase()
                .as_bytes(),
        );
        format!("local-album:{:x}", digest.finalize())
    });

    let thumbnail_url = embedded_artwork.or_else(|| folder_artwork(&canonical, artwork));

    Ok(ScannedLocalTrack {
        song: Song {
            id: song_id,
            title,
            artist: ArtistRef {
                id: None,
                name: display_artist,
            },
            album_id,
            album_name,
            duration_ms,
            thumbnail_url,
        },
        canonical_path,
        relative_path,
        mime_type: mime_type.into(),
        file_size_bytes,
        modified_at_ms,
        source_identity,
        artist_names,
        first_seen_at_ms,
    })
}

fn folder_artwork(audio_path: &Path, artwork: &LocalArtworkStore) -> Option<String> {
    let parent = audio_path.parent()?;
    const STEMS: &[&str] = &["cover", "folder", "front", "album"];
    const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
    let entries = std::fs::read_dir(parent).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        let stem = stem.to_ascii_lowercase();
        let extension = extension.to_ascii_lowercase();
        if !STEMS.contains(&stem.as_str()) || !EXTENSIONS.contains(&extension.as_str()) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(path) {
            if let Some(cached) = artwork.cache(&bytes) {
                return Some(cached);
            }
        }
    }
    None
}

fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("jpg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn append_artist_credits(target: &mut Vec<String>, value: &str) {
    let normalized = value
        .replace(" featuring ", ";")
        .replace(" feat. ", ";")
        .replace(" feat ", ";")
        .replace(" & ", ";");
    let mut seen = target
        .iter()
        .map(|artist| artist.to_lowercase())
        .collect::<HashSet<_>>();
    for artist in normalized
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if seen.insert(artist.to_lowercase()) {
            target.push(artist.to_owned());
        }
    }
}

fn content_identity(path: &Path, length: u64) -> Result<String, String> {
    const SAMPLE_SIZE: usize = 64 * 1024;
    let mut file =
        File::open(path).map_err(|error| format!("could not fingerprint file: {error}"))?;
    let mut digest = Sha256::new();
    digest.update(length.to_le_bytes());
    let mut first = vec![0; SAMPLE_SIZE.min(length as usize)];
    file.read_exact(&mut first)
        .map_err(|error| format!("could not read fingerprint prefix: {error}"))?;
    digest.update(first);
    if length > SAMPLE_SIZE as u64 {
        let sample = SAMPLE_SIZE.min(length as usize);
        file.seek(SeekFrom::End(-(sample as i64)))
            .map_err(|error| format!("could not seek fingerprint suffix: {error}"))?;
        let mut last = vec![0; sample];
        file.read_exact(&mut last)
            .map_err(|error| format!("could not read fingerprint suffix: {error}"))?;
        digest.update(last);
    }
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(unix)]
fn filesystem_source_identity(metadata: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn filesystem_source_identity(_metadata: &std::fs::Metadata) -> Option<String> {
    None
}

fn supported_mime_type(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "mp3" => Some("audio/mpeg"),
        "flac" => Some("audio/flac"),
        "m4a" | "aac" => Some("audio/mp4"),
        "ogg" | "opus" => Some("audio/ogg"),
        "wav" => Some("audio/wav"),
        _ => None,
    }
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        append_artist_credits, image_extension, normalize_lyrics, scan_directory,
        supported_mime_type, LocalArtworkStore,
    };
    use solmusic_application::MusicDirectory;
    use std::{fs, path::Path};

    #[test]
    fn normalizes_embedded_lyrics_line_endings_and_whitespace() {
        assert_eq!(
            normalize_lyrics("  Line one\r\nLine two\r\n  ").as_deref(),
            Some("Line one\nLine two")
        );
        assert_eq!(normalize_lyrics(" \r\n\t"), None);
    }

    #[test]
    fn recognizes_initial_local_formats() {
        assert_eq!(
            supported_mime_type(Path::new("track.MP3")),
            Some("audio/mpeg")
        );
        assert_eq!(
            supported_mime_type(Path::new("track.flac")),
            Some("audio/flac")
        );
        assert_eq!(supported_mime_type(Path::new("cover.jpg")), None);
    }

    #[test]
    fn artist_credits_are_deduplicated() {
        let mut artists = vec!["Justice".into()];
        append_artist_credits(&mut artists, "Justice feat. Tame Impala");
        assert_eq!(artists, vec!["Justice", "Tame Impala"]);
    }

    #[test]
    fn scans_untagged_wav_as_unknown_artist() {
        let root = std::env::temp_dir().join(format!("solmusic-scan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("Local Track.wav");
        let data_length = 8_000u32;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_length).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&8_000u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&8u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_length.to_le_bytes());
        wav.resize(44 + data_length as usize, 128);
        fs::write(&path, wav).unwrap();

        fs::write(root.join("Cover.PNG"), b"\x89PNG\r\n\x1a\ncover").unwrap();
        let artwork_root = root.with_extension("artwork-cache");
        let _ = fs::remove_dir_all(&artwork_root);
        let artwork = LocalArtworkStore::new(artwork_root.clone()).unwrap();
        let batch = scan_directory(
            &MusicDirectory {
                id: 1,
                path: root.to_string_lossy().into_owned(),
                added_at_ms: 1,
                last_scanned_at_ms: None,
                last_scan_attempt_at_ms: None,
                status: "READY".into(),
                last_error: None,
                track_count: 0,
            },
            &artwork,
        )
        .unwrap();
        assert_eq!(batch.tracks.len(), 1);
        assert!(batch.complete);
        #[cfg(unix)]
        assert!(batch.tracks[0]
            .source_identity
            .as_deref()
            .is_some_and(|value| value.starts_with("unix:")));
        assert_eq!(batch.tracks[0].song.title, "Local Track");
        assert_eq!(batch.tracks[0].artist_names, vec!["Unknown Artist"]);
        assert!(batch.tracks[0].song.id.as_str().starts_with("local:"));
        assert!(batch.tracks[0]
            .song
            .thumbnail_url
            .as_deref()
            .is_some_and(|url| url.starts_with("local-artwork:")));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(artwork_root).unwrap();
    }

    #[test]
    fn recognizes_supported_artwork_formats() {
        assert_eq!(image_extension(&[0xff, 0xd8, 0xff, 0xe0]), Some("jpg"));
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\nrest"), Some("png"));
        assert_eq!(image_extension(b"RIFF1234WEBPrest"), Some("webp"));
        assert_eq!(image_extension(b"not an image"), None);
    }
}
