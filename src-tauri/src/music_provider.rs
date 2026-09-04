use std::sync::Arc;

use async_trait::async_trait;
use solmusic_application::{
    domain::{Song, SongId},
    ArtistPage, AudioQuality, CatalogFilter, CatalogSearchResults, Lyrics, MusicProvider,
    PlaybackSource, ProviderError,
};
use solmusic_youtube::YouTubeMusicProvider;

use crate::playback_resolver::PlaybackResolver;

pub struct DesktopMusicProvider {
    metadata: YouTubeMusicProvider,
    playback: Arc<PlaybackResolver>,
}

impl DesktopMusicProvider {
    pub fn new(metadata: YouTubeMusicProvider, playback: Arc<PlaybackResolver>) -> Self {
        Self { metadata, playback }
    }
}

#[async_trait]
impl MusicProvider for DesktopMusicProvider {
    fn set_audio_quality(&self, quality: AudioQuality) {
        self.metadata.set_audio_quality(quality);
        self.playback.set_audio_quality(quality);
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<Song>, ProviderError> {
        self.metadata.search(query, limit).await
    }

    async fn search_catalog(
        &self,
        query: &str,
        filter: CatalogFilter,
        limit: usize,
    ) -> Result<CatalogSearchResults, ProviderError> {
        self.metadata.search_catalog(query, filter, limit).await
    }

    async fn artist_page(&self, artist_id: &str) -> Result<ArtistPage, ProviderError> {
        self.metadata.artist_page(artist_id).await
    }

    async fn collection_songs(&self, collection_id: &str) -> Result<Vec<Song>, ProviderError> {
        self.metadata.collection_songs(collection_id).await
    }

    async fn related(&self, song_id: &SongId, limit: usize) -> Result<Vec<Song>, ProviderError> {
        self.metadata.related(song_id, limit).await
    }

    async fn lyrics(&self, song_id: &SongId) -> Result<Option<Lyrics>, ProviderError> {
        self.metadata.lyrics(song_id).await
    }

    async fn resolve_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        self.playback.resolve(song_id).await
    }

    async fn refresh_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        self.playback.resolve_fresh(song_id).await
    }
}
