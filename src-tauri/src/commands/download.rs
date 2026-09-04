use std::path::Path;
#[cfg(desktop)]
use std::path::PathBuf;

use lofty::{
    config::WriteOptions,
    file::TaggedFileExt,
    tag::{Accessor, Tag, TagExt},
};
use serde::Serialize;
use solmusic_application::SunnySongApp;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

#[cfg(desktop)]
use crate::local_library::scan_directory;
use crate::{dto::SongDto, local_library::LocalArtworkStore, media_proxy::MediaProxy};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResultDto {
    pub location: String,
    pub file_name: String,
}

#[tauri::command]
pub async fn download_song(
    app_handle: AppHandle,
    app: State<'_, SunnySongApp>,
    media_proxy: State<'_, std::sync::Arc<MediaProxy>>,
    artwork: State<'_, LocalArtworkStore>,
    song: SongDto,
    directory_id: Option<i64>,
    android_tree_uri: Option<String>,
) -> Result<DownloadResultDto, String> {
    #[cfg(desktop)]
    let _ = &android_tree_uri;
    #[cfg(mobile)]
    let _ = (&directory_id, &artwork);
    let song: solmusic_application::domain::Song = song.try_into()?;
    let mut source = app
        .download_source(&song)
        .await
        .map_err(|error| error.to_string())?;
    let extension = source
        .local_path
        .as_deref()
        .and_then(Path::extension)
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .unwrap_or_else(|| extension_for_mime(&source.mime_type).to_owned());
    let file_name = format!(
        "{} - {}.{}",
        safe_file_component(&song.artist.name),
        safe_file_component(&song.title),
        extension
    );
    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|error| format!("could not locate the app cache: {error}"))?;
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|error| format!("could not create the download cache: {error}"))?;
    let temporary_path = cache_dir.join(format!("download-{}.{}", Uuid::new_v4(), extension));

    let staged: Result<(), String> = if let Some(local_path) = source.local_path.clone() {
        tokio::fs::copy(local_path, &temporary_path)
            .await
            .map(|_| ())
            .map_err(|error| format!("could not copy local audio: {error}"))
    } else {
        let mut refresh_attempts = 0;
        loop {
            match media_proxy
                .download_url_to_path(&source.url, &temporary_path, source.request_profile)
                .await
            {
                Ok(_) => break Ok(()),
                Err(error) if refresh_attempts < 2 && is_signed_url_rejection(&error) => {
                    refresh_attempts += 1;
                    source = app
                        .download_source(&song)
                        .await
                        .map_err(|refresh_error| refresh_error.to_string())?;
                }
                Err(error) => break Err(error.to_owned()),
            }
        }
    };
    if let Err(error) = staged {
        let _ = tokio::fs::remove_file(&temporary_path).await;
        return Err(error);
    }
    if let Err(error) = write_metadata(&temporary_path, &song) {
        let _ = tokio::fs::remove_file(&temporary_path).await;
        return Err(error);
    }

    #[cfg(desktop)]
    let location = {
        let directory_id = directory_id.ok_or("choose a music folder")?;
        let directory = app
            .music_directories()
            .map_err(|error| error.to_string())?
            .into_iter()
            .find(|directory| directory.id == directory_id)
            .ok_or("the selected music folder is no longer configured")?;
        let destination = unique_destination(&PathBuf::from(&directory.path), &file_name);
        if tokio::fs::rename(&temporary_path, &destination)
            .await
            .is_err()
        {
            if let Err(error) = tokio::fs::copy(&temporary_path, &destination).await {
                let _ = tokio::fs::remove_file(&temporary_path).await;
                return Err(format!("could not save downloaded audio: {error}"));
            }
            let _ = tokio::fs::remove_file(&temporary_path).await;
        }
        let scan_target = directory.clone();
        let artwork = artwork.inner().clone();
        match tauri::async_runtime::spawn_blocking(move || scan_directory(&scan_target, &artwork))
            .await
        {
            Ok(Ok(batch)) => {
                if let Err(error) = app.store_directory_scan(
                    directory.id,
                    &batch.tracks,
                    current_time_ms(),
                    batch.skipped_files,
                    batch.complete,
                    batch.duration_ms,
                ) {
                    tracing::warn!(category = "DOWNLOAD", event = "download_rescan_failed", directory_id = directory.id, reason = %error);
                }
            }
            Ok(Err(error)) => {
                tracing::warn!(category = "DOWNLOAD", event = "download_rescan_failed", directory_id = directory.id, reason = %error)
            }
            Err(error) => {
                tracing::warn!(category = "DOWNLOAD", event = "download_rescan_task_failed", directory_id = directory.id, reason = %error)
            }
        }
        destination.to_string_lossy().into_owned()
    };

    #[cfg(mobile)]
    let location = {
        use tauri_plugin_solmusic_storage::{PublishAudioRequest, StorageExt};
        let tree_uri = android_tree_uri.ok_or("choose an Android download folder")?;
        let request = PublishAudioRequest {
            tree_uri,
            display_name: file_name.clone(),
            mime_type: source.mime_type,
            source_path: temporary_path.to_string_lossy().into_owned(),
        };
        let handle = app_handle.clone();
        let response = tauri::async_runtime::spawn_blocking(move || {
            handle.solmusic_storage().publish_audio(request)
        })
        .await
        .map_err(|error| format!("Android storage task failed: {error}"))?
        .map_err(|error| error.to_string())?;
        let _ = tokio::fs::remove_file(&temporary_path).await;
        response.uri
    };

    Ok(DownloadResultDto {
        location,
        file_name,
    })
}

