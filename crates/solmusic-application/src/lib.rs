//! Platform-independent application use cases and ports.

mod app;
mod error;
mod ports;
mod recommendation;
mod status;

pub use app::{PlaybackPreparation, PlaybackState, QuickPickOptions, SunnySongApp};
pub use error::{AppError, ProviderError, StorageError};
pub use ports::{
    ArtistPage, AudioQuality, CatalogArtist, CatalogCollection, CatalogFilter,
    CatalogSearchResults, DatabaseDiagnostics, LibraryAlbum, LibraryFolder, LibraryScanResult,
    LibrarySource, LibraryTrack, ListeningProfile, LocalArtist, LocalPlaybackFile, Lyrics,
    MusicDirectory, MusicProvider, MusicRepository, PlaybackRequestProfile, PlaybackSource,
    Playlist, PlaylistTrack, RecentSong, ScannedLocalTrack,
};
pub use recommendation::{
    score_profile, score_profile_with_config, score_related_song, RecommendationConfig,
    RecommendedSong, ScoreComponent, RECOMMENDATION_POLICY_VERSION,
};
pub use solmusic_domain as domain;
pub use status::{app_status, AppStatus};
