use std::{
    collections::HashSet,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(target_os = "linux")]
use std::{process::Command, sync::OnceLock};

use solmusic_application::{
    domain::SongId, AudioQuality, CatalogFilter, QuickPickOptions, SunnySongApp,
};
use tauri::{AppHandle, State};

use crate::{
    commands::library::proxy_artwork,
    dto::{
        ArtistPageDto, CatalogSearchResultsDto, DatabaseDiagnosticsDto, DeveloperDiagnosticsDto,
        ListeningProfileDto, LyricsDto, PlaybackPreparationDto, PlaybackStateDto,
        PlaybackSummaryDto, RecentSongsPageDto, RecommendationDto, SongDto,
    },
    jellyfin::JellyfinService,
    local_library::read_embedded_lyrics,
    media_proxy::MediaProxy,
};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickPickOptionsDto {
    diverse: bool,
    new_songs: bool,
    rediscover: bool,
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

#[tauri::command]
pub fn sync_media_session(
    app_handle: AppHandle,
    update: tauri_plugin_solmusic_storage::MediaSessionUpdate,
) -> Result<(), String> {
    use tauri_plugin_solmusic_storage::StorageExt;
    app_handle
        .solmusic_storage()
        .update_media_session(update)
        .map_err(error)
}

fn current_time_ms() -> Result<i64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)
        .map(|duration| duration.as_millis() as i64)
}

#[cfg(target_os = "linux")]
fn ensure_audio_runtime() -> Result<(), String> {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            let available = Command::new("gst-inspect-1.0")
                .arg("autoaudiosink")
                .output()
                .is_ok_and(|output| output.status.success());
            available.then_some(()).ok_or_else(|| {
                "Linux audio support is incomplete. Install the GStreamer good plugins (gst-plugins-good on Arch Linux) and restart SunnySong.".into()
            })
        })
        .clone()
}

#[cfg(not(target_os = "linux"))]
fn ensure_audio_runtime() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub fn set_audio_quality(app: State<'_, SunnySongApp>, quality: String) -> Result<(), String> {
    let quality = match quality.as_str() {
        "low" => AudioQuality::Low,
        "medium" => AudioQuality::Medium,
        "high" => AudioQuality::High,
        _ => return Err("invalid audio quality".into()),
    };
    app.set_audio_quality(quality);
    Ok(())
}

#[tauri::command]
pub async fn search_music(
    app: State<'_, SunnySongApp>,
    query: String,
) -> Result<Vec<SongDto>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    app.search_discovery(query, 30)
        .await
        .map(|songs| songs.iter().map(SongDto::from).collect())
        .map_err(error)
}

