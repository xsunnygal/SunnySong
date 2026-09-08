use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use solmusic_application::SunnySongApp;
use tauri::State;
use tracing::info;

use crate::{
    dto::{
        LibraryAlbumDto, LibraryFolderDto, LibraryScanResultDto, LibraryTrackDto, LocalArtistDto,
        MusicDirectoryDto, SongDto,
    },
    jellyfin::JellyfinService,
    local_library::{scan_directory, LocalArtworkStore},
    media_proxy::MediaProxy,
};

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn current_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn prune_artwork(app: &SunnySongApp, artwork: &LocalArtworkStore) {
    match app.referenced_local_artwork() {
        Ok(referenced) => match artwork.prune(&referenced) {
            Ok(removed) if removed > 0 => info!(
                category = "LOCAL_LIBRARY",
                event = "stale_artwork_pruned",
                removed
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!(
                category = "LOCAL_LIBRARY",
                event = "stale_artwork_prune_failed",
                reason = %error
            ),
        },
        Err(error) => tracing::warn!(
            category = "LOCAL_LIBRARY",
            event = "artwork_references_failed",
            reason = %error
        ),
    }
}

#[tauri::command]
pub fn get_discovery_enabled(app: State<'_, SunnySongApp>) -> Result<bool, String> {
    app.discovery_enabled().map_err(error)
}

#[tauri::command]
pub fn set_discovery_enabled(app: State<'_, SunnySongApp>, enabled: bool) -> Result<(), String> {
    app.set_discovery_enabled(enabled, current_time_ms())
        .map_err(error)
}

#[tauri::command]
pub fn get_music_directories(
    app: State<'_, SunnySongApp>,
) -> Result<Vec<MusicDirectoryDto>, String> {
    app.music_directories()
        .map(|items| items.into_iter().map(Into::into).collect())
        .map_err(error)
}

#[tauri::command]
pub async fn add_music_directory(
    app: State<'_, SunnySongApp>,
    artwork: State<'_, LocalArtworkStore>,
    path: String,
) -> Result<MusicDirectoryDto, String> {
    let canonical = PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|error| format!("could not access selected folder: {error}"))?;
    if !canonical.is_dir() {
        return Err("selected path is not a folder".into());
    }
    let canonical_text = canonical
        .to_str()
        .ok_or("non-UTF-8 music folder paths are not supported")?
        .to_owned();
    for existing in app.music_directories().map_err(error)? {
        let existing_path = PathBuf::from(&existing.path);
        if canonical == existing_path
            || canonical.starts_with(&existing_path)
            || existing_path.starts_with(&canonical)
        {
            return Err(format!(
                "music folder overlaps an existing folder: {}",
                existing.path
            ));
        }
    }

    let directory = app
        .add_music_directory(&canonical_text, current_time_ms())
        .map_err(error)?;
    let attempted_at_ms = current_time_ms();
    app.set_music_directory_status(directory.id, "SCANNING", None, attempted_at_ms)
        .map_err(error)?;
    let previous = app.local_scan_state(directory.id).map_err(error)?;
    let scan_target = directory.clone();
    let artwork_store = artwork.inner().clone();
    match tauri::async_runtime::spawn_blocking(move || {
        scan_directory(&scan_target, &artwork_store, &previous)
    })
    .await
    {
        Ok(Ok(batch)) => {
            app.store_directory_scan(
                directory.id,
                &batch.tracks,
                current_time_ms(),
                batch.skipped_files,
                batch.complete,
                batch.duration_ms,
            )
            .map_err(error)?;
            prune_artwork(&app, &artwork);
        }
        Ok(Err(scan_error)) => {
            tracing::warn!(category = "SCANNER", event = "directory_scan_unavailable", directory_id = directory.id, reason = %scan_error);
            app.set_music_directory_status(
                directory.id,
                "UNAVAILABLE",
                Some(&scan_error),
                attempted_at_ms,
            )
            .map_err(error)?;
        }
        Err(join_error) => {
            let scan_error = format!("music scan task failed: {join_error}");
            app.set_music_directory_status(
                directory.id,
                "ERROR",
                Some(&scan_error),
                attempted_at_ms,
            )
            .map_err(error)?;
        }
    }
    info!(
        category = "LOCAL_LIBRARY",
        event = "music_directory_added",
        directory_id = directory.id
    );
    let updated = app
        .music_directories()
        .map_err(error)?
        .into_iter()
        .find(|item| item.id == directory.id)
        .ok_or("music directory disappeared after scan")?;
    Ok(updated.into())
}

#[tauri::command]
pub fn remove_music_directory(
    app: State<'_, SunnySongApp>,
    artwork: State<'_, LocalArtworkStore>,
    directory_id: i64,
) -> Result<(), String> {
    app.remove_music_directory(directory_id).map_err(error)?;
    prune_artwork(&app, &artwork);
    Ok(())
}

