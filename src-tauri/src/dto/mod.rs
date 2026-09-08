use serde::{Deserialize, Serialize};
use solmusic_application::{
    domain::{ArtistRef, ListeningSummary, PlaybackEndReason, Song, SongId},
    ArtistPage, CatalogArtist, CatalogCollection, CatalogSearchResults, DiscoverSection,
    DownloadRecord, HistoryEvent, LibraryAlbum, LibraryFolder, LibrarySource, LibraryTrack,
    ListeningProfile, ListeningRecap, PlaybackPreparation, PlaybackSource, PlaybackState, Playlist,
    PlaylistTrack, RecommendedSong, TimedLyricsLine,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedLyricsLineDto {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub text: String,
}

impl From<TimedLyricsLine> for TimedLyricsLineDto {
    fn from(value: TimedLyricsLine) -> Self {
        Self {
            start_ms: value.start_ms,
            end_ms: value.end_ms,
            text: value.text,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsDto {
    pub text: String,
    pub lines: Vec<TimedLyricsLineDto>,
    pub synchronized: bool,
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
pub struct NormalizationGainMetadataDto {
    pub track_gain_db: Option<f64>,
    pub album_gain_db: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_peak: Option<f64>,
}

impl From<solmusic_application::NormalizationGainMetadata> for NormalizationGainMetadataDto {
    fn from(value: solmusic_application::NormalizationGainMetadata) -> Self {
        Self {
            track_gain_db: value.track_gain_db,
            album_gain_db: value.album_gain_db,
            track_peak: value.track_peak,
            album_peak: value.album_peak,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSourceDto {
    pub url: String,
    pub mime_type: String,
    pub expires_at_ms: Option<i64>,
    pub normalization_gain_metadata: Option<NormalizationGainMetadataDto>,
}

impl From<PlaybackSource> for PlaybackSourceDto {
    fn from(value: PlaybackSource) -> Self {
        Self {
            url: value.url,
            mime_type: value.mime_type,
            expires_at_ms: value.expires_at_ms,
            normalization_gain_metadata: value.normalization_gain_metadata.map(Into::into),
        }
    }
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
            source: value.source.into(),
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
pub struct SongsPageDto {
    pub items: Vec<SongDto>,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEventDto {
    pub sequence_id: i64,
    pub event_id: String,
    pub song: SongDto,
    pub started_at_ms: i64,
    pub listened_ms: u64,
    pub duration_ms: Option<u64>,
    pub reason: String,
}

impl From<HistoryEvent> for HistoryEventDto {
    fn from(value: HistoryEvent) -> Self {
        Self {
            sequence_id: value.sequence_id,
            event_id: value.event_id,
            song: SongDto::from(&value.song),
            started_at_ms: value.started_at_ms,
            listened_ms: value.listened_ms,
            duration_ms: value.duration_ms,
            reason: value.reason,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPageDto {
    pub items: Vec<HistoryEventDto>,
    pub next_cursor: Option<i64>,
    pub has_more: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningRecapDto {
    pub total_listened_ms: u64,
    pub plays: u64,
    pub completions: u64,
    pub skips: u64,
    pub unique_songs: u64,
    pub unique_artists: u64,
    pub top_songs: Vec<RecapSongDto>,
    pub top_artists: Vec<RecapArtistDto>,
    pub coverage: RecapCoverageDto,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecapSongDto {
    pub song: SongDto,
    pub listened_ms: u64,
    pub plays: u64,
    pub completions: u64,
    pub skips: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecapArtistDto {
    pub artist_id: Option<String>,
    pub artist_name: String,
    pub listened_ms: u64,
    pub plays: u64,
    pub completions: u64,
    pub skips: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecapCoverageDto {
    pub requested_from_ms: Option<i64>,
    pub requested_to_ms: Option<i64>,
    pub available_from_ms: Option<i64>,
    pub complete_from_ms: i64,
    pub complete: bool,
    pub note: String,
}

impl From<ListeningRecap> for ListeningRecapDto {
    fn from(value: ListeningRecap) -> Self {
        Self {
            total_listened_ms: value.total_listened_ms,
            plays: value.plays,
            completions: value.completions,
            skips: value.skips,
            unique_songs: value.unique_songs,
            unique_artists: value.unique_artists,
            top_songs: value
                .top_songs
                .into_iter()
                .map(|item| RecapSongDto {
                    song: SongDto::from(&item.song),
                    listened_ms: item.listened_ms,
                    plays: item.plays,
                    completions: item.completions,
                    skips: item.skips,
                })
                .collect(),
            top_artists: value
                .top_artists
                .into_iter()
                .map(|item| RecapArtistDto {
                    artist_id: item.artist_id,
                    artist_name: item.artist_name,
                    listened_ms: item.listened_ms,
                    plays: item.plays,
                    completions: item.completions,
                    skips: item.skips,
                })
                .collect(),
            coverage: RecapCoverageDto {
                requested_from_ms: value.coverage.requested_from_ms,
                requested_to_ms: value.coverage.requested_to_ms,
                available_from_ms: value.coverage.available_from_ms,
                complete_from_ms: value.coverage.complete_from_ms,
                complete: value.coverage.complete,
                note: value.coverage.note,
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecordDto {
    pub id: String,
    pub song: SongDto,
    pub status: String,
    pub location: Option<String>,
    pub file_name: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub completed_at_ms: Option<i64>,
}
impl From<DownloadRecord> for DownloadRecordDto {
    fn from(value: DownloadRecord) -> Self {
        Self {
            id: value.id,
            song: SongDto::from(&value.song),
            status: value.status,
            location: value.location,
            file_name: value.file_name,
            error: value.error,
            created_at_ms: value.created_at_ms,
            completed_at_ms: value.completed_at_ms,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverSectionDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub items: Vec<RecommendationDto>,
}
impl From<DiscoverSection> for DiscoverSectionDto {
    fn from(value: DiscoverSection) -> Self {
        Self {
            id: value.id,
            title: value.title,
            description: value.description,
            items: value.items.into_iter().map(Into::into).collect(),
        }
    }
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
    pub unchanged_tracks: usize,
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
            unchanged_tracks: value.unchanged_tracks,
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
