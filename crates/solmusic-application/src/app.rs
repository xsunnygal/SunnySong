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

#[derive(Debug, Clone)]
pub struct DiscoverSection {
    pub id: String,
    pub title: String,
    pub description: String,
    pub items: Vec<RecommendedSong>,
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
                normalization_gain_metadata: local.normalization_gain_metadata,
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
        let mut seen_families = HashSet::from([song_family_key(&current)]);
        let mut queue = vec![current];
        queue.extend(
            candidates
                .into_iter()
                .filter(|candidate| {
                    seen.insert(candidate.id.as_str().to_owned())
                        && seen_families.insert(song_family_key(candidate))
                })
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
        let mut seen_families = state
            .queue
            .iter()
            .map(song_family_key)
            .collect::<HashSet<_>>();
        let additions = candidates
            .into_iter()
            .filter(|candidate| {
                seen.insert(candidate.id.as_str().to_owned())
                    && seen_families.insert(song_family_key(candidate))
            })
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

    pub fn enqueue_next(&self, song: Song) -> Result<PlaybackState, AppError> {
        self.repository.save_songs(std::slice::from_ref(&song))?;
        let mut state = self.playback.lock().expect("playback state poisoned");
        let mut current = state.current_index.ok_or(AppError::EmptyQueue)?;
        if state
            .current
            .as_ref()
            .is_some_and(|item| item.id == song.id)
        {
            return Ok(state.clone());
        }
        if let Some(existing) = state
            .queue
            .iter()
            .enumerate()
            .find_map(|(index, item)| (item.id == song.id).then_some(index))
        {
            state.queue.remove(existing);
            if existing < current {
                current -= 1;
                state.current_index = Some(current);
            }
        }
        let insertion = (current + 1).min(state.queue.len());
        state.queue.insert(insertion, song);
        Ok(state.clone())
    }

    pub fn enqueue_song(&self, song: Song) -> Result<PlaybackState, AppError> {
        self.repository.save_songs(std::slice::from_ref(&song))?;
        let mut state = self.playback.lock().expect("playback state poisoned");
        if state.queue.iter().all(|item| item.id != song.id) {
            state.queue.push(song);
        }
        Ok(state.clone())
    }

    pub fn remove_queue_item(&self, index: usize) -> Result<PlaybackState, AppError> {
        let mut state = self.playback.lock().expect("playback state poisoned");
        let current = state.current_index.ok_or(AppError::EmptyQueue)?;
        if index >= state.queue.len() {
            return Err(AppError::InvalidQueueOperation(
                "queue index is out of bounds".into(),
            ));
        }
        if index == current {
            return Err(AppError::InvalidQueueOperation(
                "the currently playing item cannot be removed".into(),
            ));
        }
        state.queue.remove(index);
        if index < current {
            state.current_index = Some(current - 1);
        }
        Ok(state.clone())
    }

    pub fn move_queue_item(&self, from: usize, to: usize) -> Result<PlaybackState, AppError> {
        let mut state = self.playback.lock().expect("playback state poisoned");
        if from >= state.queue.len() || to >= state.queue.len() {
            return Err(AppError::InvalidQueueOperation(
                "queue index is out of bounds".into(),
            ));
        }
        if from == to {
            return Ok(state.clone());
        }
        let current = state.current_index.ok_or(AppError::EmptyQueue)?;
        let item = state.queue.remove(from);
        state.queue.insert(to, item);
        state.current_index = Some(if current == from {
            to
        } else if from < current && to >= current {
            current - 1
        } else if from > current && to <= current {
            current + 1
        } else {
            current
        });
        state.current = state
            .current_index
            .and_then(|index| state.queue.get(index).cloned());
        Ok(state.clone())
    }

    pub fn clear_upcoming(&self) -> Result<PlaybackState, AppError> {
        let mut state = self.playback.lock().expect("playback state poisoned");
        let current = state.current_index.ok_or(AppError::EmptyQueue)?;
        state.queue.truncate(current + 1);
        Ok(state.clone())
    }

    pub async fn replace_queue(
        &self,
        songs: Vec<Song>,
        start_index: usize,
    ) -> Result<PlaybackPreparation, AppError> {
        if songs.is_empty() || start_index >= songs.len() {
            return Err(AppError::InvalidQueueOperation(
                "replacement queue and start index are invalid".into(),
            ));
        }
        let selected_id = songs[start_index].id.clone();
        let mut seen = HashSet::new();
        let queue = songs
            .into_iter()
            .filter(|song| seen.insert(song.id.as_str().to_owned()))
            .collect::<Vec<_>>();
        let current_index = queue
            .iter()
            .position(|song| song.id == selected_id)
            .ok_or_else(|| AppError::InvalidQueueOperation("selected song was removed".into()))?;
        self.repository.save_songs(&queue)?;
        let current = queue[current_index].clone();
        let generation = self.playback_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let source = self.resolve_source(&current.id, false).await?;
        if self.playback_generation.load(Ordering::SeqCst) != generation {
            return Err(AppError::StalePlaybackOperation);
        }
        let state = PlaybackState {
            current: Some(current),
            queue,
            current_index: Some(current_index),
        };
        *self.playback.lock().expect("playback state poisoned") = state.clone();
        Ok(PlaybackPreparation { source, state })
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
        if state.current_index != Some(original_index)
            || current_song_id != original_song_id
            || state.queue.get(target_index).map(|item| &item.id) != Some(&song.id)
        {
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

    pub async fn prepare_next_playback_source(
        &self,
        song_id: &solmusic_domain::SongId,
    ) -> Result<PlaybackSource, AppError> {
        let (current_index, current_song_id) = {
            let state = self.playback.lock().expect("playback state poisoned");
            let current_index = state.current_index.ok_or(AppError::EmptyQueue)?;
            let is_next = state
                .queue
                .get(current_index + 1)
                .is_some_and(|song| &song.id == song_id);
            if !is_next {
                return Err(AppError::InvalidQueueOperation(
                    "the requested song is not next in the current queue".into(),
                ));
            }
            let current_song_id = state
                .current
                .as_ref()
                .map(|song| song.id.as_str().to_owned());
            (current_index, current_song_id)
        };

        let started = Instant::now();
        let source = self.resolve_source(song_id, false).await?;
        let state = self.playback.lock().expect("playback state poisoned");
        let unchanged = state.current_index == Some(current_index)
            && state.current.as_ref().map(|song| song.id.as_str()) == current_song_id.as_deref()
            && state
                .queue
                .get(current_index + 1)
                .is_some_and(|song| &song.id == song_id);
        if !unchanged {
            return Err(AppError::StalePlaybackOperation);
        }
        debug!(
            category = "PLAYER",
            event = "next_playback_source_prepared",
            track_id = song_id.as_str(),
            duration_ms = started.elapsed().as_millis() as u64
        );
        Ok(source)
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
                normalization_gain_metadata: local.normalization_gain_metadata,
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

    pub fn local_scan_state(&self, directory_id: i64) -> Result<Vec<ScannedLocalTrack>, AppError> {
        Ok(self.repository.local_scan_state(directory_id)?)
    }

    pub fn referenced_local_artwork(&self) -> Result<Vec<String>, AppError> {
        Ok(self.repository.referenced_local_artwork()?)
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

    pub fn mark_local_file_missing(
        &self,
        canonical_path: &str,
        missing_since_ms: i64,
    ) -> Result<(), AppError> {
        Ok(self
            .repository
            .mark_local_file_missing(canonical_path, missing_since_ms)?)
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

    pub fn rename_playlist(
        &self,
        playlist_id: &str,
        name: &str,
        now_ms: i64,
    ) -> Result<crate::Playlist, AppError> {
        Ok(self.repository.rename_playlist(playlist_id, name, now_ms)?)
    }

    pub fn delete_playlist(&self, playlist_id: &str) -> Result<(), AppError> {
        Ok(self.repository.delete_playlist(playlist_id)?)
    }

    pub fn remove_song_from_playlist(
        &self,
        playlist_id: &str,
        song_id: &crate::domain::SongId,
        now_ms: i64,
    ) -> Result<(), AppError> {
        Ok(self
            .repository
            .remove_song_from_playlist(playlist_id, song_id, now_ms)?)
    }

    pub fn reorder_playlist_tracks(
        &self,
        playlist_id: &str,
        song_ids: &[String],
        now_ms: i64,
    ) -> Result<Vec<crate::PlaylistTrack>, AppError> {
        Ok(self
            .repository
            .reorder_playlist_tracks(playlist_id, song_ids, now_ms)?)
    }

    pub fn playlist_export_tracks(
        &self,
        playlist_id: &str,
    ) -> Result<Vec<crate::PlaylistExportTrack>, AppError> {
        Ok(self.repository.playlist_export_tracks(playlist_id)?)
    }

    pub fn song_by_id(&self, song_id: &crate::domain::SongId) -> Result<Option<Song>, AppError> {
        Ok(self.repository.song_by_id(song_id)?)
    }

    pub fn song_by_local_path(&self, path: &std::path::PathBuf) -> Result<Option<Song>, AppError> {
        Ok(self.repository.song_by_local_path(path)?)
    }

    pub fn create_playlist_from_songs(
        &self,
        name: &str,
        song_ids: &[crate::domain::SongId],
        now_ms: i64,
    ) -> Result<crate::Playlist, AppError> {
        Ok(self
            .repository
            .create_playlist_from_songs(name, song_ids, now_ms)?)
    }

    pub fn create_backup(&self, destination: &std::path::PathBuf) -> Result<(), AppError> {
        Ok(self.repository.create_backup(destination)?)
    }

    pub fn liked_song_ids(&self) -> Result<Vec<String>, AppError> {
        Ok(self.repository.liked_song_ids()?)
    }

    pub fn liked_songs(&self, limit: usize, offset: usize) -> Result<Vec<Song>, AppError> {
        Ok(self.repository.liked_songs(limit, offset)?)
    }

    pub fn history_events(
        &self,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<crate::HistoryEvent>, AppError> {
        Ok(self.repository.history_events(limit, before)?)
    }

    pub fn delete_history_event(&self, event_id: &str) -> Result<(), AppError> {
        Ok(self.repository.delete_history_event(event_id)?)
    }

    pub fn delete_song_history(&self, song_id: &crate::domain::SongId) -> Result<u64, AppError> {
        Ok(self.repository.delete_song_history(song_id)?)
    }

    pub fn clear_history(&self) -> Result<u64, AppError> {
        Ok(self.repository.clear_history()?)
    }

    pub fn listening_recap(
        &self,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        limit: usize,
    ) -> Result<crate::ListeningRecap, AppError> {
        Ok(self.repository.listening_recap(from_ms, to_ms, limit)?)
    }

    pub fn downloads(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::DownloadRecord>, AppError> {
        Ok(self.repository.downloads(limit, offset)?)
    }

    pub fn create_download_attempt(
        &self,
        song: &Song,
        now_ms: i64,
    ) -> Result<crate::DownloadRecord, AppError> {
        Ok(self.repository.create_download_attempt(song, now_ms)?)
    }

    pub fn finish_download_attempt(
        &self,
        id: &str,
        status: &str,
        location: Option<&str>,
        file_name: Option<&str>,
        error: Option<&str>,
        now_ms: i64,
    ) -> Result<(), AppError> {
        Ok(self
            .repository
            .finish_download_attempt(id, status, location, file_name, error, now_ms)?)
    }

    pub fn remove_download(&self, id: &str) -> Result<(), AppError> {
        Ok(self.repository.remove_download(id)?)
    }

    pub async fn search_suggestions(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>, AppError> {
        let query = query.trim();
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut suggestions = self.repository.local_text_suggestions(query, limit)?;
        if suggestions.len() < limit && self.repository.discovery_enabled()? {
            if let Ok(remote) = self.provider.search_suggestions(query, limit).await {
                suggestions.extend(remote);
            }
        }
        let mut seen = HashSet::new();
        suggestions.retain(|value| {
            let value = value.trim();
            !value.is_empty() && seen.insert(value.to_lowercase())
        });
        suggestions.truncate(limit);
        Ok(suggestions)
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
        let ordered = deduplicate_recommendations_by_song_family(ordered);
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

    pub async fn discover_feed(
        &self,
        section_limit: usize,
        now_ms: i64,
    ) -> Result<Vec<DiscoverSection>, AppError> {
        let limit = section_limit.clamp(1, 20);
        let mut excluded = HashSet::new();
        let definitions = [
            (
                "favorites",
                "Based on your favorites",
                "Songs ranked from your listening and reaction history",
                QuickPickOptions {
                    diverse: true,
                    new_songs: false,
                    rediscover: true,
                },
            ),
            (
                "unheard",
                "Explore something new",
                "Underplayed library songs and discovery results when enabled",
                QuickPickOptions {
                    diverse: true,
                    new_songs: true,
                    rediscover: false,
                },
            ),
            (
                "rediscover",
                "Rediscover",
                "Familiar songs you have not heard as recently",
                QuickPickOptions {
                    diverse: true,
                    new_songs: false,
                    rediscover: true,
                },
            ),
        ];
        let mut sections = Vec::new();
        for (id, title, description, options) in definitions {
            let items = self.quick_picks(limit, now_ms, &excluded, options).await?;
            excluded.extend(
                items
                    .iter()
                    .map(|item| item.profile.song.id.as_str().to_owned()),
            );
            if !items.is_empty() {
                sections.push(DiscoverSection {
                    id: id.into(),
                    title: title.into(),
                    description: description.into(),
                    items,
                });
            }
        }
        Ok(sections)
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

fn canonical_song_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn base_song_title(title: &str) -> String {
    const VARIANT_MARKERS: [&str; 10] = [
        "remix",
        "remaster",
        "radio edit",
        "extended mix",
        "club mix",
        "original mix",
        "official audio",
        "official lyric",
        "lyric video",
        "visualizer",
    ];

    let lowered = title.to_lowercase();
    let mut base = String::with_capacity(lowered.len());
    let mut segment = String::new();
    let mut closing = None;
    for character in lowered.chars() {
        match (closing, character) {
            (None, '(') => {
                segment.clear();
                closing = Some(')');
            }
            (None, '[') => {
                segment.clear();
                closing = Some(']');
            }
            (Some(expected), value) if value == expected => {
                if !VARIANT_MARKERS
                    .iter()
                    .any(|marker| segment.contains(marker))
                {
                    base.push(' ');
                    base.push_str(&segment);
                }
                closing = None;
            }
            (Some(_), value) => segment.push(value),
            (None, value) => base.push(value),
        }
    }
    if closing.is_some() {
        base.push(' ');
        base.push_str(&segment);
    }

    let mut cutoff = base.len();
    for separator in [" - ", " – ", " — "] {
        for (index, _) in base.match_indices(separator) {
            let suffix = &base[index + separator.len()..];
            if VARIANT_MARKERS.iter().any(|marker| suffix.contains(marker)) {
                cutoff = cutoff.min(index);
            }
        }
    }
    base.truncate(cutoff);
    canonical_song_text(&base)
}

fn song_family_key(song: &Song) -> String {
    format!(
        "{}:{}",
        canonical_song_text(&song.artist.name),
        base_song_title(&song.title)
    )
}

fn deduplicate_recommendations_by_song_family(
    candidates: Vec<RecommendedSong>,
) -> Vec<RecommendedSong> {
    let mut best = HashMap::<String, RecommendedSong>::new();
    for candidate in candidates {
        let key = song_family_key(&candidate.profile.song);
        match best.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(candidate);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if candidate.score > entry.get().score {
                    entry.insert(candidate);
                }
            }
        }
    }
    let mut selected = best.into_values().collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        right.score.total_cmp(&left.score).then_with(|| {
            left.profile
                .song
                .id
                .as_str()
                .cmp(right.profile.song.id.as_str())
        })
    });
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
    let mut seen_families = HashSet::new();
    candidates.retain(|song| seen_families.insert(song_family_key(song)));
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
                normalization_gain_metadata: None,
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

        async fn search_suggestions(
            &self,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<String>, ProviderError> {
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
        fn local_text_suggestions(
            &self,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<String>, StorageError> {
            Ok(vec!["Local suggestion".into()])
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
    fn queue_ranking_keeps_only_the_best_scored_song_variant() {
        let mut original = song("original-upload");
        original.title = "Midnight Drive".into();
        original.artist.name = "Sunny Artist".into();
        let mut duplicate = song("later-upload");
        duplicate.title = "Midnight Drive (Official Audio)".into();
        duplicate.artist.name = "Sunny Artist".into();
        let mut remix = song("remix-upload");
        remix.title = "Midnight Drive - Club Remix".into();
        remix.artist.name = "Sunny Artist".into();

        let ranked = rank_queue_candidates(vec![original.clone(), duplicate, remix], &[]);
        assert_eq!(ranked, vec![original]);
    }

    #[test]
    fn semantic_song_families_do_not_merge_different_artists() {
        let mut first = song("first-artist");
        first.title = "Home".into();
        first.artist.name = "Artist One".into();
        let mut second = song("second-artist");
        second.title = "Home (Remix)".into();
        second.artist.name = "Artist Two".into();

        assert_ne!(song_family_key(&first), song_family_key(&second));
        assert_eq!(rank_queue_candidates(vec![first, second], &[]).len(), 2);
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
    async fn suggestion_provider_failure_returns_accumulated_local_results() {
        let provider = Arc::new(DiscoveryTestProvider {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        });
        let repository = Arc::new(DiscoveryTestRepository {
            enabled: AtomicBool::new(true),
            saved: AtomicUsize::new(0),
        });
        let app = SunnySongApp::new(provider.clone(), repository);

        assert_eq!(
            app.search_suggestions("local", 10).await.unwrap(),
            vec!["Local suggestion"]
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
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
    async fn preparing_next_source_does_not_mutate_queue_state() {
        let first = song("first");
        let second = song("second");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: second.clone(),
        });
        let app = SunnySongApp::new(provider, Arc::new(FakeRepository::default()));
        app.play_song(first.clone()).await.unwrap();
        app.hydrate_queue(&first.id).await.unwrap();
        let before = app.playback_state();

        let source = app.prepare_next_playback_source(&second.id).await.unwrap();
        let after = app.playback_state();

        assert!(source.url.contains(second.id.as_str()));
        assert_eq!(after.current, before.current);
        assert_eq!(after.current_index, before.current_index);
        assert_eq!(after.queue, before.queue);
    }

    #[tokio::test]
    async fn preparing_next_source_rejects_a_non_next_song() {
        let first = song("first");
        let second = song("second");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: second.clone(),
        });
        let app = SunnySongApp::new(provider, Arc::new(FakeRepository::default()));
        app.play_song(first.clone()).await.unwrap();
        app.hydrate_queue(&first.id).await.unwrap();
        app.enqueue_song(song("later")).unwrap();
        let before = app.playback_state();

        assert!(matches!(
            app.prepare_next_playback_source(&SongId::new("later").unwrap())
                .await,
            Err(AppError::InvalidQueueOperation(_))
        ));
        let after = app.playback_state();
        assert_eq!(after.current, before.current);
        assert_eq!(after.current_index, before.current_index);
        assert_eq!(after.queue, before.queue);
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
                normalization_gain_metadata: Some(crate::NormalizationGainMetadata {
                    track_gain_db: Some(-7.5),
                    album_gain_db: Some(-5.0),
                    track_peak: Some(0.98),
                    album_peak: Some(1.0),
                }),
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
        assert_eq!(
            preparation.source.normalization_gain_metadata,
            Some(crate::NormalizationGainMetadata {
                track_gain_db: Some(-7.5),
                album_gain_db: Some(-5.0),
                track_peak: Some(0.98),
                album_peak: Some(1.0),
            })
        );
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
    async fn queue_editing_keeps_current_identity_and_play_next_is_immediate() {
        let first = song("first");
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: song("unused"),
        });
        let app = SunnySongApp::new(provider, Arc::new(FakeRepository::default()));
        app.play_song(first.clone()).await.unwrap();
        app.enqueue_song(song("later")).unwrap();
        app.enqueue_next(song("next")).unwrap();
        app.enqueue_next(song("later")).unwrap();
        assert_eq!(
            app.playback_state()
                .queue
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["first", "later", "next"]
        );
        app.move_queue_item(2, 1).unwrap();
        app.remove_queue_item(2).unwrap();
        let state = app.clear_upcoming().unwrap();
        assert_eq!(state.queue, vec![first.clone()]);
        assert_eq!(state.current, Some(first));
        assert!(app.remove_queue_item(0).is_err());
    }

    #[tokio::test]
    async fn replace_queue_deduplicates_and_prepares_selected_song() {
        let provider = Arc::new(FakeProvider {
            fail_second: AtomicBool::new(false),
            second: song("unused"),
        });
        let app = SunnySongApp::new(provider, Arc::new(FakeRepository::default()));
        let selected = song("selected");
        let preparation = app
            .replace_queue(vec![song("first"), selected.clone(), selected.clone()], 2)
            .await
            .unwrap();
        assert_eq!(preparation.state.queue.len(), 2);
        assert_eq!(preparation.state.current, Some(selected));
        assert_eq!(preparation.state.current_index, Some(1));
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