#[tauri::command]
pub async fn rescan_music_library(
    app: State<'_, SunnySongApp>,
    artwork: State<'_, LocalArtworkStore>,
) -> Result<Vec<LibraryScanResultDto>, String> {
    let mut reports = Vec::new();
    for directory in app.music_directories().map_err(error)? {
        let attempted_at_ms = current_time_ms();
        app.set_music_directory_status(directory.id, "SCANNING", None, attempted_at_ms)
            .map_err(error)?;
        let previous = app.local_scan_state(directory.id).map_err(error)?;
        let scan_target = directory.clone();
        let artwork_store = artwork.inner().clone();
        match tauri::async_runtime::spawn_blocking(move || {
            scan_directory(&scan_target, &artwork_store, &previous)
        })
        .await
        {
            Ok(Ok(batch)) => {
                let report = app
                    .store_directory_scan(
                        directory.id,
                        &batch.tracks,
                        current_time_ms(),
                        batch.skipped_files,
                        batch.complete,
                        batch.duration_ms,
                    )
                    .map_err(error)?;
                reports.push(report.into());
            }
            Ok(Err(scan_error)) => {
                tracing::warn!(category = "SCANNER", event = "directory_scan_unavailable", directory_id = directory.id, reason = %scan_error);
                app.set_music_directory_status(
                    directory.id,
                    "UNAVAILABLE",
                    Some(&scan_error),
                    attempted_at_ms,
                )
                .map_err(error)?;
                reports.push(LibraryScanResultDto {
                    directory_id: directory.id,
                    status: "UNAVAILABLE".into(),
                    indexed_tracks: 0,
                    unchanged_tracks: 0,
                    unavailable_tracks: directory.track_count as usize,
                    skipped_files: 0,
                    duration_ms: 0,
                    error: Some(scan_error),
                });
            }
            Err(join_error) => {
                let scan_error = format!("music scan task failed: {join_error}");
                app.set_music_directory_status(
                    directory.id,
                    "ERROR",
                    Some(&scan_error),
                    attempted_at_ms,
                )
                .map_err(error)?;
                reports.push(LibraryScanResultDto {
                    directory_id: directory.id,
                    status: "ERROR".into(),
                    indexed_tracks: 0,
                    unchanged_tracks: 0,
                    unavailable_tracks: directory.track_count as usize,
                    skipped_files: 0,
                    duration_ms: 0,
                    error: Some(scan_error),
                });
            }
        }
    }
    prune_artwork(&app, &artwork);
    Ok(reports)
}

#[tauri::command]
pub fn get_local_artists(
    app: State<'_, SunnySongApp>,
    query: String,
    count: usize,
    offset: usize,
) -> Result<Vec<LocalArtistDto>, String> {
    app.local_artists(query.trim(), count.clamp(1, 100), offset)
        .map(|items| items.into_iter().map(Into::into).collect())
        .map_err(error)
}

#[tauri::command]
pub fn set_local_artist_enabled(
    app: State<'_, SunnySongApp>,
    artist_id: i64,
    enabled: bool,
) -> Result<(), String> {
    app.set_local_artist_enabled(artist_id, enabled)
        .map_err(error)
}

pub(crate) fn proxy_artwork(
    artwork: &mut Option<String>,
    jellyfin: &JellyfinService,
    media_proxy: &MediaProxy,
) -> Result<(), String> {
    let Some(marker) = artwork.as_deref() else {
        return Ok(());
    };
    if let Some(url) = media_proxy.register_local_image(marker)? {
        *artwork = Some(url);
    } else if let Some(url) = jellyfin.resolve_artwork_url(marker)? {
        *artwork = Some(media_proxy.register_image(url)?);
    }
    Ok(())
}

#[tauri::command]
pub fn get_library_tracks(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    count: usize,
    offset: usize,
) -> Result<Vec<LibraryTrackDto>, String> {
    let mut items = app
        .library_tracks(count.clamp(1, 100), offset)
        .map_err(error)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<LibraryTrackDto>>();
    for item in &mut items {
        proxy_artwork(&mut item.song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(items)
}

#[tauri::command]
pub fn get_library_albums(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    count: usize,
    offset: usize,
) -> Result<Vec<LibraryAlbumDto>, String> {
    let mut items = app
        .library_albums(count.clamp(1, 100), offset)
        .map_err(error)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<LibraryAlbumDto>>();
    for item in &mut items {
        proxy_artwork(&mut item.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(items)
}

#[tauri::command]
pub fn get_library_album_tracks(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    album_id: String,
) -> Result<Vec<LibraryTrackDto>, String> {
    if album_id.trim().is_empty() {
        return Err("album ID cannot be blank".into());
    }
    let mut items = app
        .library_album_tracks(&album_id)
        .map_err(error)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<LibraryTrackDto>>();
    for item in &mut items {
        proxy_artwork(&mut item.song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(items)
}

#[tauri::command]
pub fn get_library_folders(app: State<'_, SunnySongApp>) -> Result<Vec<LibraryFolderDto>, String> {
    app.library_folders()
        .map(|items| items.into_iter().map(Into::into).collect())
        .map_err(error)
}

#[tauri::command]
pub fn search_local_music(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    query: String,
) -> Result<Vec<SongDto>, String> {
    let mut songs = app
        .search_local(query.trim(), 100)
        .map_err(error)?
        .iter()
        .map(SongDto::from)
        .collect::<Vec<_>>();
    for song in &mut songs {
        proxy_artwork(&mut song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(songs)
}
