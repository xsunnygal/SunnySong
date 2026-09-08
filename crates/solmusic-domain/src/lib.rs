//! Stable, platform-independent music concepts.

mod listening;
mod song;

pub use listening::{ListeningSummary, PlaybackEndReason, TrackProfile};
pub use song::{ArtistRef, NormalizationGainMetadata, Song, SongId};
