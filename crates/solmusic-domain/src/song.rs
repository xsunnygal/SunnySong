#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SongId(String);

impl SongId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRef {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NormalizationGainMetadata {
    pub track_gain_db: Option<f64>,
    pub album_gain_db: Option<f64>,
    pub track_peak: Option<f64>,
    pub album_peak: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Song {
    pub id: SongId,
    pub title: String,
    pub artist: ArtistRef,
    pub album_id: Option<String>,
    pub album_name: Option<String>,
    pub duration_ms: Option<u64>,
    pub thumbnail_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::SongId;

    #[test]
    fn song_id_rejects_blank_values() {
        assert!(SongId::new("  ").is_none());
        assert_eq!(SongId::new("abc123").unwrap().as_str(), "abc123");
    }
}
