use std::sync::Arc;

use async_trait::async_trait;
use solmusic_application::{
    domain::{Song, SongId},
    ArtistPage, AudioQuality, CatalogFilter, CatalogSearchResults, Lyrics, MusicProvider,
    PlaybackSource, ProviderError,
};

use crate::jellyfin::JellyfinService;

pub struct LibraryMusicProvider {
    discovery: Arc<dyn MusicProvider>,
    jellyfin: Arc<JellyfinService>,
}

impl LibraryMusicProvider {
    pub fn new(discovery: Arc<dyn MusicProvider>, jellyfin: Arc<JellyfinService>) -> Self {
        Self {
            discovery,
            jellyfin,
        }
    }
}

#[async_trait]
impl MusicProvider for LibraryMusicProvider {
    fn set_audio_quality(&self, quality: AudioQuality) {
        self.discovery.set_audio_quality(quality);
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<Song>, ProviderError> {
        self.discovery.search(query, limit).await
    }

    async fn search_catalog(
        &self,
        query: &str,
        filter: CatalogFilter,
        limit: usize,
    ) -> Result<CatalogSearchResults, ProviderError> {
        self.discovery.search_catalog(query, filter, limit).await
    }

    async fn artist_page(&self, artist_id: &str) -> Result<ArtistPage, ProviderError> {
        self.discovery.artist_page(artist_id).await
    }

    async fn collection_songs(&self, collection_id: &str) -> Result<Vec<Song>, ProviderError> {
        self.discovery.collection_songs(collection_id).await
    }

    async fn related(&self, song_id: &SongId, limit: usize) -> Result<Vec<Song>, ProviderError> {
        if song_id.as_str().starts_with("jellyfin:") {
            Ok(Vec::new())
        } else {
            self.discovery.related(song_id, limit).await
        }
    }

    async fn lyrics(&self, song_id: &SongId) -> Result<Option<Lyrics>, ProviderError> {
        if song_id.as_str().starts_with("jellyfin:") {
            Ok(None)
        } else {
            self.discovery.lyrics(song_id).await
        }
    }

    async fn resolve_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        if song_id.as_str().starts_with("jellyfin:") {
            self.jellyfin
                .resolve_playback(song_id)
                .map_err(ProviderError::Network)
        } else {
            self.discovery.resolve_playback(song_id).await
        }
    }

    async fn refresh_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        if song_id.as_str().starts_with("jellyfin:") {
            self.jellyfin
                .resolve_playback(song_id)
                .map_err(ProviderError::Network)
        } else {
            self.discovery.refresh_playback(song_id).await
        }
    }
}
