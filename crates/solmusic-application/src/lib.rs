//! Platform-independent application use cases and ports.

mod app;
mod error;
mod ports;
mod recommendation;
mod status;

pub use app::{
    DiscoverSection, PlaybackPreparation, PlaybackState, QuickPickOptions, SunnySongApp,
};
pub use error::{AppError, ProviderError, StorageError};
pub use ports::{
    ArtistPage, AudioQuality, CatalogArtist, CatalogCollection, CatalogFilter,
    CatalogSearchResults, DatabaseDiagnostics, DownloadRecord, HistoryEvent, LibraryAlbum,
    LibraryFolder, LibraryScanResult, LibrarySource, LibraryTrack, ListeningProfile,
    ListeningRecap, LocalArtist, LocalPlaybackFile, Lyrics, MusicDirectory, MusicProvider,
    MusicRepository, PlaybackRequestProfile, PlaybackSource, Playlist, PlaylistExportTrack,
    PlaylistItemSource, PlaylistTrack, RecapArtist, RecapCoverage, RecapSong, RecentSong,
    ScannedLocalTrack, TimedLyricsLine,
};
pub use recommendation::{
    score_profile, score_profile_with_config, score_related_song, RecommendationConfig,
    RecommendedSong, ScoreComponent, RECOMMENDATION_POLICY_VERSION,
};
pub use solmusic_domain as domain;
pub use solmusic_domain::NormalizationGainMetadata;
pub use status::{app_status, AppStatus};