#[cfg(mobile)]
#[tauri::command]
pub async fn pick_android_download_directory(
    app_handle: AppHandle,
) -> Result<tauri_plugin_solmusic_storage::StorageDirectory, String> {
    use tauri_plugin_solmusic_storage::StorageExt;
    tauri::async_runtime::spawn_blocking(move || app_handle.solmusic_storage().pick_directory())
        .await
        .map_err(|error| format!("Android directory picker failed: {error}"))?
        .map_err(|error| error.to_string())
}

#[cfg(desktop)]
#[tauri::command]
pub async fn pick_android_download_directory() -> Result<serde_json::Value, String> {
    Err("Android storage selection is only available on Android".into())
}

#[cfg(desktop)]
fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn is_signed_url_rejection(error: &str) -> bool {
    error.contains("HTTP 401") || error.contains("HTTP 403")
}

fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type.split(';').next().unwrap_or(mime_type).trim() {
        "audio/webm" => "webm",
        "audio/ogg" | "audio/opus" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/aac" => "aac",
        "audio/flac" => "flac",
        _ => "m4a",
    }
}

fn write_metadata(path: &Path, song: &solmusic_application::domain::Song) -> Result<(), String> {
    let tagged = lofty::read_from_path(path)
        .map_err(|error| format!("could not inspect downloaded audio metadata: {error}"))?;
    let mut tag = Tag::new(tagged.primary_tag_type());
    tag.set_title(song.title.clone());
    tag.set_artist(song.artist.name.clone());
    if let Some(album) = &song.album_name {
        tag.set_album(album.clone());
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|error| format!("could not write audio metadata: {error}"))
}

fn safe_file_component(value: &str) -> String {
    let cleaned = value
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();
    let cleaned = cleaned.trim().trim_matches('.');
    if cleaned.is_empty() {
        "Unknown".into()
    } else {
        cleaned.chars().take(96).collect()
    }
}

#[cfg(desktop)]
fn unique_destination(directory: &Path, file_name: &str) -> PathBuf {
    let initial = directory.join(file_name);
    if !initial.exists() {
        return initial;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Song");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("m4a");
    for suffix in 1..10_000 {
        let candidate = directory.join(format!("{stem} ({suffix}).{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}.{extension}", Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::{extension_for_mime, is_signed_url_rejection, safe_file_component};

    #[test]
    fn sanitizes_download_file_names() {
        assert_eq!(safe_file_component("A/B: C?"), "A_B_ C_");
        assert_eq!(safe_file_component("..."), "Unknown");
    }

    #[test]
    fn selects_an_extension_matching_the_stream_container() {
        assert_eq!(extension_for_mime("audio/webm; codecs=opus"), "webm");
        assert_eq!(extension_for_mime("audio/mp4"), "m4a");
        assert_eq!(extension_for_mime("audio/mpeg"), "mp3");
    }

    #[test]
    fn recognizes_rejected_signed_media_urls() {
        assert!(is_signed_url_rejection(
            "media provider returned HTTP 403 Forbidden"
        ));
        assert!(!is_signed_url_rejection("media request timed out"));
    }
}
