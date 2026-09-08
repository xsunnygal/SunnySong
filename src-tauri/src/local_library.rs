use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use lofty::{
    file::{AudioFile, TaggedFileExt},
    probe::Probe,
    tag::{Accessor, ItemKey, Tag},
};
use sha2::{Digest, Sha256};
use solmusic_application::{
    domain::{ArtistRef, Song, SongId},
    Lyrics, MusicDirectory, NormalizationGainMetadata, ScannedLocalTrack, TimedLyricsLine,
};
use tracing::{debug, warn};
use walkdir::WalkDir;

pub struct ScanBatch {
    pub tracks: Vec<ScannedLocalTrack>,
    pub skipped_files: usize,
    pub complete: bool,
    pub duration_ms: u64,
}

struct FileObservation {
    canonical: PathBuf,
    canonical_path: String,
    relative_path: String,
    mime_type: &'static str,
    file_size_bytes: u64,
    modified_at_ms: i64,
    modified_at_ns: Option<i64>,
    source_identity: Option<String>,
    sidecar_artwork: Option<SidecarArtwork>,
}

#[derive(Clone, PartialEq, Eq)]
struct SidecarArtwork {
    path: PathBuf,
    canonical_path: String,
    file_size_bytes: u64,
    modified_at_ns: Option<i64>,
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