#[tauri::command]
pub async fn search_catalog(
    app: State<'_, SunnySongApp>,
    query: String,
    filter: String,
) -> Result<CatalogSearchResultsDto, String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(solmusic_application::CatalogSearchResults::default().into());
    }
    let filter = match filter.as_str() {
        "songs" => CatalogFilter::Songs,
        "channels" | "artists" => CatalogFilter::Artists,
        "albums" => CatalogFilter::Albums,
        "playlists" => CatalogFilter::Playlists,
        "all" => CatalogFilter::All,
        _ => return Err("invalid catalog search filter".into()),
    };
    app.search_discovery_catalog(query, filter, 24)
        .await
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub async fn get_artist_page(
    app: State<'_, SunnySongApp>,
    artist_id: String,
) -> Result<ArtistPageDto, String> {
    let artist_id = artist_id.trim();
    if artist_id.is_empty() {
        return Err("artist id cannot be blank".into());
    }
    app.artist_page(artist_id)
        .await
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub async fn get_collection_songs(
    app: State<'_, SunnySongApp>,
    collection_id: String,
) -> Result<Vec<SongDto>, String> {
    let collection_id = collection_id.trim();
    if collection_id.is_empty() {
        return Err("collection id cannot be blank".into());
    }
    app.collection_songs(collection_id)
        .await
        .map(|songs| songs.iter().map(SongDto::from).collect())
        .map_err(error)
}

#[tauri::command]
pub async fn get_song_lyrics(
    app: State<'_, SunnySongApp>,
    song_id: String,
) -> Result<Option<LyricsDto>, String> {
    let song_id = SongId::new(song_id).ok_or("song id cannot be blank")?;
    if let Some(local) = app.local_playback_file(&song_id).map_err(error)? {
        let lyrics =
            tauri::async_runtime::spawn_blocking(move || read_embedded_lyrics(&local.path))
                .await
                .map_err(error)??;
        return Ok(lyrics.map(|text| LyricsDto {
            text,
            source: "embedded".into(),
            attribution: None,
        }));
    }
    if song_id.as_str().starts_with("jellyfin:") {
        return Ok(None);
    }
    app.discovery_lyrics(&song_id)
        .await
        .map(|lyrics| {
            lyrics.map(|lyrics| LyricsDto {
                text: lyrics.text,
                source: "youtube".into(),
                attribution: lyrics.attribution,
            })
        })
        .map_err(error)
}

#[tauri::command]
pub async fn prepare_playback(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    song: SongDto,
) -> Result<PlaybackPreparationDto, String> {
    ensure_audio_runtime()?;
    let preparation = app.play_song(song.try_into()?).await.map_err(error)?;
    proxy_preparation(&media_proxy, &jellyfin, preparation, false)
}

#[tauri::command]
pub async fn warm_playback_source(
    app: State<'_, SunnySongApp>,
    song_id: String,
) -> Result<(), String> {
    let song_id = SongId::new(song_id).ok_or("song id cannot be blank")?;
    app.warm_playback_source(&song_id).await.map_err(error)
}

#[tauri::command]
pub async fn hydrate_playback_queue(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    song_id: String,
) -> Result<PlaybackStateDto, String> {
    let song_id = SongId::new(song_id).ok_or("song id cannot be blank")?;
    let mut state: PlaybackStateDto = app.hydrate_queue(&song_id).await.map_err(error)?.into();
    proxy_state_artwork(&mut state, &jellyfin, &media_proxy)?;
    Ok(state)
}

#[tauri::command]
pub async fn refill_playback_queue(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    count: usize,
) -> Result<PlaybackStateDto, String> {
    let mut state: PlaybackStateDto = app
        .refill_queue(count.clamp(1, 20))
        .await
        .map_err(error)?
        .into();
    proxy_state_artwork(&mut state, &jellyfin, &media_proxy)?;
    Ok(state)
}

fn proxy_state_artwork(
    state: &mut PlaybackStateDto,
    jellyfin: &JellyfinService,
    media_proxy: &MediaProxy,
) -> Result<(), String> {
    if let Some(current) = &mut state.current {
        proxy_artwork(&mut current.thumbnail_url, jellyfin, media_proxy)?;
    }
    for song in &mut state.queue {
        proxy_artwork(&mut song.thumbnail_url, jellyfin, media_proxy)?;
    }
    Ok(())
}

fn proxy_preparation(
    media_proxy: &MediaProxy,
    jellyfin: &JellyfinService,
    preparation: solmusic_application::PlaybackPreparation,
    force_new_remote: bool,
) -> Result<PlaybackPreparationDto, String> {
    let local_path = preparation.source.local_path.clone();
    let request_profile = preparation.source.request_profile;
    let mut dto: PlaybackPreparationDto = preparation.into();
    media_proxy.register(&mut dto, local_path, request_profile, force_new_remote)?;
    proxy_state_artwork(&mut dto.state, jellyfin, media_proxy)?;
    Ok(dto)
}

#[tauri::command]
pub async fn play_queue_item(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    song_id: String,
) -> Result<PlaybackPreparationDto, String> {
    let song_id = SongId::new(song_id).ok_or("song id cannot be blank")?;
    let preparation = app.play_queue_item(&song_id).await.map_err(error)?;
    proxy_preparation(&media_proxy, &jellyfin, preparation, false)
}

#[tauri::command]
pub async fn play_next(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
) -> Result<PlaybackPreparationDto, String> {
    let preparation = app.next().await.map_err(error)?;
    proxy_preparation(&media_proxy, &jellyfin, preparation, false)
}

#[tauri::command]
pub async fn play_previous(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
) -> Result<PlaybackPreparationDto, String> {
    let preparation = app.previous().await.map_err(error)?;
    proxy_preparation(&media_proxy, &jellyfin, preparation, false)
}

#[tauri::command]
pub async fn refresh_playback_source(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
) -> Result<PlaybackPreparationDto, String> {
    let preparation = app.refresh_current_source().await.map_err(error)?;
    proxy_preparation(&media_proxy, &jellyfin, preparation, true)
}

#[tauri::command]
pub fn get_playback_state(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
) -> Result<PlaybackStateDto, String> {
    let mut state = app.playback_state().into();
    proxy_state_artwork(&mut state, &jellyfin, &media_proxy)?;
    Ok(state)
}

#[tauri::command]
pub fn get_listening_profiles(
    app: State<'_, SunnySongApp>,
) -> Result<Vec<ListeningProfileDto>, String> {
    app.listening_profiles()
        .map(|profiles| profiles.into_iter().map(Into::into).collect())
        .map_err(error)
}

#[tauri::command]
pub fn get_active_listening_profile(
    app: State<'_, SunnySongApp>,
) -> Result<ListeningProfileDto, String> {
    app.active_listening_profile()
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub fn create_listening_profile(
    app: State<'_, SunnySongApp>,
    name: String,
) -> Result<ListeningProfileDto, String> {
    app.create_listening_profile(&name, current_time_ms()?)
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub fn rename_listening_profile(
    app: State<'_, SunnySongApp>,
    profile_id: String,
    name: String,
) -> Result<ListeningProfileDto, String> {
    app.rename_listening_profile(&profile_id, &name)
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub fn delete_listening_profile(
    app: State<'_, SunnySongApp>,
    profile_id: String,
) -> Result<(), String> {
    app.delete_listening_profile(&profile_id).map_err(error)
}

#[tauri::command]
pub fn set_active_listening_profile(
    app: State<'_, SunnySongApp>,
    profile_id: String,
) -> Result<ListeningProfileDto, String> {
    app.set_active_listening_profile(&profile_id, current_time_ms()?)
        .map(Into::into)
        .map_err(error)
}

#[tauri::command]
pub fn get_liked_song_ids(app: State<'_, SunnySongApp>) -> Result<Vec<String>, String> {
    app.liked_song_ids().map_err(error)
}

#[tauri::command]
pub fn report_playback(
    app: State<'_, SunnySongApp>,
    summary: PlaybackSummaryDto,
) -> Result<(), String> {
    app.record_playback(&summary.try_into()?).map_err(error)
}

#[tauri::command]
pub fn set_reaction(
    app: State<'_, SunnySongApp>,
    song: SongDto,
    liked: bool,
    disliked: bool,
) -> Result<(), String> {
    if liked && disliked {
        return Err("a track cannot be liked and disliked at the same time".into());
    }
    app.set_reaction(&song.try_into()?, liked, disliked)
        .map_err(error)
}

#[tauri::command]
pub fn get_recent_songs(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    count: usize,
    before: Option<i64>,
) -> Result<RecentSongsPageDto, String> {
    let count = count.clamp(1, 50);
    let mut items = app.recent_songs(count + 1, before).map_err(error)?;
    let has_more = items.len() > count;
    items.truncate(count);
    let next_cursor = items.last().map(|item| item.cursor);
    let mut songs = items
        .iter()
        .map(|item| SongDto::from(&item.song))
        .collect::<Vec<_>>();
    for song in &mut songs {
        proxy_artwork(&mut song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(RecentSongsPageDto {
        items: songs,
        next_cursor,
        has_more,
    })
}

#[tauri::command]
pub async fn get_quick_picks(
    app: State<'_, SunnySongApp>,
    jellyfin: State<'_, Arc<JellyfinService>>,
    media_proxy: State<'_, Arc<MediaProxy>>,
    count: usize,
    exclude: Vec<String>,
    options: QuickPickOptionsDto,
) -> Result<Vec<RecommendationDto>, String> {
    let now = current_time_ms()?;
    let exclude = exclude.into_iter().collect::<HashSet<_>>();
    let mut items = app
        .quick_picks(
            count.clamp(1, 20),
            now,
            &exclude,
            QuickPickOptions {
                diverse: options.diverse,
                new_songs: options.new_songs,
                rediscover: options.rediscover,
            },
        )
        .await
        .map_err(error)?
        .into_iter()
        .map(RecommendationDto::from)
        .collect::<Vec<_>>();
    for item in &mut items {
        proxy_artwork(&mut item.song.thumbnail_url, &jellyfin, &media_proxy)?;
    }
    Ok(items)
}

#[tauri::command]
pub async fn get_developer_diagnostics(
    app: State<'_, SunnySongApp>,
) -> Result<DeveloperDiagnosticsDto, String> {
    let now = current_time_ms()?;
    let database: DatabaseDiagnosticsDto = app.database_diagnostics().map_err(error)?.into();
    let player: PlaybackStateDto = app.playback_state().into();
    let recommendations: Vec<RecommendationDto> = app
        .quick_picks(20, now, &HashSet::new(), QuickPickOptions::default())
        .await
        .map_err(error)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(DeveloperDiagnosticsDto {
        generated_at_ms: now,
        active_profile: app.active_listening_profile().map_err(error)?.into(),
        database,
        player,
        recommendations,
    })
}
