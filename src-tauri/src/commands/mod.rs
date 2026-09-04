mod download;
mod jellyfin;
mod library;
mod music;
mod playlist;
mod system;
mod youtube;

pub use download::{download_song, pick_android_download_directory};
pub use jellyfin::{
    begin_jellyfin_quick_connect, connect_jellyfin_password, finish_jellyfin_quick_connect,
    get_jellyfin_servers, refresh_jellyfin_libraries, remove_jellyfin_server,
    set_jellyfin_library_enabled, validate_jellyfin_server,
};
pub use library::{
    add_music_directory, get_discovery_enabled, get_library_album_tracks, get_library_albums,
    get_library_folders, get_library_tracks, get_local_artists, get_music_directories,
    remove_music_directory, rescan_music_library, search_local_music, set_discovery_enabled,
    set_local_artist_enabled,
};
pub use music::{
    create_listening_profile, delete_listening_profile, get_active_listening_profile,
    get_artist_page, get_collection_songs, get_developer_diagnostics, get_liked_song_ids,
    get_listening_profiles, get_playback_state, get_quick_picks, get_recent_songs, get_song_lyrics,
    hydrate_playback_queue, play_next, play_previous, play_queue_item, prepare_playback,
    refill_playback_queue, refresh_playback_source, rename_listening_profile, report_playback,
    search_catalog, search_music, set_active_listening_profile, set_audio_quality, set_reaction,
    sync_media_session, warm_playback_source,
};
pub use playlist::{add_song_to_playlist, create_playlist, get_playlist_tracks, get_playlists};
pub use system::{background_app, get_app_status};
pub use youtube::{clear_youtube_auth, get_youtube_auth_status, set_youtube_cookies};
