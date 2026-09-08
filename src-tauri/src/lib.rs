mod commands;
mod dto;
mod jellyfin;
mod library_provider;
mod local_library;
mod media_proxy;
#[cfg(desktop)]
mod music_provider;
#[cfg(desktop)]
mod playback_resolver;
mod youtube_auth;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use commands::{
    add_music_directory, add_song_to_playlist, apply_pending_restore, background_app,
    begin_jellyfin_quick_connect, clear_history, clear_upcoming, clear_youtube_auth,
    connect_jellyfin_password, create_listening_profile, create_playlist, delete_history_event,
    delete_listening_profile, delete_playlist, delete_song_history, download_song, enqueue_next,
    enqueue_song, export_backup, export_playlist_m3u, finish_jellyfin_quick_connect,
    get_active_listening_profile, get_app_status, get_artist_page, get_collection_songs,
    get_developer_diagnostics, get_discover_feed, get_discovery_enabled, get_downloads,
    get_history_events, get_jellyfin_servers, get_library_album_tracks, get_library_albums,
    get_library_folders, get_library_tracks, get_liked_song_ids, get_liked_songs,
    get_listening_profiles, get_listening_recap, get_local_artists, get_music_directories,
    get_playback_state, get_playlist_tracks, get_playlists, get_quick_picks, get_recent_songs,
    get_search_suggestions, get_song_lyrics, get_youtube_auth_status, hydrate_playback_queue,
    import_playlist_m3u, move_queue_item, pick_android_download_directory, play_next,
    play_previous, play_queue_item, prepare_next_playback_source, prepare_playback,
    refill_playback_queue, refresh_jellyfin_libraries, refresh_playback_source, remove_download,
    remove_jellyfin_server, remove_music_directory, remove_queue_item, remove_song_from_playlist,
    rename_listening_profile, rename_playlist, reorder_playlist_tracks, replace_queue,
    report_playback, rescan_music_library, search_catalog, search_local_music, search_music,
    set_active_listening_profile, set_audio_quality, set_discovery_enabled,
    set_jellyfin_library_enabled, set_local_artist_enabled, set_reaction, set_youtube_cookies,
    stage_backup_restore, sync_media_session, validate_jellyfin_server, warm_playback_source,
    AppPaths,
};
use solmusic_application::{MusicProvider, SunnySongApp};

use jellyfin::JellyfinService;
use library_provider::LibraryMusicProvider;
use local_library::LocalArtworkStore;
use media_proxy::MediaProxy;
#[cfg(desktop)]
use music_provider::DesktopMusicProvider;
#[cfg(desktop)]
use playback_resolver::PlaybackResolver;
use solmusic_sqlite::SqliteMusicRepository;
use solmusic_youtube::YouTubeMusicProvider;
use tauri::Manager;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use youtube_auth::YouTubeAuthService;

