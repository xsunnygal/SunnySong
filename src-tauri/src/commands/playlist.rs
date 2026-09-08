use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use solmusic_application::{domain::SongId, SunnySongApp};
use tauri::State;

use crate::{
    commands::library::proxy_artwork,
    dto::{PlaylistDto, PlaylistTrackDto, SongDto},
    jellyfin::JellyfinService,
    media_proxy::MediaProxy,
};

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn current_time_ms() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)
        .map(|duration| duration.as_millis() as i64)
}

#[tauri::command]
pub fn get_playlists(app: State<'_, SunnySongApp>) -> Result<Vec<PlaylistDto>, String> {
    app.playlists()
        .map(|playlists| playlists.into_iter().map(Into::into).collect())
        .map_err(error)
}

#[tauri::command]
pub fn create_playlist(app: State<'_, SunnySongApp>, name: String) -> Result<PlaylistDto, String> {
    app.create_playlist(&name, current_time_ms()?)
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub fn get_playlist_tracks(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    playlist_id: String,
) -> Result<Vec<PlaylistTrackDto>, String> {
    let mut tracks = app
        .playlist_tracks(&playlist_id)
        .map_err(error)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<PlaylistTrackDto>>();
    for track in &mut tracks {
        proxy_artwork(&mut track.song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(tracks)
}

#[tauri::command]
pub fn rename_playlist(
    app: State<'_, SunnySongApp>,
    playlist_id: String,
    name: String,
) -> Result<PlaylistDto, String> {
    app.rename_playlist(&playlist_id, &name, current_time_ms()?)
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub fn delete_playlist(app: State<'_, SunnySongApp>, playlist_id: String) -> Result<(), String> {
    app.delete_playlist(&playlist_id).map_err(error)
}

#[tauri::command]
pub fn remove_song_from_playlist(
    app: State<'_, SunnySongApp>,
    playlist_id: String,
    song_id: String,
) -> Result<(), String> {
    let song_id = SongId::new(song_id).ok_or("song id cannot be blank")?;
    app.remove_song_from_playlist(&playlist_id, &song_id, current_time_ms()?)
        .map_err(error)
}

#[tauri::command]
pub fn reorder_playlist_tracks(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    playlist_id: String,
    song_ids: Vec<String>,
) -> Result<Vec<PlaylistTrackDto>, String> {
    let mut tracks = app
        .reorder_playlist_tracks(&playlist_id, &song_ids, current_time_ms()?)
        .map_err(error)?
        .into_iter()
        .map(Into::into)
        .collect::<Vec<PlaylistTrackDto>>();
    for track in &mut tracks {
        proxy_artwork(&mut track.song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(tracks)
}

#[tauri::command]
pub fn add_song_to_playlist(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    playlist_id: String,
    song: SongDto,
) -> Result<PlaylistTrackDto, String> {
    let song = song.try_into()?;
    let mut track: PlaylistTrackDto = app
        .add_song_to_playlist(&playlist_id, &song, current_time_ms()?)
        .map_err(error)?
        .into();
    proxy_artwork(&mut track.song.thumbnail_url, &jellyfin, &media_proxy)?;
    Ok(track)
}
