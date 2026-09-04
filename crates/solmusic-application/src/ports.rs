use std::{collections::HashMap, path::PathBuf};

use async_trait::async_trait;
use solmusic_domain::{ListeningSummary, Song, SongId, TrackProfile};

use crate::{ProviderError, StorageError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioQuality {
    Low,
    Medium,
    #[default]
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackRequestProfile {
    #[default]
    Web,
    VisionOs,
    AndroidVr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackSource {
    pub url: String,
    pub mime_type: String,
    pub expires_at_ms: Option<i64>,
    pub local_path: Option<PathBuf>,
    pub request_profile: PlaybackRequestProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseDiagnostics {
    pub schema_version: usize,
    pub database_size_bytes: u64,
    pub track_count: u64,
    pub artist_count: u64,
    pub history_event_count: u64,
    pub liked_song_count: u64,
    pub integrity_status: String,
    pub query_duration_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogFilter {
    All,
    Songs,
    Artists,
    Albums,
    Playlists,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogArtist {
    pub id: String,
    pub name: String,
    pub thumbnail_url: Option<String>,
    pub subtitle: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogCollection {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail_url: Option<String>,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogSearchResults {
    pub songs: Vec<Song>,
    pub artists: Vec<CatalogArtist>,
    pub albums: Vec<CatalogCollection>,
    pub playlists: Vec<CatalogCollection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistPage {
    pub artist: CatalogArtist,
    pub top_songs: Vec<Song>,
    pub songs: Vec<Song>,
    pub latest_releases: Vec<CatalogCollection>,
    pub albums: Vec<CatalogCollection>,
    pub singles: Vec<CatalogCollection>,
    pub playlists: Vec<CatalogCollection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSong {
    pub song: Song,
    pub cursor: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListeningProfile {
    pub id: String,
    pub name: String,
    pub created_at_ms: i64,
    pub last_used_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub track_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistTrack {
    pub song: Song,
    pub position: u64,
    pub added_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicDirectory {
    pub id: i64,
    pub path: String,
    pub added_at_ms: i64,
    pub last_scanned_at_ms: Option<i64>,
    pub last_scan_attempt_at_ms: Option<i64>,
    pub status: String,
    pub last_error: Option<String>,
    pub track_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalArtist {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub track_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedLocalTrack {
    pub song: Song,
    pub canonical_path: String,
    pub relative_path: String,
    pub mime_type: String,
    pub file_size_bytes: u64,
    pub modified_at_ms: i64,
    pub source_identity: Option<String>,
    pub artist_names: Vec<String>,
    pub first_seen_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPlaybackFile {
    pub path: PathBuf,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lyrics {
    pub text: String,
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySource {
    Local,
    Jellyfin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryTrack {
    pub song: Song,
    pub source: LibrarySource,
    pub source_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryAlbum {
    pub id: String,
    pub title: String,
    pub artist_name: String,
    pub thumbnail_url: Option<String>,
    pub track_count: u64,
    pub source: LibrarySource,
    pub source_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFolder {
    pub id: String,
    pub name: String,
    pub detail: String,
    pub track_count: u64,
    pub source: LibrarySource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryScanResult {
    pub directory_id: i64,
    pub status: String,
    pub indexed_tracks: usize,
    pub unavailable_tracks: usize,
    pub skipped_files: usize,
    pub duration_ms: u64,
    pub error: Option<String>,
}

#[async_trait]
pub trait MusicProvider: Send + Sync {
    fn set_audio_quality(&self, _quality: AudioQuality) {}

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<Song>, ProviderError>;
    async fn search_catalog(
        &self,
        query: &str,
        filter: CatalogFilter,
        limit: usize,
    ) -> Result<CatalogSearchResults, ProviderError> {
        let songs = if matches!(filter, CatalogFilter::All | CatalogFilter::Songs) {
            self.search(query, limit).await?
        } else {
            Vec::new()
        };
        Ok(CatalogSearchResults {
            songs,
            ..CatalogSearchResults::default()
        })
    }
    async fn artist_page(&self, _artist_id: &str) -> Result<ArtistPage, ProviderError> {
        Err(ProviderError::Incompatible(
            "artist browsing is unavailable".into(),
        ))
    }
    async fn collection_songs(&self, _collection_id: &str) -> Result<Vec<Song>, ProviderError> {
        Err(ProviderError::Incompatible(
            "collection browsing is unavailable".into(),
        ))
    }
    async fn related(&self, song_id: &SongId, limit: usize) -> Result<Vec<Song>, ProviderError>;
    async fn lyrics(&self, _song_id: &SongId) -> Result<Option<Lyrics>, ProviderError> {
        Ok(None)
    }
    async fn resolve_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError>;
    async fn refresh_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        self.resolve_playback(song_id).await
    }
}

pub trait MusicRepository: Send + Sync {
    fn save_songs(&self, songs: &[Song]) -> Result<(), StorageError>;
    fn record_playback(&self, summary: &ListeningSummary) -> Result<(), StorageError>;
    fn set_reaction(&self, song: &Song, liked: bool, disliked: bool) -> Result<(), StorageError>;
    fn recent_songs(
        &self,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<RecentSong>, StorageError>;
    fn track_profiles(&self, limit: usize) -> Result<Vec<TrackProfile>, StorageError>;
    fn database_diagnostics(&self) -> Result<DatabaseDiagnostics, StorageError>;

    fn listening_profiles(&self) -> Result<Vec<ListeningProfile>, StorageError> {
        Ok(vec![self.active_listening_profile()?])
    }
    fn active_listening_profile(&self) -> Result<ListeningProfile, StorageError> {
        Ok(ListeningProfile {
            id: "main".into(),
            name: "Main".into(),
            created_at_ms: 0,
            last_used_at_ms: 0,
        })
    }
    fn create_listening_profile(
        &self,
        _name: &str,
        _now_ms: i64,
    ) -> Result<ListeningProfile, StorageError> {
        Err(StorageError("listening profiles are unavailable".into()))
    }
    fn rename_listening_profile(
        &self,
        _profile_id: &str,
        _name: &str,
    ) -> Result<ListeningProfile, StorageError> {
        Err(StorageError("listening profiles are unavailable".into()))
    }
    fn delete_listening_profile(&self, _profile_id: &str) -> Result<(), StorageError> {
        Err(StorageError("listening profiles are unavailable".into()))
    }
    fn set_active_listening_profile(
        &self,
        _profile_id: &str,
        _now_ms: i64,
    ) -> Result<ListeningProfile, StorageError> {
        Err(StorageError("listening profiles are unavailable".into()))
    }
    fn playlists(&self) -> Result<Vec<Playlist>, StorageError> {
        Ok(Vec::new())
    }
    fn create_playlist(&self, _name: &str, _now_ms: i64) -> Result<Playlist, StorageError> {
        Err(StorageError("playlists are unavailable".into()))
    }
    fn playlist_tracks(&self, _playlist_id: &str) -> Result<Vec<PlaylistTrack>, StorageError> {
        Ok(Vec::new())
    }
    fn add_song_to_playlist(
        &self,
        _playlist_id: &str,
        _song: &Song,
        _now_ms: i64,
    ) -> Result<PlaylistTrack, StorageError> {
        Err(StorageError("playlists are unavailable".into()))
    }
    fn liked_song_ids(&self) -> Result<Vec<String>, StorageError> {
        Ok(Vec::new())
    }
    fn artist_affinities(&self) -> Result<HashMap<String, f64>, StorageError> {
        Ok(HashMap::new())
    }

    fn discovery_enabled(&self) -> Result<bool, StorageError> {
        Ok(false)
    }
    fn set_discovery_enabled(&self, _enabled: bool, _now_ms: i64) -> Result<(), StorageError> {
        Err(StorageError("discovery settings are unavailable".into()))
    }
    fn music_directories(&self) -> Result<Vec<MusicDirectory>, StorageError> {
        Ok(Vec::new())
    }
    fn add_music_directory(
        &self,
        _path: &str,
        _now_ms: i64,
    ) -> Result<MusicDirectory, StorageError> {
        Err(StorageError("local library is unavailable".into()))
    }
    fn remove_music_directory(&self, _directory_id: i64) -> Result<(), StorageError> {
        Err(StorageError("local library is unavailable".into()))
    }
    fn replace_directory_scan(
        &self,
        _directory_id: i64,
        _tracks: &[ScannedLocalTrack],
        _scanned_at_ms: i64,
        _skipped_files: usize,
        _complete: bool,
        _duration_ms: u64,
    ) -> Result<LibraryScanResult, StorageError> {
        Err(StorageError("local library is unavailable".into()))
    }
    fn set_music_directory_status(
        &self,
        _directory_id: i64,
        _status: &str,
        _error: Option<&str>,
        _attempted_at_ms: i64,
    ) -> Result<(), StorageError> {
        Err(StorageError("local library is unavailable".into()))
    }
    fn local_artists(
        &self,
        _query: &str,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<LocalArtist>, StorageError> {
        Ok(Vec::new())
    }
    fn set_local_artist_enabled(
        &self,
        _artist_id: i64,
        _enabled: bool,
    ) -> Result<(), StorageError> {
        Err(StorageError("local library is unavailable".into()))
    }
    fn search_local(&self, _query: &str, _limit: usize) -> Result<Vec<Song>, StorageError> {
        Ok(Vec::new())
    }
    fn library_tracks(
        &self,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<LibraryTrack>, StorageError> {
        Ok(Vec::new())
    }
    fn library_albums(
        &self,
        _limit: usize,
        _offset: usize,
    ) -> Result<Vec<LibraryAlbum>, StorageError> {
        Ok(Vec::new())
    }
    fn library_album_tracks(&self, _album_id: &str) -> Result<Vec<LibraryTrack>, StorageError> {
        Ok(Vec::new())
    }
    fn library_folders(&self) -> Result<Vec<LibraryFolder>, StorageError> {
        Ok(Vec::new())
    }
    fn has_indexed_source(&self, _song_id: &SongId) -> Result<bool, StorageError> {
        Ok(false)
    }
    fn local_track_profiles(&self, _limit: usize) -> Result<Vec<TrackProfile>, StorageError> {
        Ok(Vec::new())
    }
    fn local_playback_file(
        &self,
        _song_id: &SongId,
    ) -> Result<Option<LocalPlaybackFile>, StorageError> {
        Ok(None)
    }
}
