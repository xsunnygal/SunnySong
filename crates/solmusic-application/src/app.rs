use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use futures::future::join_all;
use solmusic_domain::{ListeningSummary, Song};
use tracing::{debug, info, warn};

use crate::{
    score_profile, score_related_song, AppError, ArtistPage, AudioQuality, CatalogFilter,
    CatalogSearchResults, LibraryAlbum, LibraryFolder, LibraryScanResult, LibraryTrack,
    LocalArtist, MusicDirectory, MusicProvider, MusicRepository, PlaybackRequestProfile,
    PlaybackSource, RecommendedSong, ScannedLocalTrack,
};

#[derive(Debug, Clone, Default)]
pub struct PlaybackState {
    pub current: Option<Song>,
    pub queue: Vec<Song>,
    pub current_index: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct QuickPickOptions {
    pub diverse: bool,
    pub new_songs: bool,
    pub rediscover: bool,
}

impl Default for QuickPickOptions {
    fn default() -> Self {
        Self {
            diverse: true,
            new_songs: true,
            rediscover: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlaybackPreparation {
    pub source: PlaybackSource,
    pub state: PlaybackState,
}

pub struct SunnySongApp {
    provider: Arc<dyn MusicProvider>,
    repository: Arc<dyn MusicRepository>,
    playback: Mutex<PlaybackState>,
    playback_generation: AtomicU64,
    discovery_generation: AtomicU64,
    playback_source_cache: Mutex<HashMap<String, PlaybackSource>>,
}

impl SunnySongApp {
    pub fn new(provider: Arc<dyn MusicProvider>, repository: Arc<dyn MusicRepository>) -> Self {
        Self {
            provider,
            repository,
            playback: Mutex::new(PlaybackState::default()),
            playback_generation: AtomicU64::new(0),
            discovery_generation: AtomicU64::new(0),
            playback_source_cache: Mutex::new(HashMap::new()),
        }
    }

    async fn search_provider(&self, query: &str, limit: usize) -> Result<Vec<Song>, AppError> {
        let started = Instant::now();
        let songs = self.provider.search(query, limit).await?;
        info!(
            category = "YOUTUBE",
            event = "search_completed",
            result_count = songs.len(),
            duration_ms = started.elapsed().as_millis() as u64
        );
        Ok(songs)
    }

    pub fn set_audio_quality(&self, quality: AudioQuality) {
        self.provider.set_audio_quality(quality);
    }

    pub fn local_playback_file(
        &self,
        song_id: &crate::domain::SongId,
    ) -> Result<Option<crate::LocalPlaybackFile>, AppError> {
        Ok(self.repository.local_playback_file(song_id)?)
    }

    pub async fn discovery_lyrics(
        &self,
        song_id: &crate::domain::SongId,
    ) -> Result<Option<crate::Lyrics>, AppError> {
        if !self.repository.discovery_enabled()? {
            return Ok(None);
        }
        Ok(self.provider.lyrics(song_id).await?)
    }

    pub async fn download_source(&self, song: &Song) -> Result<PlaybackSource, AppError> {
        if let Some(local) = self.repository.local_playback_file(&song.id)? {
            return Ok(PlaybackSource {
                url: String::new(),
                mime_type: local.mime_type,
                expires_at_ms: None,
                local_path: Some(local.path),
                request_profile: PlaybackRequestProfile::Web,
            });
        }
        if song.id.as_str().starts_with("jellyfin:") {
            return Ok(self.provider.refresh_playback(&song.id).await?);
        }
        if !self.repository.discovery_enabled()? {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        Ok(self.provider.refresh_playback(&song.id).await?)
    }

    pub async fn play_song(&self, song: Song) -> Result<PlaybackPreparation, AppError> {
        let generation = self.playback_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let source = self.resolve_source(&song.id, false).await?;
        if self.playback_generation.load(Ordering::SeqCst) != generation {
            debug!(
                category = "PLAYER",
                event = "stale_playback_selection_discarded",
                track_id = song.id.as_str()
            );
            return Err(AppError::StalePlaybackOperation);
        }
        self.repository.save_songs(std::slice::from_ref(&song))?;
        let state = PlaybackState {
            current: Some(song.clone()),
            queue: vec![song.clone()],
            current_index: Some(0),
        };
        *self.playback.lock().expect("playback state poisoned") = state.clone();
        info!(
            category = "PLAYER",
            event = "track_prepared",
            track_id = song.id.as_str(),
            generation
        );
        Ok(PlaybackPreparation { source, state })
    }

    pub async fn hydrate_queue(
        &self,
        song_id: &solmusic_domain::SongId,
    ) -> Result<PlaybackState, AppError> {
        let started = Instant::now();
        let indexed_track = self.repository.has_indexed_source(song_id)?;
        let discovery_enabled = self.repository.discovery_enabled()?;
        let candidates = if indexed_track || !discovery_enabled {
            self.repository
                .local_track_profiles(40)?
                .into_iter()
                .map(|profile| profile.song)
                .collect::<Vec<_>>()
        } else {
            let generation = self.discovery_generation.load(Ordering::SeqCst);
            let related = self.provider.related(song_id, 20).await?;
            if generation != self.discovery_generation.load(Ordering::SeqCst)
                || !self.repository.discovery_enabled()?
            {
                Vec::new()
            } else {
                self.repository.save_songs(&related)?;
                related
            }
        };
        let candidate_count = candidates.len();
        let recent = self.repository.recent_songs(20, None)?;
        let candidates = rank_queue_candidates(candidates, &recent);

        let mut state = self.playback.lock().expect("playback state poisoned");
        let Some(current) = state.current.clone() else {
            return Ok(state.clone());
        };
        if &current.id != song_id {
            return Ok(state.clone());
        }

        let mut seen = HashSet::from([current.id.as_str().to_owned()]);
        let mut queue = vec![current];
        queue.extend(
            candidates
                .into_iter()
                .filter(|candidate| seen.insert(candidate.id.as_str().to_owned()))
                .take(20),
        );
        state.queue = queue;
        state.current_index = Some(0);
        info!(
            category = "QUEUE",
            event = "next_up_hydrated",
            track_id = song_id.as_str(),
            candidate_count,
            queue_size = state.queue.len(),
            indexed_library = indexed_track || !discovery_enabled,
            duration_ms = started.elapsed().as_millis() as u64
        );
        Ok(state.clone())
    }

    pub async fn refill_queue(&self, count: usize) -> Result<PlaybackState, AppError> {
        if count == 0 {
            return Ok(self.playback_state());
        }
        let snapshot = self.playback_state();
        let Some(current) = snapshot.current.clone() else {
            return Ok(snapshot);
        };
        let current_index = snapshot.current_index.ok_or(AppError::EmptyQueue)?;

        let indexed_track = self.repository.has_indexed_source(&current.id)?;
        let discovery_enabled = self.repository.discovery_enabled()?;
        let candidates = if indexed_track || !discovery_enabled {
            self.repository
                .local_track_profiles(100)?
                .into_iter()
                .map(|profile| profile.song)
                .collect::<Vec<_>>()
        } else {
            let generation = self.discovery_generation.load(Ordering::SeqCst);
            let related = self.provider.related(&current.id, 40).await?;
            if generation != self.discovery_generation.load(Ordering::SeqCst)
                || !self.repository.discovery_enabled()?
            {
                Vec::new()
            } else {
                self.repository.save_songs(&related)?;
                related
            }
        };
        let recent = self.repository.recent_songs(20, None)?;
        let candidates = rank_queue_candidates(candidates, &recent);

        let mut state = self.playback.lock().expect("playback state poisoned");
        if state.current.as_ref().map(|song| &song.id) != Some(&current.id)
            || state.current_index != Some(current_index)
        {
            return Ok(state.clone());
        }
        let mut seen = state
            .queue
            .iter()
            .map(|song| song.id.as_str().to_owned())
            .collect::<HashSet<_>>();
        let additions = candidates
            .into_iter()
            .filter(|candidate| seen.insert(candidate.id.as_str().to_owned()))
            .take(count)
            .collect::<Vec<_>>();
        let added = additions.len();
        state.queue.extend(additions);
        info!(
            category = "QUEUE",
            event = "next_up_refilled",
            track_id = current.id.as_str(),
            added,
            queue_size = state.queue.len(),
            recent_context_count = recent.len()
        );
        Ok(state.clone())
    }

    pub async fn play_queue_item(
        &self,
        song_id: &solmusic_domain::SongId,
    ) -> Result<PlaybackPreparation, AppError> {
        let generation = self.playback_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (song, target_index, original_index, original_song_id) = {
            let state = self.playback.lock().expect("playback state poisoned");
            let target_index = state
                .queue
                .iter()
                .position(|song| &song.id == song_id)
                .ok_or(AppError::EmptyQueue)?;
            let song = state.queue[target_index].clone();
            let original_song_id = state
                .current
                .as_ref()
                .map(|current| current.id.as_str().to_owned());
            (song, target_index, state.current_index, original_song_id)
        };
        let source = self.resolve_source(&song.id, false).await?;
        if self.playback_generation.load(Ordering::SeqCst) != generation {
            return Err(AppError::StalePlaybackOperation);
        }
        let mut state = self.playback.lock().expect("playback state poisoned");
        let current_song_id = state
            .current
            .as_ref()
            .map(|current| current.id.as_str().to_owned());
        if state.current_index != original_index
            || current_song_id != original_song_id
            || state.queue.get(target_index).map(|item| &item.id) != Some(song_id)
        {
            return Err(AppError::StalePlaybackOperation);
        }
        state.current_index = Some(target_index);
        state.current = Some(song);
        let state = state.clone();
        info!(
            category = "QUEUE",
            event = "queue_item_selected",
            track_id = song_id.as_str(),
            queue_index = target_index,
            queue_size = state.queue.len()
        );
        Ok(PlaybackPreparation { source, state })
    }

    pub async fn next(&self) -> Result<PlaybackPreparation, AppError> {
        self.move_in_queue(1).await
    }

    pub async fn previous(&self) -> Result<PlaybackPreparation, AppError> {
        self.move_in_queue(-1).await
    }

    async fn move_in_queue(&self, delta: isize) -> Result<PlaybackPreparation, AppError> {
        let generation = self.playback_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (song, original_index, target_index, original_song_id) = {
            let state = self.playback.lock().expect("playback state poisoned");
            let current = state.current_index.ok_or(AppError::EmptyQueue)?;
            let target = current as isize + delta;
            if target < 0 || target >= state.queue.len() as isize {
                debug!(
                    category = "QUEUE",
                    event = "queue_boundary_reached",
                    current_index = current,
                    queue_size = state.queue.len(),
                    direction = delta
                );
                return Err(AppError::QueueBoundary);
            }
            let song = state
                .queue
                .get(target as usize)
                .cloned()
                .ok_or(AppError::EmptyQueue)?;
            let original_song_id = state
                .current
                .as_ref()
                .map(|current| current.id.as_str().to_owned());
            (song, current, target as usize, original_song_id)
        };

        let source = self.resolve_source(&song.id, false).await?;
        if self.playback_generation.load(Ordering::SeqCst) != generation {
            warn!(
                category = "QUEUE",
                event = "stale_queue_transition_discarded",
                generation
            );
            return Err(AppError::StalePlaybackOperation);
        }

        let mut state = self.playback.lock().expect("playback state poisoned");
        let current_song_id = state
            .current
            .as_ref()
            .map(|current| current.id.as_str().to_owned());
        if state.current_index != Some(original_index) || current_song_id != original_song_id {
            warn!(
                category = "QUEUE",
                event = "queue_changed_during_resolution",
                generation
            );
            return Err(AppError::StalePlaybackOperation);
        }
        state.current_index = Some(target_index);
        state.current = Some(song.clone());
        info!(
            category = "QUEUE",
            event = "queue_transition_committed",
            track_id = song.id.as_str(),
            queue_index = target_index,
            queue_size = state.queue.len()
        );
        let state = state.clone();
        Ok(PlaybackPreparation { source, state })
    }

    pub async fn warm_playback_source(
        &self,
        song_id: &solmusic_domain::SongId,
    ) -> Result<(), AppError> {
        let started = Instant::now();
        self.resolve_source(song_id, false).await?;
        debug!(
            category = "PLAYER",
            event = "playback_source_warmed",
            track_id = song_id.as_str(),
            duration_ms = started.elapsed().as_millis() as u64
        );
        Ok(())
    }

    pub async fn refresh_current_source(&self) -> Result<PlaybackPreparation, AppError> {
        let state = self.playback_state();
        let song = state.current.as_ref().ok_or(AppError::EmptyQueue)?;
        let source = self.resolve_source(&song.id, true).await?;
        Ok(PlaybackPreparation { source, state })
    }

    async fn resolve_source(
        &self,
        song_id: &solmusic_domain::SongId,
        force_refresh: bool,
    ) -> Result<PlaybackSource, AppError> {
        if let Some(local) = self.repository.local_playback_file(song_id)? {
            return Ok(PlaybackSource {
                url: String::new(),
                mime_type: local.mime_type,
                expires_at_ms: None,
                local_path: Some(local.path),
                request_profile: PlaybackRequestProfile::Web,
            });
        }
        let is_jellyfin = song_id.as_str().starts_with("jellyfin:");
        if !is_jellyfin && !self.repository.discovery_enabled()? {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        if !force_refresh {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            if let Some(source) = self
                .playback_source_cache
                .lock()
                .expect("playback source cache poisoned")
                .get(song_id.as_str())
                .filter(|source| {
                    source
                        .expires_at_ms
                        .is_none_or(|expires| expires > now_ms + 60_000)
                })
                .cloned()
            {
                return Ok(source);
            }
        }
        if is_jellyfin {
            let source = if force_refresh {
                self.provider.refresh_playback(song_id).await?
            } else {
                self.provider.resolve_playback(song_id).await?
            };
            self.cache_playback_source(song_id, &source);
            return Ok(source);
        }
        let generation = self.discovery_generation.load(Ordering::SeqCst);
        let source = if force_refresh {
            self.provider.refresh_playback(song_id).await?
        } else {
            self.provider.resolve_playback(song_id).await?
        };
        if generation != self.discovery_generation.load(Ordering::SeqCst)
            || !self.repository.discovery_enabled()?
        {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        self.cache_playback_source(song_id, &source);
        Ok(source)
    }

    fn cache_playback_source(&self, song_id: &solmusic_domain::SongId, source: &PlaybackSource) {
        let mut cache = self
            .playback_source_cache
            .lock()
            .expect("playback source cache poisoned");
        if cache.len() >= 32 && !cache.contains_key(song_id.as_str()) {
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
            }
        }
        cache.insert(song_id.as_str().to_owned(), source.clone());
    }

    pub fn discovery_enabled(&self) -> Result<bool, AppError> {
        Ok(self.repository.discovery_enabled()?)
    }

    pub fn set_discovery_enabled(&self, enabled: bool, now_ms: i64) -> Result<(), AppError> {
        self.repository.set_discovery_enabled(enabled, now_ms)?;
        self.discovery_generation.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub fn music_directories(&self) -> Result<Vec<MusicDirectory>, AppError> {
        Ok(self.repository.music_directories()?)
    }

    pub fn add_music_directory(&self, path: &str, now_ms: i64) -> Result<MusicDirectory, AppError> {
        Ok(self.repository.add_music_directory(path, now_ms)?)
    }

    pub fn remove_music_directory(&self, directory_id: i64) -> Result<(), AppError> {
        Ok(self.repository.remove_music_directory(directory_id)?)
    }

    pub fn library_tracks(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LibraryTrack>, AppError> {
        Ok(self.repository.library_tracks(limit, offset)?)
    }

    pub fn library_albums(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LibraryAlbum>, AppError> {
        Ok(self.repository.library_albums(limit, offset)?)
    }

    pub fn library_album_tracks(&self, album_id: &str) -> Result<Vec<LibraryTrack>, AppError> {
        Ok(self.repository.library_album_tracks(album_id)?)
    }

    pub fn library_folders(&self) -> Result<Vec<LibraryFolder>, AppError> {
        Ok(self.repository.library_folders()?)
    }

    pub fn store_directory_scan(
        &self,
        directory_id: i64,
        tracks: &[ScannedLocalTrack],
        scanned_at_ms: i64,
        skipped_files: usize,
        complete: bool,
        duration_ms: u64,
    ) -> Result<LibraryScanResult, AppError> {
        Ok(self.repository.replace_directory_scan(
            directory_id,
            tracks,
            scanned_at_ms,
            skipped_files,
            complete,
            duration_ms,
        )?)
    }

    pub fn set_music_directory_status(
        &self,
        directory_id: i64,
        status: &str,
        error: Option<&str>,
        attempted_at_ms: i64,
    ) -> Result<(), AppError> {
        Ok(self.repository.set_music_directory_status(
            directory_id,
            status,
            error,
            attempted_at_ms,
        )?)
    }

    pub fn local_artists(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LocalArtist>, AppError> {
        Ok(self.repository.local_artists(query, limit, offset)?)
    }

    pub fn set_local_artist_enabled(&self, artist_id: i64, enabled: bool) -> Result<(), AppError> {
        Ok(self
            .repository
            .set_local_artist_enabled(artist_id, enabled)?)
    }

    pub fn search_local(&self, query: &str, limit: usize) -> Result<Vec<Song>, AppError> {
        Ok(self.repository.search_local(query, limit)?)
    }

    pub async fn search_discovery(&self, query: &str, limit: usize) -> Result<Vec<Song>, AppError> {
        if !self.repository.discovery_enabled()? {
            return Ok(Vec::new());
        }
        let generation = self.discovery_generation.load(Ordering::SeqCst);
        let songs = self.search_provider(query, limit).await?;
        if generation != self.discovery_generation.load(Ordering::SeqCst)
            || !self.repository.discovery_enabled()?
        {
            return Ok(Vec::new());
        }
        self.repository.save_songs(&songs)?;
        Ok(songs)
    }

    pub async fn search_discovery_catalog(
        &self,
        query: &str,
        filter: CatalogFilter,
        limit: usize,
    ) -> Result<CatalogSearchResults, AppError> {
        if !self.repository.discovery_enabled()? {
            return Ok(CatalogSearchResults::default());
        }
        let generation = self.discovery_generation.load(Ordering::SeqCst);
        let results = self.provider.search_catalog(query, filter, limit).await?;
        if generation != self.discovery_generation.load(Ordering::SeqCst)
            || !self.repository.discovery_enabled()?
        {
            return Ok(CatalogSearchResults::default());
        }
        self.repository.save_songs(&results.songs)?;
        Ok(results)
    }

    pub async fn artist_page(&self, artist_id: &str) -> Result<ArtistPage, AppError> {
        if !self.repository.discovery_enabled()? {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        let generation = self.discovery_generation.load(Ordering::SeqCst);
        let page = self.provider.artist_page(artist_id).await?;
        if generation != self.discovery_generation.load(Ordering::SeqCst)
            || !self.repository.discovery_enabled()?
        {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        let songs = page
            .top_songs
            .iter()
            .chain(page.songs.iter())
            .cloned()
            .collect::<Vec<_>>();
        self.repository.save_songs(&songs)?;
        Ok(page)
    }

    pub async fn collection_songs(&self, collection_id: &str) -> Result<Vec<Song>, AppError> {
        if !self.repository.discovery_enabled()? {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        let generation = self.discovery_generation.load(Ordering::SeqCst);
        let songs = self.provider.collection_songs(collection_id).await?;
        if generation != self.discovery_generation.load(Ordering::SeqCst)
            || !self.repository.discovery_enabled()?
        {
            return Err(AppError::Provider(crate::ProviderError::Unavailable));
        }
        self.repository.save_songs(&songs)?;
        Ok(songs)
    }

    pub fn playback_state(&self) -> PlaybackState {
        self.playback
            .lock()
            .expect("playback state poisoned")
            .clone()
    }

    pub fn record_playback(&self, summary: &ListeningSummary) -> Result<(), AppError> {
        let started = Instant::now();
        self.repository.record_playback(summary)?;
        info!(category = "HISTORY", event = "playback_summary_recorded", profile_id = summary.profile_id, track_id = summary.song.id.as_str(), listened_ms = summary.listened_ms, end_reason = ?summary.reason, duration_ms = started.elapsed().as_millis() as u64);
        Ok(())
    }

    pub fn set_reaction(&self, song: &Song, liked: bool, disliked: bool) -> Result<(), AppError> {
        self.repository.set_reaction(song, liked, disliked)?;
        Ok(())
    }

    pub fn database_diagnostics(&self) -> Result<crate::DatabaseDiagnostics, AppError> {
        Ok(self.repository.database_diagnostics()?)
    }

    pub fn listening_profiles(&self) -> Result<Vec<crate::ListeningProfile>, AppError> {
        Ok(self.repository.listening_profiles()?)
    }

    pub fn active_listening_profile(&self) -> Result<crate::ListeningProfile, AppError> {
        Ok(self.repository.active_listening_profile()?)
    }

    pub fn create_listening_profile(
        &self,
        name: &str,
        now_ms: i64,
    ) -> Result<crate::ListeningProfile, AppError> {
        Ok(self.repository.create_listening_profile(name, now_ms)?)
    }

    pub fn rename_listening_profile(
        &self,
        profile_id: &str,
        name: &str,
    ) -> Result<crate::ListeningProfile, AppError> {
        Ok(self.repository.rename_listening_profile(profile_id, name)?)
    }

    pub fn delete_listening_profile(&self, profile_id: &str) -> Result<(), AppError> {
        self.repository.delete_listening_profile(profile_id)?;
        Ok(())
    }

    pub fn set_active_listening_profile(
        &self,
        profile_id: &str,
        now_ms: i64,
    ) -> Result<crate::ListeningProfile, AppError> {
        let profile = self
            .repository
            .set_active_listening_profile(profile_id, now_ms)?;
        info!(
            category = "PROFILE",
            event = "active_profile_changed",
            profile_id = profile.id,
            profile_name = profile.name
        );
        Ok(profile)
    }

    pub fn playlists(&self) -> Result<Vec<crate::Playlist>, AppError> {
        Ok(self.repository.playlists()?)
    }

    pub fn create_playlist(&self, name: &str, now_ms: i64) -> Result<crate::Playlist, AppError> {
        Ok(self.repository.create_playlist(name, now_ms)?)
    }

    pub fn playlist_tracks(
        &self,
        playlist_id: &str,
    ) -> Result<Vec<crate::PlaylistTrack>, AppError> {
        Ok(self.repository.playlist_tracks(playlist_id)?)
    }

    pub fn add_song_to_playlist(
        &self,
        playlist_id: &str,
        song: &Song,
        now_ms: i64,
    ) -> Result<crate::PlaylistTrack, AppError> {
        Ok(self
            .repository
            .add_song_to_playlist(playlist_id, song, now_ms)?)
    }

    pub fn liked_song_ids(&self) -> Result<Vec<String>, AppError> {
        Ok(self.repository.liked_song_ids()?)
    }

    pub fn recent_songs(
        &self,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<crate::RecentSong>, AppError> {
        Ok(self.repository.recent_songs(limit, before)?)
    }

    pub async fn quick_picks(
        &self,
        limit: usize,
        now_ms: i64,
        exclude: &HashSet<String>,
        options: QuickPickOptions,
    ) -> Result<Vec<RecommendedSong>, AppError> {
        let started = Instant::now();
        let discovery_enabled = self.repository.discovery_enabled()?;
        let discovery_generation = self.discovery_generation.load(Ordering::SeqCst);
        let artist_affinities = self.repository.artist_affinities()?;
        let local_profiles = self.repository.local_track_profiles(200)?;
        let local_ids = local_profiles
            .iter()
            .map(|profile| profile.song.id.as_str().to_owned())
            .collect::<HashSet<_>>();
        let all_profiles = if discovery_enabled {
            self.repository.track_profiles(400)?
        } else {
            local_profiles.clone()
        };
        let known_ids = all_profiles
            .iter()
            .map(|profile| profile.song.id.as_str().to_owned())
            .collect::<HashSet<_>>();
        let queue_ids = self
            .playback_state()
            .queue
            .into_iter()
            .map(|song| song.id.as_str().to_owned())
            .collect::<HashSet<_>>();

        let mut local_candidates = local_profiles
            .into_iter()
            .map(|profile| {
                let underplayed = profile.play_count == 0;
                let mut candidate = score_profile(profile, now_ms);
                apply_artist_affinity(&mut candidate, &artist_affinities);
                if underplayed {
                    if options.new_songs {
                        candidate.score += 1.5;
                    }
                    candidate.source = "local_underplayed";
                    candidate
                        .reasons
                        .push("Unheard from your local library".into());
                    candidate.components.push(crate::ScoreComponent {
                        name: "local_underplayed",
                        raw_value: 1.0,
                        contribution: if options.new_songs { 1.5 } else { 0.0 },
                    });
                } else if candidate.profile.liked || candidate.profile.affinity >= 3.0 {
                    candidate.source = "local_favorite";
                } else {
                    candidate.source = "local_rediscovery";
                }
                candidate
            })
            .filter(|candidate| options.rediscover || candidate.source != "local_rediscovery")
            .filter(|item| item.score.is_finite())
            .collect::<Vec<_>>();
        local_candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
        let eligible_local_count = local_candidates
            .iter()
            .filter(|candidate| {
                let id = candidate.profile.song.id.as_str();
                !exclude.contains(id) && !queue_ids.contains(id)
            })
            .count();
        let initial_discovery_target =
            online_discovery_target(limit, eligible_local_count, discovery_enabled);
        let seed_limit = initial_discovery_target.min(2);

        let seeds = if discovery_enabled && seed_limit > 0 {
            let mut remote_candidates = all_profiles
                .into_iter()
                .filter(|profile| !local_ids.contains(profile.song.id.as_str()))
                .map(|profile| {
                    let mut candidate = score_profile(profile, now_ms);
                    apply_artist_affinity(&mut candidate, &artist_affinities);
                    candidate
                })
                .filter(|candidate| candidate.score.is_finite() && candidate.score > 0.0)
                .collect::<Vec<_>>();
            remote_candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
            remote_candidates
                .into_iter()
                .take(seed_limit)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let related_results = join_all(
            seeds
                .iter()
                .map(|seed| self.provider.related(&seed.profile.song.id, 12)),
        )
        .await;

        let mut discovered = HashMap::<String, RecommendedSong>::new();
        for (seed, result) in seeds.iter().zip(related_results) {
            let songs = match result {
                Ok(songs) => songs,
                Err(error) => {
                    warn!(category = "RECOMMENDER", event = "related_seed_failed", seed_track_id = seed.profile.song.id.as_str(), reason = %error);
                    continue;
                }
            };
            for (rank, song) in songs.into_iter().enumerate() {
                let id = song.id.as_str().to_owned();
                if known_ids.contains(&id) || exclude.contains(&id) || queue_ids.contains(&id) {
                    continue;
                }
                let recommendation = score_related_song(song, seed, rank);
                match discovered.entry(id) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(recommendation);
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        if recommendation.score > entry.get().score {
                            entry.insert(recommendation);
                        }
                    }
                }
            }
        }

        let discovery_still_enabled = discovery_enabled
            && discovery_generation == self.discovery_generation.load(Ordering::SeqCst)
            && self.repository.discovery_enabled()?;
        if !discovery_still_enabled {
            discovered.clear();
        }
        let discovered_songs = discovered
            .values()
            .map(|candidate| candidate.profile.song.clone())
            .collect::<Vec<_>>();
        if !discovered_songs.is_empty() {
            self.repository.save_songs(&discovered_songs)?;
        }

        let mut discovery_candidates = discovered.into_values().collect::<Vec<_>>();
        discovery_candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
        let discovery_target =
            online_discovery_target(limit, eligible_local_count, discovery_still_enabled);
        if options.new_songs {
            for candidate in &mut discovery_candidates {
                candidate.score += 1.5;
                candidate
                    .reasons
                    .push("New to your listening history".into());
            }
            discovery_candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
        }
        let artist_cap = if options.diverse { 1 } else { limit.max(1) };
        let mut ordered = diversify_by_artist(discovery_candidates, discovery_target, artist_cap);

        local_candidates.retain(|candidate| {
            let id = candidate.profile.song.id.as_str();
            !exclude.contains(id) && !queue_ids.contains(id)
        });
        ordered.extend(local_candidates);
        let candidate_count = ordered.len();
        let picks = diversify_by_artist(ordered, limit, artist_cap);
        let discovery_count = picks
            .iter()
            .filter(|candidate| candidate.source == "related_to_preference")
            .count();
        let active_profile = self.repository.active_listening_profile()?;
        info!(
            category = "RECOMMENDER",
            event = "quick_picks_generated",
            profile_id = active_profile.id,
            candidate_count,
            selected_count = picks.len(),
            discovery_count,
            seed_count = seeds.len(),
            excluded_count = exclude.len(),
            duration_ms = started.elapsed().as_millis() as u64
        );
        Ok(picks)
    }
}

fn apply_artist_affinity(candidate: &mut RecommendedSong, affinities: &HashMap<String, f64>) {
    let key = candidate.profile.song.artist.id.clone().unwrap_or_else(|| {
        format!(
            "name:{}",
            candidate.profile.song.artist.name.trim().to_lowercase()
        )
    });
    let raw = affinities.get(&key).copied().unwrap_or(0.0);
    let contribution = (raw * 0.2).clamp(-2.0, 2.0);
    if contribution.abs() < f64::EPSILON {
        return;
    }
    candidate.score += contribution;
    candidate.components.push(crate::ScoreComponent {
        name: "artist_affinity",
        raw_value: raw,
        contribution,
    });
    if contribution >= 0.5 {
        candidate.reasons.push("Artist you enjoy".into());
    }
}

fn online_discovery_target(
    limit: usize,
    eligible_local_count: usize,
    discovery_enabled: bool,
) -> usize {
    if !discovery_enabled {
        return 0;
    }
    let preferred_online = ((limit as f64) * 0.75).ceil() as usize;
    preferred_online.max(limit.saturating_sub(eligible_local_count))
}

fn diversify_by_artist(
    candidates: Vec<RecommendedSong>,
    limit: usize,
    artist_cap: usize,
) -> Vec<RecommendedSong> {
    let mut selected = Vec::with_capacity(limit);
    let mut deferred = Vec::new();
    let mut artist_counts = std::collections::HashMap::<String, usize>::new();

    for candidate in candidates {
        let artist_key = candidate
            .profile
            .song
            .artist
            .id
            .clone()
            .unwrap_or_else(|| candidate.profile.song.artist.name.to_lowercase());
        let count = artist_counts.entry(artist_key).or_default();
        if *count < artist_cap && selected.len() < limit {
            *count += 1;
            selected.push(candidate);
        } else {
            deferred.push(candidate);
        }
    }

    if selected.len() < limit {
        selected.extend(deferred.into_iter().take(limit - selected.len()));
    }
    selected
}

fn rank_queue_candidates(mut candidates: Vec<Song>, recent: &[crate::RecentSong]) -> Vec<Song> {
    let recent_artist_weight = recent.iter().enumerate().fold(
        HashMap::<String, f64>::new(),
        |mut weights, (index, item)| {
            let key = item
                .song
                .artist
                .id
                .clone()
                .unwrap_or_else(|| item.song.artist.name.to_lowercase());
            weights
                .entry(key)
                .and_modify(|weight| *weight = weight.max(8.0 / (index as f64 + 1.0)))
                .or_insert_with(|| 8.0 / (index as f64 + 1.0));
            weights
        },
    );
    let recent_track_rank = recent
        .iter()
        .enumerate()
        .map(|(index, item)| (item.song.id.as_str().to_owned(), index))
        .collect::<HashMap<_, _>>();
    let original_rank = candidates
        .iter()
        .enumerate()
        .map(|(index, song)| (song.id.as_str().to_owned(), index))
        .collect::<HashMap<_, _>>();

    candidates.sort_by(|left, right| {
        let score = |song: &Song| {
            let artist_key = song
                .artist
                .id
                .clone()
                .unwrap_or_else(|| song.artist.name.to_lowercase());
            let artist_recency = recent_artist_weight
                .get(&artist_key)
                .copied()
                .unwrap_or(0.0);
            let direct_repeat_penalty = recent_track_rank
                .get(song.id.as_str())
                .map_or(0.0, |rank| 4.0 / (*rank as f64 + 1.0));
            let provider_rank = original_rank
                .get(song.id.as_str())
                .map_or(0.0, |rank| 2.0 / (*rank as f64 + 1.0).sqrt());
            artist_recency + provider_rank - direct_repeat_penalty
        };
        score(right)
            .total_cmp(&score(left))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    candidates
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;

    use async_trait::async_trait;
    use solmusic_domain::{ArtistRef, ListeningSummary, SongId, TrackProfile};

    use super::*;
    use crate::{DatabaseDiagnostics, LocalPlaybackFile, ProviderError, RecentSong, StorageError};

    struct FakeProvider {
        fail_second: AtomicBool,
        second: Song,
    }

    #[async_trait]
    impl MusicProvider for FakeProvider {
        async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<Song>, ProviderError> {
            Ok(Vec::new())
        }

        async fn related(
            &self,
            _song_id: &SongId,
            _limit: usize,
        ) -> Result<Vec<Song>, ProviderError> {
            Ok(vec![self.second.clone()])
        }

        async fn resolve_playback(
            &self,
            song_id: &SongId,
        ) -> Result<PlaybackSource, ProviderError> {
            if song_id == &self.second.id && self.fail_second.load(Ordering::SeqCst) {
                return Err(ProviderError::Unavailable);
            }
            Ok(PlaybackSource {
                url: format!("https://example.test/{}.m4a", song_id.as_str()),
                mime_type: "audio/mp4".into(),
                expires_at_ms: None,
                local_path: None,
                request_profile: PlaybackRequestProfile::Web,
            })
        }
    }

    struct DiscoveryTestProvider {
        calls: AtomicUsize,
        started: Notify,
        release: Notify,
    }

    #[async_trait]
    impl MusicProvider for DiscoveryTestProvider {
        async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<Song>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            Ok(vec![song("remote")])
        }

        async fn related(
            &self,
            _song_id: &SongId,
            _limit: usize,
        ) -> Result<Vec<Song>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        }

        async fn resolve_playback(
            &self,
            _song_id: &SongId,
        ) -> Result<PlaybackSource, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Unavailable)
        }
    }

    struct DiscoveryTestRepository {
        enabled: AtomicBool,
        saved: AtomicUsize,
    }

    impl MusicRepository for DiscoveryTestRepository {
        fn save_songs(&self, songs: &[Song]) -> Result<(), StorageError> {
            self.saved.fetch_add(songs.len(), Ordering::SeqCst);
            Ok(())
        }
        fn record_playback(&self, _summary: &ListeningSummary) -> Result<(), StorageError> {
            Ok(())
        }
        fn set_reaction(
            &self,
            _song: &Song,
            _liked: bool,
            _disliked: bool,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        fn recent_songs(
            &self,
            _limit: usize,
            _before: Option<i64>,
        ) -> Result<Vec<RecentSong>, StorageError> {
            Ok(Vec::new())
        }
        fn track_profiles(&self, _limit: usize) -> Result<Vec<TrackProfile>, StorageError> {
            Ok(Vec::new())
        }
        fn database_diagnostics(&self) -> Result<DatabaseDiagnostics, StorageError> {
            Ok(DatabaseDiagnostics {
                schema_version: 0,
                database_size_bytes: 0,
                track_count: 0,
                artist_count: 0,
                history_event_count: 0,
                liked_song_count: 0,
                integrity_status: "ok".into(),
                query_duration_ms: 0,
            })
        }
        fn discovery_enabled(&self) -> Result<bool, StorageError> {
            Ok(self.enabled.load(Ordering::SeqCst))
        }
        fn set_discovery_enabled(&self, enabled: bool, _now_ms: i64) -> Result<(), StorageError> {
            self.enabled.store(enabled, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FakeRepository {
        profiles: Vec<TrackProfile>,
        local_profiles: Vec<TrackProfile>,
        local_file: Option<LocalPlaybackFile>,
        discovery_enabled: bool,
    }

    impl Default for FakeRepository {
        fn default() -> Self {
            Self {
                profiles: Vec::new(),
                local_profiles: Vec::new(),
                local_file: None,
                discovery_enabled: true,
            }
        }
    }

    impl MusicRepository for FakeRepository {
        fn save_songs(&self, _songs: &[Song]) -> Result<(), StorageError> {
            Ok(())
        }
        fn record_playback(&self, _summary: &ListeningSummary) -> Result<(), StorageError> {
            Ok(())
        }
        fn set_reaction(
            &self,
            _song: &Song,
            _liked: bool,
            _disliked: bool,
        ) -> Result<(), StorageError> {
            Ok(())
        }
        fn recent_songs(
            &self,
            _limit: usize,
            _before: Option<i64>,
        ) -> Result<Vec<RecentSong>, StorageError> {
            Ok(Vec::new())
        }
        fn track_profiles(&self, limit: usize) -> Result<Vec<TrackProfile>, StorageError> {
            Ok(self.profiles.iter().take(limit).cloned().collect())
        }
        fn local_track_profiles(&self, limit: usize) -> Result<Vec<TrackProfile>, StorageError> {
            Ok(self.local_profiles.iter().take(limit).cloned().collect())
        }
        fn discovery_enabled(&self) -> Result<bool, StorageError> {
            Ok(self.discovery_enabled)
        }
        fn local_playback_file(
            &self,
            _song_id: &SongId,
        ) -> Result<Option<LocalPlaybackFile>, StorageError> {
            Ok(self.local_file.clone())
        }
        fn database_diagnostics(&self) -> Result<DatabaseDiagnostics, StorageError> {
            Ok(DatabaseDiagnostics {
                schema_version: 0,
                database_size_bytes: 0,
                track_count: 0,
                artist_count: 0,
                history_event_count: 0,
                liked_song_count: 0,
                integrity_status: "ok".into(),
                query_duration_ms: 0,
            })
        }
    }

    fn song(id: &str) -> Song {
        Song {
            id: SongId::new(id).unwrap(),
            title: id.into(),
            artist: ArtistRef {
                id: Some(format!("artist-{id}")),
                name: format!("Artist {id}"),
            },
            album_id: None,
            album_name: None,
            duration_ms: Some(100_000),
            thumbnail_url: None,
        }
    }

    #[test]
    fn queue_ranking_favors_the_most_recent_artist_without_repeating_the_same_track() {
        let mut newest_seed = song("newest-seed");
        newest_seed.artist.id = Some("artist-new".into());
        let mut older_seed = song("older-seed");
        older_seed.artist.id = Some("artist-old".into());
        let recent = vec![
            RecentSong {
                song: newest_seed,
                cursor: 200,
            },
            RecentSong {
                song: older_seed,
                cursor: 100,
            },
        ];
        let mut old_candidate = song("old-candidate");
        old_candidate.artist.id = Some("artist-old".into());
        let mut new_candidate = song("new-candidate");
        new_candidate.artist.id = Some("artist-new".into());
        let ranked = rank_queue_candidates(vec![old_candidate, new_candidate], &recent);
        assert_eq!(ranked[0].id.as_str(), "new-candidate");
    }

    #[test]
    fn online_discovery_fills_slots_when_local_library_is_small() {
        assert_eq!(online_discovery_target(4, 0, true), 4);
        assert_eq!(online_discovery_target(4, 1, true), 3);
        assert_eq!(online_discovery_target(4, 3, true), 3);
        assert_eq!(online_discovery_target(4, 20, true), 3);
        assert_eq!(online_discovery_target(4, 0, false), 0);
    }

    #[tokio::test]
    async fn discovery_off_calls_zero_provider_methods() {
        let provider = Arc::new(DiscoveryTestProvider {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        });
        let repository = Arc::new(DiscoveryTestRepository {
            enabled: AtomicBool::new(false),
            saved: AtomicUsize::new(0),
        });
        let app = SunnySongApp::new(provider.clone(), repository);

        assert!(app.search_discovery("test", 10).await.unwrap().is_empty());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn jellyfin_playback_bypasses_the_discovery_gate() {
        let remote = song("jellyfin:1:item");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: song("unused"),
        });
        let repository = Arc::new(FakeRepository {
            discovery_enabled: false,
            ..FakeRepository::default()
        });
        let app = SunnySongApp::new(provider, repository);

        let preparation = app.play_song(remote.clone()).await.unwrap();
        assert_eq!(preparation.state.current, Some(remote));
        assert!(preparation.source.url.contains("jellyfin:1:item"));
    }

    #[tokio::test]
    async fn disabling_discovery_discards_inflight_search() {
        let provider = Arc::new(DiscoveryTestProvider {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        });
        let repository = Arc::new(DiscoveryTestRepository {
            enabled: AtomicBool::new(true),
            saved: AtomicUsize::new(0),
        });
        let app = Arc::new(SunnySongApp::new(provider.clone(), repository.clone()));
        let started = provider.started.notified();
        let search_app = app.clone();
        let search = tokio::spawn(async move { search_app.search_discovery("test", 10).await });
        started.await;
        app.set_discovery_enabled(false, 2).unwrap();
        provider.release.notify_one();

        assert!(search.await.unwrap().unwrap().is_empty());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(repository.saved.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failed_next_resolution_keeps_committed_queue_state() {
        let first = song("first");
        let second = song("second");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second,
        });
        let app = SunnySongApp::new(provider.clone(), Arc::new(FakeRepository::default()));
        app.play_song(first.clone()).await.unwrap();
        app.hydrate_queue(&first.id).await.unwrap();
        provider.fail_second.store(true, Ordering::SeqCst);

        assert!(matches!(app.next().await, Err(AppError::Provider(_))));
        let state = app.playback_state();
        assert_eq!(state.current, Some(first));
        assert_eq!(state.current_index, Some(0));
        assert_eq!(state.queue.len(), 2);
    }

    #[tokio::test]
    async fn selecting_an_existing_queue_item_preserves_next_up() {
        let first = song("first");
        let second = song("second");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: second.clone(),
        });
        let app = SunnySongApp::new(provider, Arc::new(FakeRepository::default()));
        app.play_song(first.clone()).await.unwrap();
        let hydrated = app.hydrate_queue(&first.id).await.unwrap();
        let original_queue = hydrated.queue.clone();

        let selected = app.play_queue_item(&second.id).await.unwrap();
        assert_eq!(selected.state.queue, original_queue);
        assert_eq!(selected.state.current, Some(second));
        assert_eq!(selected.state.current_index, Some(1));
    }

    #[tokio::test]
    async fn local_playback_bypasses_online_resolution_when_discovery_is_off() {
        let local = song("local:track");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(true),
            second: local.clone(),
        });
        let repository = FakeRepository {
            local_file: Some(LocalPlaybackFile {
                path: "/music/track.flac".into(),
                mime_type: "audio/flac".into(),
            }),
            discovery_enabled: false,
            ..FakeRepository::default()
        };
        let app = SunnySongApp::new(provider, Arc::new(repository));

        let preparation = app.play_song(local).await.unwrap();
        assert_eq!(
            preparation.source.local_path,
            Some(std::path::PathBuf::from("/music/track.flac"))
        );
        assert!(preparation.source.url.is_empty());
    }

    #[tokio::test]
    async fn quick_picks_discovers_unheard_tracks_from_preferred_seeds() {
        let first = song("first");
        let second = song("second");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: second.clone(),
        });
        let repository = FakeRepository {
            profiles: vec![TrackProfile {
                song: first.clone(),
                play_count: 5,
                completed_count: 4,
                early_skip_count: 0,
                completion_ema: 0.9,
                affinity: 5.0,
                liked: true,
                disliked: false,
                last_played_at_ms: Some(0),
            }],
            ..FakeRepository::default()
        };
        let app = SunnySongApp::new(provider, Arc::new(repository));

        let picks = app
            .quick_picks(
                4,
                40 * 24 * 3_600_000,
                &HashSet::new(),
                QuickPickOptions::default(),
            )
            .await
            .unwrap();

        let discovery = picks
            .iter()
            .find(|pick| pick.profile.song.id == second.id)
            .expect("related unheard song should be recommended");
        assert_eq!(discovery.source, "related_to_preference");
        assert!(discovery
            .reasons
            .iter()
            .any(|reason| reason == "Based on a liked track"));
        assert!(!picks.is_empty());
    }

    #[tokio::test]
    async fn queue_refill_appends_exact_count_without_replacing_or_duplicating() {
        let first = song("first");
        let already_queued = song("already-queued");
        let candidates = ["already-queued", "new-1", "new-2", "new-3", "new-4"]
            .into_iter()
            .map(|id| TrackProfile {
                song: song(id),
                play_count: 1,
                completed_count: 1,
                early_skip_count: 0,
                completion_ema: 1.0,
                affinity: 1.0,
                liked: false,
                disliked: false,
                last_played_at_ms: None,
            })
            .collect();
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: song("unused"),
        });
        let repository = Arc::new(FakeRepository {
            local_profiles: candidates,
            discovery_enabled: false,
            ..FakeRepository::default()
        });
        let app = SunnySongApp::new(provider, repository);
        {
            let mut state = app.playback.lock().expect("playback state poisoned");
            state.current = Some(first.clone());
            state.current_index = Some(0);
            state.queue = vec![first.clone(), already_queued.clone()];
        }

        let original_queue = app.playback_state().queue;
        let refilled = app.refill_queue(3).await.unwrap();

        assert_eq!(&refilled.queue[..original_queue.len()], original_queue);
        assert_eq!(refilled.queue.len(), original_queue.len() + 3);
        let unique_ids = refilled
            .queue
            .iter()
            .map(|candidate| candidate.id.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(unique_ids.len(), refilled.queue.len());
    }

    #[tokio::test]
    async fn queue_boundary_is_explicit_and_does_not_replay_edge_item() {
        let first = song("first");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: song("second"),
        });
        let app = SunnySongApp::new(provider, Arc::new(FakeRepository::default()));
        app.play_song(first.clone()).await.unwrap();

        assert!(matches!(app.previous().await, Err(AppError::QueueBoundary)));
        assert_eq!(app.playback_state().current, Some(first));
    }
}
