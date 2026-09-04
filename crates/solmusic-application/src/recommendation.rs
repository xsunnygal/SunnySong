use solmusic_domain::TrackProfile;

pub const RECOMMENDATION_POLICY_VERSION: &str = "quick-picks-v3";

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecommendationConfig {
    pub affinity_weight: f64,
    pub liked_bonus: f64,
    pub completion_weight: f64,
    pub play_count_weight: f64,
    pub early_skip_weight: f64,
    pub maximum_early_skip_penalty_count: u64,
    pub rediscovery_bonus: f64,
}

impl Default for RecommendationConfig {
    fn default() -> Self {
        Self {
            affinity_weight: 0.30,
            liked_bonus: 8.0,
            completion_weight: 2.0,
            play_count_weight: 0.75,
            early_skip_weight: 0.40,
            maximum_early_skip_penalty_count: 10,
            rediscovery_bonus: 1.5,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreComponent {
    pub name: &'static str,
    pub raw_value: f64,
    pub contribution: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecommendedSong {
    pub profile: TrackProfile,
    pub score: f64,
    pub reasons: Vec<String>,
    pub source: &'static str,
    pub policy_version: &'static str,
    pub components: Vec<ScoreComponent>,
}

pub fn score_profile(profile: TrackProfile, now_ms: i64) -> RecommendedSong {
    score_profile_with_config(profile, now_ms, RecommendationConfig::default())
}

pub fn score_related_song(
    song: solmusic_domain::Song,
    seed: &RecommendedSong,
    related_rank: usize,
) -> RecommendedSong {
    let seed_preference = seed.score.max(0.0).ln_1p() * 1.5;
    let rank_bonus = 2.0 / (related_rank as f64 + 1.0).sqrt();
    let exploration_bonus = 1.0;
    let score = seed_preference + rank_bonus + exploration_bonus;
    let mut reasons = vec![format!("Related to {}", seed.profile.song.title)];
    if seed.profile.liked {
        reasons.push("Based on a liked track".into());
    } else if seed.profile.completion_ema >= 0.8 || seed.profile.play_count >= 2 {
        reasons.push("Based on strong listening history".into());
    }

    RecommendedSong {
        profile: TrackProfile {
            song,
            play_count: 0,
            completed_count: 0,
            early_skip_count: 0,
            completion_ema: 0.0,
            affinity: 0.0,
            liked: false,
            disliked: false,
            last_played_at_ms: None,
        },
        score,
        reasons,
        source: "related_to_preference",
        policy_version: RECOMMENDATION_POLICY_VERSION,
        components: vec![
            ScoreComponent {
                name: "seed_preference",
                raw_value: seed.score,
                contribution: seed_preference,
            },
            ScoreComponent {
                name: "related_rank",
                raw_value: related_rank as f64,
                contribution: rank_bonus,
            },
            ScoreComponent {
                name: "exploration",
                raw_value: 1.0,
                contribution: exploration_bonus,
            },
        ],
    }
}

pub fn score_profile_with_config(
    profile: TrackProfile,
    now_ms: i64,
    config: RecommendationConfig,
) -> RecommendedSong {
    if profile.disliked {
        return RecommendedSong {
            profile,
            score: f64::NEG_INFINITY,
            reasons: vec!["Explicitly disliked".into()],
            source: "local_profile",
            policy_version: RECOMMENDATION_POLICY_VERSION,
            components: vec![ScoreComponent {
                name: "explicit_dislike_filter",
                raw_value: 1.0,
                contribution: f64::NEG_INFINITY,
            }],
        };
    }

    let mut score = 0.0;
    let mut reasons = Vec::new();
    let mut components = Vec::new();

    let affinity = profile.affinity * config.affinity_weight;
    score += affinity;
    components.push(ScoreComponent {
        name: "track_affinity",
        raw_value: profile.affinity,
        contribution: affinity,
    });

    if profile.liked {
        score += config.liked_bonus;
        reasons.push("Liked track".into());
        components.push(ScoreComponent {
            name: "explicit_like",
            raw_value: 1.0,
            contribution: config.liked_bonus,
        });
    }

    let completion = profile.completion_ema.clamp(0.0, 1.0) * config.completion_weight;
    score += completion;
    components.push(ScoreComponent {
        name: "completion",
        raw_value: profile.completion_ema,
        contribution: completion,
    });
    if profile.completion_ema >= 0.8 {
        reasons.push("Strong completion history".into());
    }

    let play_count = (profile.play_count as f64 + 1.0).log2() * config.play_count_weight;
    score += play_count;
    components.push(ScoreComponent {
        name: "play_count",
        raw_value: profile.play_count as f64,
        contribution: play_count,
    });

    if let Some(last_played) = profile.last_played_at_ms {
        let age_hours = ((now_ms - last_played).max(0) as f64) / 3_600_000.0;
        let repetition_multiplier = if age_hours < 1.0 {
            0.05
        } else if age_hours < 6.0 {
            0.2
        } else if age_hours < 24.0 {
            0.5
        } else if age_hours < 72.0 {
            0.8
        } else {
            1.0
        };
        let before_repetition = score;
        score *= repetition_multiplier;
        components.push(ScoreComponent {
            name: "recent_repetition",
            raw_value: age_hours,
            contribution: score - before_repetition,
        });
        if age_hours >= 24.0 * 30.0 && profile.affinity > 2.0 {
            score += config.rediscovery_bonus;
            reasons.push("Rediscovery".into());
            components.push(ScoreComponent {
                name: "rediscovery",
                raw_value: age_hours,
                contribution: config.rediscovery_bonus,
            });
        }
    }

    let bounded_early_skips = profile
        .early_skip_count
        .min(config.maximum_early_skip_penalty_count);
    let early_skip_penalty = -(bounded_early_skips as f64 * config.early_skip_weight);
    score += early_skip_penalty;
    components.push(ScoreComponent {
        name: "early_skips",
        raw_value: profile.early_skip_count as f64,
        contribution: early_skip_penalty,
    });

    RecommendedSong {
        profile,
        score,
        reasons,
        source: "local_profile",
        policy_version: RECOMMENDATION_POLICY_VERSION,
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        score_profile, score_profile_with_config, score_related_song, RecommendationConfig,
    };
    use solmusic_domain::{ArtistRef, Song, SongId, TrackProfile};

    fn profile() -> TrackProfile {
        TrackProfile {
            song: Song {
                id: SongId::new("a").unwrap(),
                title: "A".into(),
                artist: ArtistRef {
                    id: None,
                    name: "Artist".into(),
                },
                album_id: None,
                album_name: None,
                duration_ms: None,
                thumbnail_url: None,
            },
            play_count: 4,
            completed_count: 3,
            early_skip_count: 0,
            completion_ema: 0.9,
            affinity: 4.0,
            liked: true,
            disliked: false,
            last_played_at_ms: Some(0),
        }
    }

    #[test]
    fn dislikes_are_excluded_with_diagnostic_reason() {
        let mut value = profile();
        value.disliked = true;
        let scored = score_profile(value, 1_000);
        assert_eq!(scored.score, f64::NEG_INFINITY);
        assert_eq!(scored.components[0].name, "explicit_dislike_filter");
    }

    #[test]
    fn recent_repetition_reduces_score() {
        let recent = score_profile(profile(), 1_000).score;
        let old = score_profile(profile(), 40 * 24 * 3_600_000).score;
        assert!(old > recent);
    }

    #[test]
    fn early_skip_penalty_is_bounded() {
        let mut ten_skips = profile();
        ten_skips.early_skip_count = 10;
        let mut thousand_skips = ten_skips.clone();
        thousand_skips.early_skip_count = 1_000;
        let now = 40 * 24 * 3_600_000;
        assert_eq!(
            score_profile(ten_skips, now).score,
            score_profile(thousand_skips, now).score
        );
    }

    #[test]
    fn weights_are_centralized_and_tunable() {
        let default = score_profile(profile(), 1_000).score;
        let without_like = score_profile_with_config(
            profile(),
            1_000,
            RecommendationConfig {
                liked_bonus: 0.0,
                ..RecommendationConfig::default()
            },
        )
        .score;
        assert!(default > without_like);
    }

    #[test]
    fn related_song_inherits_preference_signal_without_fake_history() {
        let seed = score_profile(profile(), 40 * 24 * 3_600_000);
        let recommendation = score_related_song(
            Song {
                id: SongId::new("new-song").unwrap(),
                title: "New Song".into(),
                artist: ArtistRef {
                    id: None,
                    name: "New Artist".into(),
                },
                album_id: None,
                album_name: None,
                duration_ms: None,
                thumbnail_url: None,
            },
            &seed,
            0,
        );
        assert_eq!(recommendation.source, "related_to_preference");
        assert_eq!(recommendation.profile.play_count, 0);
        assert!(!recommendation.profile.liked);
        assert!(recommendation.score > 0.0);
        assert!(recommendation
            .reasons
            .iter()
            .any(|reason| reason == "Based on a liked track"));
    }
}