fn init_logging(log_dir: &Path) -> Result<tracing_appender::non_blocking::WorkerGuard, String> {
    std::fs::create_dir_all(log_dir)
        .map_err(|error| format!("could not create diagnostic log directory: {error}"))?;
    let appender = tracing_appender::rolling::daily(log_dir, "solmusic.jsonl");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let default_level = if cfg!(debug_assertions) {
        "warn,solmusic=debug,solmusic_application=debug,solmusic_sqlite=debug,solmusic_youtube=debug"
    } else {
        "warn,solmusic=info,solmusic_application=info,solmusic_sqlite=info,solmusic_youtube=info"
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_current_span(true)
        .with_span_list(true)
        .try_init()
        .map_err(|error| format!("could not initialize diagnostic logging: {error}"))?;
    Ok(guard)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_solmusic_storage::init())
        .setup(|app| {
            let data_dir = std::env::var_os("SUNNYSONG_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&data_dir)?;
            let log_guard = init_logging(&data_dir.join("logs"))?;
            app.manage(log_guard);
            info!(category = "STARTUP", event = "application_setup_started");
            let paths = AppPaths::new(data_dir.clone());
            if apply_pending_restore(&paths)? {
                info!(category = "DATABASE", event = "staged_restore_applied");
            }
            let database_path = paths.database.clone();
            app.manage(paths);
            let repository = match SqliteMusicRepository::open(&database_path) {
                Ok(repository) => Arc::new(repository),
                Err(error) => {
                    error!(category = "DATABASE", event = "database_open_failed", path = %database_path.display(), reason = %error);
                    return Err(error.to_string().into());
                }
            };
            let jellyfin = Arc::new(JellyfinService::open(&database_path)?);
            let youtube_auth = Arc::new(YouTubeAuthService::load(app.handle()));
            let youtube_auth_state = youtube_auth.state();
            #[cfg(desktop)]
            let discovery_provider: Arc<dyn MusicProvider> = {
                let playback_resolver = Arc::new(PlaybackResolver::new(
                    app.handle(),
                    youtube_auth_state.clone(),
                )?);
                Arc::new(DesktopMusicProvider::new(
                    YouTubeMusicProvider::with_auth(youtube_auth_state.clone())
                        .map_err(|error| error.to_string())?,
                    playback_resolver,
                ))
            };
            #[cfg(mobile)]
            let discovery_provider: Arc<dyn MusicProvider> = Arc::new(
                YouTubeMusicProvider::with_auth(youtube_auth_state)
                    .map_err(|error| error.to_string())?,
            );
            let provider = Arc::new(LibraryMusicProvider::new(
                discovery_provider,
                jellyfin.clone(),
            ));
            app.manage(SunnySongApp::new(provider, repository));
            app.manage(jellyfin);
            app.manage(youtube_auth);
            let artwork_cache = data_dir.join("artwork-cache");
            app.manage(LocalArtworkStore::new(artwork_cache.clone())?);
            app.manage(MediaProxy::start(data_dir.join("media-cache"), artwork_cache)?);
            info!(category = "STARTUP", event = "application_setup_completed");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_status,
            background_app,
            get_discovery_enabled,
            set_discovery_enabled,
            get_youtube_auth_status,
            set_youtube_cookies,
            clear_youtube_auth,
            validate_jellyfin_server,
            connect_jellyfin_password,
            begin_jellyfin_quick_connect,
            finish_jellyfin_quick_connect,
            get_jellyfin_servers,
            refresh_jellyfin_libraries,
            set_jellyfin_library_enabled,
            remove_jellyfin_server,
            get_music_directories,
            add_music_directory,
            remove_music_directory,
            rescan_music_library,
            get_local_artists,
            set_local_artist_enabled,
            get_library_tracks,
            get_library_albums,
            get_library_album_tracks,
            get_library_folders,
            get_playlists,
            create_playlist,
            get_playlist_tracks,
            export_playlist_m3u,
            import_playlist_m3u,
            add_song_to_playlist,
            rename_playlist,
            delete_playlist,
            remove_song_from_playlist,
            reorder_playlist_tracks,
            search_local_music,
            search_music,
            search_catalog,
            set_audio_quality,
            download_song,
            get_downloads,
            remove_download,
            pick_android_download_directory,
            get_artist_page,
            get_collection_songs,
            get_song_lyrics,
            prepare_playback,
            prepare_next_playback_source,
            warm_playback_source,
            hydrate_playback_queue,
            refill_playback_queue,
            enqueue_next,
            enqueue_song,
            remove_queue_item,
            move_queue_item,
            clear_upcoming,
            replace_queue,
            play_next,
            play_previous,
            play_queue_item,
            refresh_playback_source,
            get_playback_state,
            get_listening_profiles,
            get_active_listening_profile,
            create_listening_profile,
            rename_listening_profile,
            delete_listening_profile,
            set_active_listening_profile,
            get_liked_song_ids,
            get_liked_songs,
            get_history_events,
            delete_history_event,
            delete_song_history,
            clear_history,
            get_listening_recap,
            get_search_suggestions,
            get_discover_feed,
            report_playback,
            set_reaction,
            sync_media_session,
            get_recent_songs,
            get_quick_picks,
            get_developer_diagnostics,
            export_backup,
            stage_backup_restore
        ])
        .run(tauri::generate_context!())
        .expect("failed to run SunnySong");
}
