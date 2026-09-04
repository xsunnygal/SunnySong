use serde::{Deserialize, Serialize};
use solmusic_application::{
    domain::{ArtistRef, ListeningSummary, PlaybackEndReason, Song, SongId},
    ArtistPage, CatalogArtist, CatalogCollection, CatalogSearchResults, LibraryAlbum,
    LibraryFolder, LibrarySource, LibraryTrack, ListeningProfile, PlaybackPreparation,
    PlaybackState, Playlist, PlaylistTrack, RecommendedSong,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsDto {
    pub text: String,
    pub source: String,
    pub attribution: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningProfileDto {
    pub id: String,
    pub name: String,
    pub created_at_ms: i64,
    pub last_used_at_ms: i64,
}

impl From<ListeningProfile> for ListeningProfileDto {
    fn from(value: ListeningProfile) -> Self {
        Self {
            id: value.id,
            name: value.name,
            created_at_ms: value.created_at_ms,
            last_used_at_ms: value.last_used_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistDto {
    pub id: String,
    pub name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub track_count: u64,
}

impl From<Playlist> for PlaylistDto {
    fn from(value: Playlist) -> Self {
        Self {
            id: value.id,
            name: value.name,
            created_at_ms: value.created_at_ms,
            updated_at_ms: value.updated_at_ms,
            track_count: value.track_count,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistTrackDto {
    pub song: SongDto,
    pub position: u64,
    pub added_at_ms: i64,
}

impl From<PlaylistTrack> for PlaylistTrackDto {
    fn from(value: PlaylistTrack) -> Self {
        Self {
            song: SongDto::from(&value.song),
            position: value.position,
            added_at_ms: value.added_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTrackDto {
    pub song: SongDto,
    pub source: String,
    pub source_name: String,
}

impl From<LibraryTrack> for LibraryTrackDto {
    fn from(value: LibraryTrack) -> Self {
        Self {
            song: SongDto::from(&value.song),
            source: library_source_name(value.source),
            source_name: value.source_name,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryAlbumDto {
    pub id: String,
    pub title: String,
    pub artist_name: String,
    pub thumbnail_url: Option<String>,
    pub track_count: u64,
    pub source: String,
    pub source_name: String,
}

impl From<LibraryAlbum> for LibraryAlbumDto {
    fn from(value: LibraryAlbum) -> Self {
        Self {
            id: value.id,
            title: value.title,
            artist_name: value.artist_name,
            thumbnail_url: value.thumbnail_url,
            track_count: value.track_count,
            source: library_source_name(value.source),
            source_name: value.source_name,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFolderDto {
    pub id: String,
    pub name: String,
    pub detail: String,
    pub track_count: u64,
    pub source: String,
}

impl From<LibraryFolder> for LibraryFolderDto {
    fn from(value: LibraryFolder) -> Self {
        Self {
            id: value.id,
            name: value.name,
            detail: value.detail,
            track_count: value.track_count,
            source: library_source_name(value.source),
        }
    }
}

fn library_source_name(source: LibrarySource) -> String {
    match source {
        LibrarySource::Local => "local",
        LibrarySource::Jellyfin => "jellyfin",
    }
    .into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongDto {
    pub id: String,
    pub title: String,
    pub artist_id: Option<String>,
    pub artist_name: String,
    pub album_id: Option<String>,
    pub album_name: Option<String>,
    pub duration_ms: Option<u64>,
    pub thumbnail_url: Option<String>,
}

impl From<&Song> for SongDto {
    fn from(song: &Song) -> Self {
        Self {
            id: song.id.as_str().into(),
            title: song.title.clone(),
            artist_id: song.artist.id.clone(),
            artist_name: song.artist.name.clone(),
            album_id: song.album_id.clone(),
            album_name: song.album_name.clone(),
            duration_ms: song.duration_ms,
            thumbnail_url: song.thumbnail_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogArtistDto {
    pub id: String,
    pub name: String,
    pub thumbnail_url: Option<String>,
    pub subtitle: Option<String>,
    pub source: String,
}

impl From<CatalogArtist> for CatalogArtistDto {
    fn from(value: CatalogArtist) -> Self {
        Self {
            id: value.id,
            name: value.name,
            thumbnail_url: value.thumbnail_url,
            subtitle: value.subtitle,
            source: value.source,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCollectionDto {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub thumbnail_url: Option<String>,
    pub kind: String,
}

impl From<CatalogCollection> for CatalogCollectionDto {
    fn from(value: CatalogCollection) -> Self {
        Self {
            id: value.id,
            title: value.title,
            subtitle: value.subtitle,
            thumbnail_url: value.thumbnail_url,
            kind: value.kind,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSearchResultsDto {
    pub songs: Vec<SongDto>,
    pub artists: Vec<CatalogArtistDto>,
    pub albums: Vec<CatalogCollectionDto>,
    pub playlists: Vec<CatalogCollectionDto>,
}

impl From<CatalogSearchResults> for CatalogSearchResultsDto {
    fn from(value: CatalogSearchResults) -> Self {
        Self {
            songs: value.songs.iter().map(SongDto::from).collect(),
            artists: value.artists.into_iter().map(Into::into).collect(),
            albums: value.albums.into_iter().map(Into::into).collect(),
            playlists: value.playlists.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistPageDto {
    pub artist: CatalogArtistDto,
    pub top_songs: Vec<SongDto>,
    pub songs: Vec<SongDto>,
    pub latest_releases: Vec<CatalogCollectionDto>,
    pub albums: Vec<CatalogCollectionDto>,
    pub singles: Vec<CatalogCollectionDto>,
    pub playlists: Vec<CatalogCollectionDto>,
}

impl From<ArtistPage> for ArtistPageDto {
    fn from(value: ArtistPage) -> Self {
        Self {
            artist: value.artist.into(),
            top_songs: value.top_songs.iter().map(SongDto::from).collect(),
            songs: value.songs.iter().map(SongDto::from).collect(),
            latest_releases: value.latest_releases.into_iter().map(Into::into).collect(),
            albums: value.albums.into_iter().map(Into::into).collect(),
            singles: value.singles.into_iter().map(Into::into).collect(),
            playlists: value.playlists.into_iter().map(Into::into).collect(),
        }
    }
}

impl TryFrom<SongDto> for Song {
    type Error = String;
    fn try_from(value: SongDto) -> Result<Self, Self::Error> {
        Ok(Self {
            id: SongId::new(value.id).ok_or("song id cannot be blank")?,
            title: value.title,
            artist: ArtistRef {
                id: value.artist_id,
                name: value.artist_name,
            },
            album_id: value.album_id,
            album_name: value.album_name,
            duration_ms: value.duration_ms,
            thumbnail_url: value.thumbnail_url,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSourceDto {
    pub url: String,
    pub mime_type: String,
    pub expires_at_ms: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStateDto {
    pub current: Option<SongDto>,
    pub queue: Vec<SongDto>,
    pub current_index: Option<usize>,
}

impl From<PlaybackState> for PlaybackStateDto {
    fn from(value: PlaybackState) -> Self {
        Self {
            current: value.current.as_ref().map(SongDto::from),
            queue: value.queue.iter().map(SongDto::from).collect(),
            current_index: value.current_index,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackPreparationDto {
    pub source: PlaybackSourceDto,
    pub state: PlaybackStateDto,
}

impl From<PlaybackPreparation> for PlaybackPreparationDto {
    fn from(value: PlaybackPreparation) -> Self {
        Self {
            source: PlaybackSourceDto {
                url: value.source.url,
                mime_type: value.source.mime_type,
                expires_at_ms: value.source.expires_at_ms,
            },
            state: value.state.into(),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSummaryDto {
    pub event_id: String,
    pub profile_id: String,
    pub song: SongDto,
    pub started_at_ms: i64,
    pub listened_ms: u64,
    pub duration_ms: Option<u64>,
    pub reason: String,
}

impl TryFrom<PlaybackSummaryDto> for ListeningSummary {
    type Error = String;
    fn try_from(value: PlaybackSummaryDto) -> Result<Self, Self::Error> {
        if value.event_id.trim().is_empty() {
            return Err("playback event ID cannot be blank".into());
        }
        if value.profile_id.trim().is_empty() {
            return Err("playback profile ID cannot be blank".into());
        }
        if value.started_at_ms <= 0 {
            return Err("playback start time must be positive".into());
        }
        if value.listened_ms > 24 * 60 * 60 * 1_000 {
            return Err("listened time exceeds the maximum session length".into());
        }
        if value.duration_ms == Some(0) {
            return Err("playback duration must be positive when provided".into());
        }
        let reason = match value.reason.as_str() {
            "completed" => PlaybackEndReason::Completed,
            "next" => PlaybackEndReason::Next,
            "previous" => PlaybackEndReason::Previous,
            "replaced" => PlaybackEndReason::Replaced,
            "stopped" => PlaybackEndReason::Stopped,
            "failed" => PlaybackEndReason::Failed,
            _ => return Err("invalid playback end reason".into()),
        };
        Ok(Self {
            event_id: value.event_id,
            profile_id: value.profile_id,
            song: value.song.try_into()?,
            started_at_ms: value.started_at_ms,
            listened_ms: value.listened_ms,
            duration_ms: value.duration_ms,
            reason,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseDiagnosticsDto {
    pub schema_version: usize,
    pub database_size_bytes: u64,
    pub track_count: u64,
    pub artist_count: u64,
    pub history_event_count: u64,
    pub liked_song_count: u64,
    pub integrity_status: String,
    pub query_duration_ms: u64,
}

impl From<solmusic_application::DatabaseDiagnostics> for DatabaseDiagnosticsDto {
    fn from(value: solmusic_application::DatabaseDiagnostics) -> Self {
        Self {
            schema_version: value.schema_version,
            database_size_bytes: value.database_size_bytes,
            track_count: value.track_count,
            artist_count: value.artist_count,
            history_event_count: value.history_event_count,
            liked_song_count: value.liked_song_count,
            integrity_status: value.integrity_status,
            query_duration_ms: value.query_duration_ms,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeveloperDiagnosticsDto {
    pub generated_at_ms: i64,
    pub active_profile: ListeningProfileDto,
    pub database: DatabaseDiagnosticsDto,
    pub player: PlaybackStateDto,
    pub recommendations: Vec<RecommendationDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentSongsPageDto {
    pub items: Vec<SongDto>,
    pub next_cursor: Option<i64>,
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreComponentDto {
    pub name: String,
    pub raw_value: f64,
    pub contribution: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationDto {
    pub song: SongDto,
    pub score: f64,
    pub reasons: Vec<String>,
    pub source: String,
    pub policy_version: String,
    pub components: Vec<ScoreComponentDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicDirectoryDto {
    pub id: i64,
    pub path: String,
    pub added_at_ms: i64,
    pub last_scanned_at_ms: Option<i64>,
    pub last_scan_attempt_at_ms: Option<i64>,
    pub status: String,
    pub last_error: Option<String>,
    pub track_count: u64,
}

impl From<solmusic_application::MusicDirectory> for MusicDirectoryDto {
    fn from(value: solmusic_application::MusicDirectory) -> Self {
        Self {
            id: value.id,
            path: value.path,
            added_at_ms: value.added_at_ms,
            last_scanned_at_ms: value.last_scanned_at_ms,
            last_scan_attempt_at_ms: value.last_scan_attempt_at_ms,
            status: value.status,
            last_error: value.last_error,
            track_count: value.track_count,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalArtistDto {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub track_count: u64,
}

impl From<solmusic_application::LocalArtist> for LocalArtistDto {
    fn from(value: solmusic_application::LocalArtist) -> Self {
        Self {
            id: value.id,
            name: value.name,
            enabled: value.enabled,
            track_count: value.track_count,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryScanResultDto {
    pub directory_id: i64,
    pub status: String,
    pub indexed_tracks: usize,
    pub unavailable_tracks: usize,
    pub skipped_files: usize,
    pub duration_ms: u64,
    pub error: Option<String>,
}

impl From<solmusic_application::LibraryScanResult> for LibraryScanResultDto {
    fn from(value: solmusic_application::LibraryScanResult) -> Self {
        Self {
            directory_id: value.directory_id,
            status: value.status,
            indexed_tracks: value.indexed_tracks,
            unavailable_tracks: value.unavailable_tracks,
            skipped_files: value.skipped_files,
            duration_ms: value.duration_ms,
            error: value.error,
        }
    }
}

impl From<RecommendedSong> for RecommendationDto {
    fn from(value: RecommendedSong) -> Self {
        Self {
            song: SongDto::from(&value.profile.song),
            score: value.score,
            reasons: value.reasons,
            source: value.source.into(),
            policy_version: value.policy_version.into(),
            components: value
                .components
                .into_iter()
                .map(|component| ScoreComponentDto {
                    name: component.name.into(),
                    raw_value: component.raw_value,
                    contribution: component.contribution,
                })
                .collect(),
        }
    }
}