    pub fn prune(&self, referenced: &[String]) -> Result<usize, String> {
        let referenced = referenced
            .iter()
            .filter_map(|marker| marker.strip_prefix("local-artwork:"))
            .map(PathBuf::from)
            .collect::<HashSet<_>>();
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.root)
            .map_err(|error| format!("could not inspect local artwork cache: {error}"))?
        {
            let entry =
                entry.map_err(|error| format!("could not inspect cached artwork: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("could not inspect cached artwork type: {error}"))?;
            let path = entry.path();
            if !file_type.is_file() || !is_managed_artwork_file(&path) || referenced.contains(&path)
            {
                continue;
            }
            std::fs::remove_file(&path)
                .map_err(|error| format!("could not remove stale cached artwork: {error}"))?;
            removed += 1;
        }
        Ok(removed)
    }
}

pub fn scan_directory(
    directory: &MusicDirectory,
    artwork: &LocalArtworkStore,
    previous: &[ScannedLocalTrack],
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

    let by_path = previous
        .iter()
        .enumerate()
        .map(|(index, track)| (track.canonical_path.as_str(), index))
        .collect::<HashMap<_, _>>();
    let mut identity_counts = HashMap::new();
    let mut by_identity = HashMap::new();
    for (index, track) in previous.iter().enumerate() {
        if let Some(identity) = track.source_identity.as_deref() {
            *identity_counts.entry(identity).or_insert(0_usize) += 1;
            by_identity.insert(identity, index);
        }
    }

    let first_seen_at_ms = current_time_ms();
    let mut tracks = Vec::new();
    let mut indexed_tracks: usize = 0;
    let mut unchanged_tracks: usize = 0;
    let mut skipped_files = 0;
    let mut reused_local_files = HashSet::new();
    let mut sidecar_artwork_by_directory = HashMap::new();
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
        let sidecar_artwork = entry.path().parent().and_then(|parent| {
            sidecar_artwork_by_directory
                .entry(parent.to_path_buf())
                .or_insert_with(|| folder_artwork_candidate(entry.path()))
                .clone()
        });
        let observation = match observe_file(&root, entry.path(), mime_type, sidecar_artwork) {
            Ok(observation) => observation,
            Err(error) => {
                skipped_files += 1;
                warn!(category = "LOCAL_LIBRARY", event = "track_scan_failed", path = %entry.path().display(), reason = %error);
                continue;
            }
        };
        let previous_track = by_path
            .get(observation.canonical_path.as_str())
            .copied()
            .or_else(|| {
                let identity = observation.source_identity.as_deref()?;
                (identity_counts.get(identity) == Some(&1))
                    .then(|| by_identity[identity])
                    .filter(|index| !Path::new(&previous[*index].canonical_path).is_file())
            })
            .and_then(|index| {
                let track = &previous[index];
                let available = track
                    .local_file_id
                    .is_none_or(|local_file_id| !reused_local_files.contains(&local_file_id));
                available.then_some(track)
            });

        let result = if previous_track.is_some_and(|track| is_unchanged(track, &observation)) {
            let mut track = previous_track
                .expect("unchanged files have previous scan state")
                .clone();
            track.canonical_path = observation.canonical_path;
            track.relative_path = observation.relative_path;
            track.mime_type = observation.mime_type.into();
            track.file_size_bytes = observation.file_size_bytes;
            track.modified_at_ms = observation.modified_at_ms;
            track.modified_at_ns = observation.modified_at_ns;
            track.source_identity = observation.source_identity;
            apply_sidecar_state(&mut track, observation.sidecar_artwork.as_ref());
            track.metadata_changed = false;
            unchanged_tracks += 1;
            Ok(track)
        } else {
            indexed_tracks += 1;
            scan_track(observation, previous_track, first_seen_at_ms, artwork)
        };
        match result {
            Ok(track) => {
                if let Some(local_file_id) = track.local_file_id {
                    reused_local_files.insert(local_file_id);
                }
                tracks.push(track);
            }
            Err(error) => {
                indexed_tracks = indexed_tracks.saturating_sub(1);
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
        indexed_tracks,
        unchanged_tracks,
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

pub fn read_embedded_lyrics(path: &Path) -> Result<Option<Lyrics>, String> {
    let sidecar = path.with_extension("lrc");
    if sidecar.is_file() {
        if let Ok(text) = std::fs::read_to_string(&sidecar) {
            if let Some(lyrics) = parse_lyrics_text(&text) {
                return Ok(Some(lyrics));
            }
        }
    }
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
    Ok(value.and_then(parse_lyrics_text))
}

fn parse_lyrics_text(value: &str) -> Option<Lyrics> {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut offset_ms = 0_i64;
    let mut lines = Vec::new();
    for raw_line in trimmed.lines() {
        if let Some(raw_offset) = raw_line
            .strip_prefix("[offset:")
            .and_then(|line| line.strip_suffix(']'))
        {
            offset_ms = raw_offset.trim().parse().unwrap_or(0);
            continue;
        }
        let mut remaining = raw_line;
        let mut timestamps = Vec::new();
        while let Some(after_open) = remaining.strip_prefix('[') {
            let Some((stamp, rest)) = after_open.split_once(']') else {
                break;
            };
            let Some(start_ms) = parse_lrc_timestamp(stamp) else {
                break;
            };
            timestamps.push(start_ms.saturating_add_signed(offset_ms));
            remaining = rest;
        }
        let text = remaining.trim();
        if !text.is_empty() {
            lines.extend(timestamps.into_iter().map(|start_ms| TimedLyricsLine {
                start_ms,
                end_ms: None,
                text: text.to_owned(),
            }));
        }
    }
    lines.sort_by_key(|line| line.start_ms);
    for index in 0..lines.len().saturating_sub(1) {
        lines[index].end_ms = Some(lines[index + 1].start_ms);
    }
    Some(Lyrics {
        text: trimmed.to_owned(),
        synchronized: !lines.is_empty(),
        lines,
        attribution: None,
    })
}

fn parse_lrc_timestamp(value: &str) -> Option<u64> {
    let (minutes, seconds) = value.split_once(':')?;
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<f64>().ok()?;
    if !(0.0..60.0).contains(&seconds) {
        return None;
    }
    Some(
        minutes
            .saturating_mul(60_000)
            .saturating_add((seconds * 1_000.0).round() as u64),
    )
}

fn observe_file(
    root: &Path,
    path: &Path,
    mime_type: &'static str,
    sidecar_artwork: Option<SidecarArtwork>,
) -> Result<FileObservation, String> {
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
    let modified = metadata.modified().ok();
    Ok(FileObservation {
        sidecar_artwork,
        canonical,
        canonical_path,
        relative_path,
        mime_type,
        file_size_bytes: metadata.len(),
        modified_at_ms: modified.and_then(system_time_ms).unwrap_or(0),
        modified_at_ns: modified.and_then(system_time_ns),
        source_identity: filesystem_source_identity(&metadata),
    })
}

fn is_unchanged(previous: &ScannedLocalTrack, current: &FileObservation) -> bool {
    cached_artwork_available(previous.song.thumbnail_url.as_deref())
        && previous.file_size_bytes == current.file_size_bytes
        && previous.modified_at_ns.is_some()
        && previous.modified_at_ns == current.modified_at_ns
        && previous.sidecar_artwork_path.as_deref()
            == current
                .sidecar_artwork
                .as_ref()
                .map(|candidate| candidate.canonical_path.as_str())
        && previous.sidecar_artwork_size_bytes
            == current
                .sidecar_artwork
                .as_ref()
                .map(|candidate| candidate.file_size_bytes)
        && previous.sidecar_artwork_modified_at_ns
            == current
                .sidecar_artwork
                .as_ref()
                .and_then(|candidate| candidate.modified_at_ns)
}

fn cached_artwork_available(marker: Option<&str>) -> bool {
    marker
        .and_then(|value| value.strip_prefix("local-artwork:"))
        .is_none_or(|path| Path::new(path).is_file())
}

fn apply_sidecar_state(track: &mut ScannedLocalTrack, sidecar: Option<&SidecarArtwork>) {
    track.sidecar_artwork_path = sidecar.map(|candidate| candidate.canonical_path.clone());
    track.sidecar_artwork_size_bytes = sidecar.map(|candidate| candidate.file_size_bytes);
    track.sidecar_artwork_modified_at_ns = sidecar.and_then(|candidate| candidate.modified_at_ns);
}

fn scan_track(
    observation: FileObservation,
    previous: Option<&ScannedLocalTrack>,
    first_seen_at_ms: i64,
    artwork: &LocalArtworkStore,
) -> Result<ScannedLocalTrack, String> {
    let song_id = previous.map_or_else(
        || {
            SongId::new(format!(
                "local:{}",
                content_identity(&observation.canonical, observation.file_size_bytes)?
            ))
            .ok_or_else(|| "generated local song ID was blank".to_owned())
        },
        |track| Ok(track.song.id.clone()),
    )?;

    let tagged = Probe::open(&observation.canonical)
        .and_then(|probe| probe.read())
        .map_err(|error| format!("could not read audio metadata: {error}"));
    let fallback_title = observation
        .canonical
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Unknown Track")
        .to_owned();

    let (
        title,
        track_artist,
        album_artist,
        album_name,
        duration_ms,
        embedded_artwork,
        normalization_gain_metadata,
    ) = match tagged {
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
            let normalization_gain_metadata = normalization_gain_metadata(tagged.tags().iter());
            (
                title,
                track_artist,
                album_artist,
                album_name,
                duration_ms,
                embedded_artwork,
                normalization_gain_metadata,
            )
        }
        Err(error) => {
            debug!(category = "LOCAL_LIBRARY", event = "metadata_fallback_used", path = %observation.canonical.display(), reason = %error);
            (fallback_title, None, None, None, None, None, None)
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

    let thumbnail_url = embedded_artwork.or_else(|| {
        observation
            .sidecar_artwork
            .as_ref()
            .and_then(|candidate| cache_folder_artwork(candidate, artwork))
    });

    Ok(ScannedLocalTrack {
        local_file_id: previous.and_then(|track| track.local_file_id),
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
        canonical_path: observation.canonical_path,
        relative_path: observation.relative_path,
        mime_type: observation.mime_type.into(),
        file_size_bytes: observation.file_size_bytes,
        modified_at_ms: observation.modified_at_ms,
        modified_at_ns: observation.modified_at_ns,
        source_identity: observation.source_identity,
        sidecar_artwork_path: observation
            .sidecar_artwork
            .as_ref()
            .map(|candidate| candidate.canonical_path.clone()),
        sidecar_artwork_size_bytes: observation
            .sidecar_artwork
            .as_ref()
            .map(|candidate| candidate.file_size_bytes),
        sidecar_artwork_modified_at_ns: observation
            .sidecar_artwork
            .as_ref()
            .and_then(|candidate| candidate.modified_at_ns),
        normalization_gain_metadata,
        artist_names,
        first_seen_at_ms: previous.map_or(first_seen_at_ms, |track| track.first_seen_at_ms),
        metadata_changed: true,
    })
}

fn normalization_gain_metadata<'a>(
    tags: impl IntoIterator<Item = &'a Tag>,
) -> Option<NormalizationGainMetadata> {
    let mut replay_gain = NormalizationGainMetadata::default();
    let mut r128_track_gain = None;
    let mut r128_album_gain = None;

    for tag in tags {
        for item in tag.items() {
            let Some(value) = item.value().text() else {
                continue;
            };
            match item.key() {
                ItemKey::ReplayGainTrackGain => {
                    replay_gain.track_gain_db =
                        replay_gain.track_gain_db.or_else(|| parse_gain_db(value));
                }
                ItemKey::ReplayGainAlbumGain => {
                    replay_gain.album_gain_db =
                        replay_gain.album_gain_db.or_else(|| parse_gain_db(value));
                }
                ItemKey::ReplayGainTrackPeak => {
                    replay_gain.track_peak = replay_gain.track_peak.or_else(|| parse_peak(value));
                }
                ItemKey::ReplayGainAlbumPeak => {
                    replay_gain.album_peak = replay_gain.album_peak.or_else(|| parse_peak(value));
                }
                ItemKey::Unknown(key) => match canonical_gain_key(key).as_str() {
                    "REPLAYGAINTRACKGAIN" => {
                        replay_gain.track_gain_db =
                            replay_gain.track_gain_db.or_else(|| parse_gain_db(value));
                    }
                    "REPLAYGAINALBUMGAIN" => {
                        replay_gain.album_gain_db =
                            replay_gain.album_gain_db.or_else(|| parse_gain_db(value));
                    }
                    "REPLAYGAINTRACKPEAK" => {
                        replay_gain.track_peak =
                            replay_gain.track_peak.or_else(|| parse_peak(value));
                    }
                    "REPLAYGAINALBUMPEAK" => {
                        replay_gain.album_peak =
                            replay_gain.album_peak.or_else(|| parse_peak(value));
                    }
                    "R128TRACKGAIN" => {
                        r128_track_gain = r128_track_gain.or_else(|| parse_r128_gain(value));
                    }
                    "R128ALBUMGAIN" => {
                        r128_album_gain = r128_album_gain.or_else(|| parse_r128_gain(value));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    replay_gain.track_gain_db = replay_gain.track_gain_db.or(r128_track_gain);
    replay_gain.album_gain_db = replay_gain.album_gain_db.or(r128_album_gain);
    (replay_gain != NormalizationGainMetadata::default()).then_some(replay_gain)
}

fn canonical_gain_key(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn parse_gain_db(value: &str) -> Option<f64> {
    let value = value.trim();
    let number = value
        .get(..value.len().saturating_sub(2))
        .filter(|_| {
            value
                .get(value.len().saturating_sub(2)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case("db"))
        })
        .unwrap_or(value)
        .trim();
    let gain = number.parse::<f64>().ok()?;
    gain.is_finite().then_some(gain)
}

fn parse_peak(value: &str) -> Option<f64> {
    let peak = value.trim().parse::<f64>().ok()?;
    (peak.is_finite() && peak > 0.0).then_some(peak)
}

fn parse_r128_gain(value: &str) -> Option<f64> {
    let gain = value.trim().parse::<i32>().ok()?;
    Some(f64::from(gain) / 256.0 + 5.0)
}

fn folder_artwork_candidate(audio_path: &Path) -> Option<SidecarArtwork> {
    let parent = audio_path.parent()?;
    const STEMS: &[&str] = &["cover", "folder", "front", "album"];
    const EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp"];
    let mut candidates = std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_ascii_lowercase();
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            let stem_order = STEMS.iter().position(|candidate| *candidate == stem)?;
            let extension_order = EXTENSIONS
                .iter()
                .position(|candidate| *candidate == extension)?;
            Some((stem_order, extension_order, path))
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.cmp(right));
    candidates.into_iter().find_map(|(_, _, path)| {
        let canonical = path.canonicalize().ok()?;
        let metadata = canonical.metadata().ok()?;
        if !metadata.is_file() {
            return None;
        }
        Some(SidecarArtwork {
            canonical_path: canonical.to_str()?.to_owned(),
            path: canonical,
            file_size_bytes: metadata.len(),
            modified_at_ns: metadata.modified().ok().and_then(system_time_ns),
        })
    })
}

fn cache_folder_artwork(candidate: &SidecarArtwork, artwork: &LocalArtworkStore) -> Option<String> {
    std::fs::read(&candidate.path)
        .ok()
        .and_then(|bytes| artwork.cache(&bytes))
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

fn system_time_ms(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

fn system_time_ns(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
}

fn is_managed_artwork_file(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
        return false;
    };
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    stem.len() == 64
        && stem.bytes().all(|byte| byte.is_ascii_hexdigit())
        && matches!(extension, "jpg" | "png" | "webp")
}

fn current_time_ms() -> i64 {
    system_time_ms(SystemTime::now()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{
        append_artist_credits, image_extension, normalization_gain_metadata, parse_gain_db,
        parse_lyrics_text, parse_peak, read_embedded_lyrics, scan_directory, supported_mime_type,
        LocalArtworkStore,
    };
    use lofty::{
        file::TaggedFileExt,
        tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType},
    };
    use solmusic_application::MusicDirectory;
    use std::{fs, path::Path};

    fn test_directory(root: &Path) -> MusicDirectory {
        MusicDirectory {
            id: 1,
            path: root.to_string_lossy().into_owned(),
            added_at_ms: 1,
            last_scanned_at_ms: None,
            last_scan_attempt_at_ms: None,
            status: "READY".into(),
            last_error: None,
            track_count: 0,
        }
    }

    fn write_test_wav(path: &Path) {
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
        fs::write(path, wav).unwrap();
    }

    #[test]
    fn parses_plain_and_timed_local_lyrics() {
        let plain = parse_lyrics_text("  Line one\r\nLine two\r\n  ").unwrap();
        assert_eq!(plain.text, "Line one\nLine two");
        assert!(!plain.synchronized);
        assert!(plain.lines.is_empty());
        let timed = parse_lyrics_text("[00:01.50]First\n[00:03.000]Second").unwrap();
        assert!(timed.synchronized);
        assert_eq!(timed.lines[0].start_ms, 1_500);
        assert_eq!(timed.lines[0].end_ms, Some(3_000));
        assert_eq!(timed.lines[1].text, "Second");
        assert!(parse_lyrics_text(" \r\n\t").is_none());
    }

    #[test]
    fn reads_sidecar_lrc_before_audio_metadata() {
        let root = std::env::temp_dir().join(format!("solmusic-lrc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let audio = root.join("song.mp3");
        fs::write(&audio, b"not needed for sidecar lyrics").unwrap();
        fs::write(root.join("song.lrc"), "[00:01.00]Sidecar line").unwrap();
        let lyrics = read_embedded_lyrics(&audio).unwrap().unwrap();
        assert!(lyrics.synchronized);
        assert_eq!(lyrics.lines[0].text, "Sidecar line");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_sidecar_falls_back_to_embedded_lyrics() {
        let root =
            std::env::temp_dir().join(format!("solmusic-lrc-fallback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let audio = root.join("song.wav");
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
        fs::write(&audio, wav).unwrap();

        let tagged = lofty::read_from_path(&audio).unwrap();
        let mut tag = Tag::new(tagged.primary_tag_type());
        tag.insert_text(ItemKey::Lyrics, "Embedded fallback".into());
        tag.save_to_path(&audio, lofty::config::WriteOptions::default())
            .unwrap();
        fs::write(root.join("song.lrc"), [0xff, 0xfe, 0xfd]).unwrap();

        let lyrics = read_embedded_lyrics(&audio).unwrap().unwrap();
        assert_eq!(lyrics.text, "Embedded fallback");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parses_replaygain_and_r128_metadata_without_inventing_values() {
        let mut replay_gain = Tag::new(TagType::VorbisComments);
        replay_gain.insert_text(ItemKey::ReplayGainTrackGain, " -7.25 dB ".into());
        replay_gain.insert_unchecked(TagItem::new(
            ItemKey::Unknown("replaygain-album-gain".into()),
            ItemValue::Text("-6.5db".into()),
        ));
        replay_gain.insert_text(ItemKey::ReplayGainTrackPeak, "0.987654".into());
        replay_gain.insert_text(ItemKey::ReplayGainAlbumPeak, "1.021".into());
        replay_gain.insert_unchecked(TagItem::new(
            ItemKey::Unknown("R128_TRACK_GAIN".into()),
            ItemValue::Text("-2560".into()),
        ));
        let metadata = normalization_gain_metadata([&replay_gain]).unwrap();
        assert_eq!(metadata.track_gain_db, Some(-7.25));
        assert_eq!(metadata.album_gain_db, Some(-6.5));
        assert_eq!(metadata.track_peak, Some(0.987654));
        assert_eq!(metadata.album_peak, Some(1.021));

        let mut r128 = Tag::new(TagType::VorbisComments);
        r128.insert_unchecked(TagItem::new(
            ItemKey::Unknown("r128 track gain".into()),
            ItemValue::Text("-2560".into()),
        ));
        r128.insert_unchecked(TagItem::new(
            ItemKey::Unknown("r128_album_gain".into()),
            ItemValue::Text("-1280".into()),
        ));
        let metadata = normalization_gain_metadata([&r128]).unwrap();
        assert_eq!(metadata.track_gain_db, Some(-5.0));
        assert_eq!(metadata.album_gain_db, Some(0.0));
        assert_eq!(metadata.track_peak, None);
        assert_eq!(metadata.album_peak, None);

        assert_eq!(parse_gain_db("+3 DB"), Some(3.0));
        assert_eq!(parse_gain_db("not gain"), None);
        assert_eq!(parse_peak("0"), None);
        assert_eq!(parse_peak("NaN"), None);
        assert!(normalization_gain_metadata(std::iter::empty::<&Tag>()).is_none());
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
            &[],
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
    fn unchanged_rescan_reuses_persisted_metadata() {
        let root = std::env::temp_dir().join(format!(
            "solmusic-incremental-unchanged-{}",
            std::process::id()
        ));
        let artwork_root = root.with_extension("artwork-cache");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&artwork_root);
        fs::create_dir_all(&root).unwrap();
        write_test_wav(&root.join("track.wav"));
        let artwork = LocalArtworkStore::new(artwork_root.clone()).unwrap();

        let first = scan_directory(&test_directory(&root), &artwork, &[]).unwrap();
        assert!(first.tracks[0].metadata_changed);
        let second = scan_directory(&test_directory(&root), &artwork, &first.tracks).unwrap();

        assert_eq!(second.tracks.len(), 1);
        assert!(!second.tracks[0].metadata_changed);
        assert_eq!(second.tracks[0].song, first.tracks[0].song);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(artwork_root).unwrap();
    }

    #[test]
    fn audio_metadata_edit_is_reindexed_without_changing_canonical_id() {
        let root = std::env::temp_dir().join(format!(
            "solmusic-incremental-metadata-{}",
            std::process::id()
        ));
        let artwork_root = root.with_extension("artwork-cache");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&artwork_root);
        fs::create_dir_all(&root).unwrap();
        let audio = root.join("track.wav");
        write_test_wav(&audio);
        let artwork = LocalArtworkStore::new(artwork_root.clone()).unwrap();
        let first = scan_directory(&test_directory(&root), &artwork, &[]).unwrap();

        let tagged = lofty::read_from_path(&audio).unwrap();
        let mut tag = Tag::new(tagged.primary_tag_type());
        tag.insert_text(ItemKey::TrackTitle, "Edited title".into());
        tag.save_to_path(&audio, lofty::config::WriteOptions::default())
            .unwrap();
        let second = scan_directory(&test_directory(&root), &artwork, &first.tracks).unwrap();

        assert!(second.tracks[0].metadata_changed);
        assert_eq!(second.tracks[0].song.id, first.tracks[0].song.id);
        assert_eq!(second.tracks[0].song.title, "Edited title");
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(artwork_root).unwrap();
    }

    #[test]
    fn rename_preserves_identity_and_skips_metadata_on_supported_filesystems() {
        let root = std::env::temp_dir().join(format!(
            "solmusic-incremental-rename-{}",
            std::process::id()
        ));
        let artwork_root = root.with_extension("artwork-cache");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&artwork_root);
        fs::create_dir_all(&root).unwrap();
        let old_path = root.join("old.wav");
        let new_path = root.join("new.wav");
        write_test_wav(&old_path);
        let artwork = LocalArtworkStore::new(artwork_root.clone()).unwrap();
        let first = scan_directory(&test_directory(&root), &artwork, &[]).unwrap();
        fs::rename(&old_path, &new_path).unwrap();

        let second = scan_directory(&test_directory(&root), &artwork, &first.tracks).unwrap();
        assert_eq!(second.tracks[0].song.id, first.tracks[0].song.id);
        assert_eq!(second.tracks[0].canonical_path, new_path.to_str().unwrap());
        #[cfg(unix)]
        assert!(!second.tracks[0].metadata_changed);
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(artwork_root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn hard_link_is_kept_as_a_duplicate_source_not_mistaken_for_a_rename() {
        let root = std::env::temp_dir().join(format!(
            "solmusic-incremental-hard-link-{}",
            std::process::id()
        ));
        let artwork_root = root.with_extension("artwork-cache");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&artwork_root);
        fs::create_dir_all(&root).unwrap();
        let original = root.join("original.wav");
        write_test_wav(&original);
        let artwork = LocalArtworkStore::new(artwork_root.clone()).unwrap();
        let first = scan_directory(&test_directory(&root), &artwork, &[]).unwrap();
        fs::hard_link(&original, root.join("copy.wav")).unwrap();

        let second = scan_directory(&test_directory(&root), &artwork, &first.tracks).unwrap();
        assert_eq!(second.tracks.len(), 2);
        assert_eq!(
            second
                .tracks
                .iter()
                .filter(|track| track.metadata_changed)
                .count(),
            1
        );
        assert!(second
            .tracks
            .iter()
            .all(|track| track.song.id == first.tracks[0].song.id));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(artwork_root).unwrap();
    }

    #[test]
    fn changed_sidecar_rebuilds_and_prunes_cached_artwork() {
        let root =
            std::env::temp_dir().join(format!("solmusic-incremental-cover-{}", std::process::id()));
        let artwork_root = root.with_extension("artwork-cache");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&artwork_root);
        fs::create_dir_all(&root).unwrap();
        write_test_wav(&root.join("track.wav"));
        let cover = root.join("cover.png");
        fs::write(&cover, b"\x89PNG\r\n\x1a\nfirst").unwrap();
        let artwork = LocalArtworkStore::new(artwork_root.clone()).unwrap();
        let first = scan_directory(&test_directory(&root), &artwork, &[]).unwrap();
        let first_marker = first.tracks[0].song.thumbnail_url.clone().unwrap();

        fs::write(&cover, b"\x89PNG\r\n\x1a\na different cover").unwrap();
        let second = scan_directory(&test_directory(&root), &artwork, &first.tracks).unwrap();
        let second_marker = second.tracks[0].song.thumbnail_url.clone().unwrap();
        assert!(second.tracks[0].metadata_changed);
        assert_ne!(second_marker, first_marker);

        assert_eq!(
            artwork.prune(std::slice::from_ref(&second_marker)).unwrap(),
            1
        );
        assert!(!Path::new(first_marker.trim_start_matches("local-artwork:")).exists());
        assert!(Path::new(second_marker.trim_start_matches("local-artwork:")).exists());

        fs::remove_file(&cover).unwrap();
        let third = scan_directory(&test_directory(&root), &artwork, &second.tracks).unwrap();
        assert!(third.tracks[0].metadata_changed);
        assert_eq!(third.tracks[0].song.thumbnail_url, None);
        assert_eq!(artwork.prune(&[]).unwrap(), 1);
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
