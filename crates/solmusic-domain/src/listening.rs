use crate::Song;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackEndReason {
    Completed,
    Next,
    Previous,
    Replaced,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListeningSummary {
    pub event_id: String,
    pub profile_id: String,
    pub song: Song,
    pub started_at_ms: i64,
    pub listened_ms: u64,
    pub duration_ms: Option<u64>,
    pub reason: PlaybackEndReason,
}

impl ListeningSummary {
    pub fn completion(&self) -> Option<f64> {
        let duration = self.duration_ms?;
        (duration > 0).then(|| (self.listened_ms as f64 / duration as f64).clamp(0.0, 1.0))
    }

    pub fn is_early_skip(&self) -> bool {
        matches!(
            self.reason,
            PlaybackEndReason::Next | PlaybackEndReason::Previous
        ) && self.listened_ms < 20_000
    }

    pub fn is_meaningful(&self) -> bool {
        self.listened_ms >= 30_000 || self.completion().is_some_and(|value| value >= 0.2)
    }

    pub fn is_completed(&self) -> bool {
        matches!(self.reason, PlaybackEndReason::Completed)
            || self.completion().is_some_and(|value| value >= 0.8)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackProfile {
    pub song: Song,
    pub play_count: u64,
    pub completed_count: u64,
    pub early_skip_count: u64,
    pub completion_ema: f64,
    pub affinity: f64,
    pub liked: bool,
    pub disliked: bool,
    pub last_played_at_ms: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::{ListeningSummary, PlaybackEndReason};
    use crate::{ArtistRef, Song, SongId};

    fn summary(listened_ms: u64, duration_ms: u64, reason: PlaybackEndReason) -> ListeningSummary {
        ListeningSummary {
            event_id: "event".into(),
            profile_id: "profile".into(),
            song: Song {
                id: SongId::new("song").unwrap(),
                title: "Song".into(),
                artist: ArtistRef {
                    id: None,
                    name: "Artist".into(),
                },
                album_id: None,
                album_name: None,
                duration_ms: Some(duration_ms),
                thumbnail_url: None,
            },
            started_at_ms: 0,
            listened_ms,
            duration_ms: Some(duration_ms),
            reason,
        }
    }

    #[test]
    fn classifies_playback_signals() {
        let early = summary(9_000, 180_000, PlaybackEndReason::Next);
        assert!(early.is_early_skip());
        assert!(!early.is_meaningful());

        let complete = summary(150_000, 180_000, PlaybackEndReason::Next);
        assert!(complete.is_completed());
        assert_eq!(complete.completion(), Some(150_000.0 / 180_000.0));
    }
}
