import { addPluginListener, type PluginListener } from "@tauri-apps/api/core";
import {
	getLikedSongIds,
	getPlaybackState,
	hydratePlaybackQueue,
	playNext,
	playPrevious,
	playQueueItem,
	preparePlayback,
	refreshPlaybackSource,
	refillPlaybackQueue,
	reportPlayback,
	setReaction,
	syncMediaSession,
	warmPlaybackSource,
	type PlaybackPreparation,
	type PlaybackState,
	type PlaybackSummary,
	type Song,
} from "$lib/api/backend";

type EndReason =
	"completed" | "next" | "previous" | "replaced" | "stopped" | "failed";

const PENDING_SUMMARIES_KEY = "solmusic-pending-summaries";
const PLAYER_SNAPSHOT_KEY = "solmusic-player-snapshot";

interface PlayerSnapshot extends PlaybackState {
	positionMs: number;
}

function errorMessage(error: unknown) {
	return error instanceof Error ? error.message : String(error);
}

function requiresYouTubeVerification(error: unknown) {
	const message = errorMessage(error).toLowerCase();
	return (
		message.includes("account verification is required") ||
		message.includes("sign in to confirm") ||
		message.includes("not a bot")
	);
}

class PlayerController {
	current = $state<Song | null>(null);
	queue = $state<Song[]>([]);
	currentIndex = $state<number | null>(null);
	playing = $state(false);
	positionMs = $state(0);
	durationMs = $state(0);
	bufferedMs = $state(0);
	loading = $state(false);
	error = $state<string | null>(null);
	likedSongIds = $state<string[]>([]);
	repeatOne = $state(false);
	private optimisticCurrent = $state<Song | null>(null);
	private optimisticCurrentIndex = $state<number | null>(null);
	private optimisticWasPlaying = false;
	private audio: HTMLAudioElement | null = null;
	private eventId: string | null = null;
	private profileId: string | null = null;
	private sessionProfileId: string | null = null;
	private startedAtMs = 0;
	private listenedMs = 0;
	private listeningSince: number | null = null;
	private loadGeneration = 0;
	private sourceRetryCount = 0;
	private bufferingTimer: ReturnType<typeof setTimeout> | null = null;
	private stallRecoveryTimer: ReturnType<typeof setTimeout> | null = null;
	private pendingSummaries: PlaybackSummary[] = [];
	private lastSnapshotAt = 0;
	private initialized = false;
	private warmingSongIds = new Set<string>();
	private audioEventsAttached = false;
	private mediaControlListener: PluginListener | null = null;
	private mediaControlsInitialized = false;
	private mediaControlRegistrationPending = false;
	private mediaControlRetryTimer: ReturnType<typeof setTimeout> | null = null;
	private lastMediaSessionSyncAt = 0;
	private lastMediaSessionFingerprint = "";
	private flushPromise: Promise<void> | null = null;
	private refillPromise: Promise<void> | null = null;
	private pendingRefillCount = 0;
	private pendingSeekMs: number | null = null;
	private warmedQueueForSongId = "";

	get visibleCurrent() {
		return this.optimisticCurrent ?? this.current;
	}

	get visibleCurrentIndex() {
		return this.optimisticCurrent
			? this.optimisticCurrentIndex
			: this.currentIndex;
	}

	get visiblePositionMs() {
		return this.optimisticCurrent ? 0 : this.positionMs;
	}

	get visibleDurationMs() {
		return this.optimisticCurrent?.durationMs ?? this.durationMs;
	}

	private ensureAudio() {
		if (this.audio || typeof Audio === "undefined") return;
		const audio = new Audio();
		audio.preload = "auto";
		this.audio = audio;
		if (this.audioEventsAttached) return;
		this.audioEventsAttached = true;
		audio.addEventListener("timeupdate", () => this.sync());
		audio.addEventListener("durationchange", () => this.sync());
		audio.addEventListener("progress", () => this.sync());
		audio.addEventListener("canplaythrough", () => this.preloadUpcoming(2));
		audio.addEventListener("play", () => this.onPlay());
		audio.addEventListener("playing", () => this.onPlaying());
		audio.addEventListener("pause", () => this.onPause());
		audio.addEventListener("waiting", () => this.onWaiting());
		audio.addEventListener("seeking", () => this.onSeeking());
		audio.addEventListener("seeked", () => this.onSeeked());
		audio.addEventListener("ended", () => void this.ended());
		audio.addEventListener("error", () => void this.handleMediaError());
	}

	async initialize(profileId?: string) {
		this.ensureAudio();
		if (profileId) this.profileId = profileId;
		void this.initializeMediaControls();
		if (this.initialized || typeof localStorage === "undefined") return;
		this.initialized = true;
		this.pendingSummaries = this.readJson<PlaybackSummary[]>(
			PENDING_SUMMARIES_KEY,
			[],
		)
			.map((summary) => ({
				...summary,
				profileId: summary.profileId || this.profileId || "",
			}))
			.filter((summary) => Boolean(summary.profileId));
		const snapshot = this.readJson<PlayerSnapshot | null>(
			PLAYER_SNAPSHOT_KEY,
			null,
		);
		try {
			const state = await getPlaybackState();
			if (state.current) {
				this.applyState(state);
				if (snapshot?.current && snapshot.current.id === state.current.id) {
					this.positionMs = Math.max(0, snapshot.positionMs || 0);
					this.durationMs = state.current.durationMs ?? 0;
				}
			} else if (snapshot?.current) {
				this.applyState(snapshot);
				this.positionMs = Math.max(0, snapshot.positionMs || 0);
				this.durationMs = snapshot.current.durationMs ?? 0;
			}
		} catch {
			if (snapshot?.current) {
				this.applyState(snapshot);
				this.positionMs = Math.max(0, snapshot.positionMs || 0);
			}
		}
		await this.refreshLikedSongs();
		void this.flushPending();
		this.syncSystemMediaSession(true);
	}

	preload(song: Song) {
		if (this.current?.id === song.id || this.warmingSongIds.has(song.id))
			return;
		this.warmingSongIds.add(song.id);
		void warmPlaybackSource(song.id)
			.catch(() => {
				// Prewarming is opportunistic; normal playback reports actionable failures.
			})
			.finally(() => this.warmingSongIds.delete(song.id));
	}

	async playSong(song: Song) {
		if (this.current?.id === song.id) {
			if (this.audio?.src) {
				if (this.audio.paused) await this.toggle();
				return;
			}
			const resumeAt = this.positionMs;
			try {
				await this.load(() => preparePlayback(song), resumeAt);
			} catch {
				// The player error state is rendered in the transport bar.
			}
			return;
		}

		const queued =
			this.queue.length > 1 && this.queue.some((item) => item.id === song.id);
		if (queued) {
			await this.playQueuedSong(song);
			return;
		}

		this.finalize("replaced");
		this.beginOptimisticSelection(song, 0);
		try {
			await this.load(() => preparePlayback(song));
		} catch {
			// The player error state is rendered in the transport bar.
		}
	}

	async playQueuedSong(song: Song) {
		if (this.current?.id === song.id) {
			if (this.audio?.paused) await this.toggle();
			return;
		}
		if (!this.queue.some((item) => item.id === song.id)) {
			this.error = "That song is no longer in Next Up.";
			return;
		}

		const targetIndex = this.queue.findIndex((item) => item.id === song.id);
		const consumed = Math.max(0, targetIndex - (this.currentIndex ?? 0));
		this.finalize("next");
		this.beginOptimisticSelection(song, targetIndex);
		try {
			await this.load(() => playQueueItem(song.id), 0, consumed);
		} catch {
			// Queue selection never falls back to preparePlayback: doing so would replace
			// the queue and regenerate Next Up around the selected song.
			this.playing = this.audio ? !this.audio.paused : false;
		}
	}

	async next(reason: EndReason = "next") {
		const targetIndex = (this.currentIndex ?? -1) + 1;
		const target = this.queue[targetIndex];
		if (!target) return;
		if (reason !== "completed") this.finalize(reason);
		this.beginOptimisticSelection(target, targetIndex);
		try {
			await this.load(playNext, 0, 1);
		} catch {
			this.playing = false;
		}
	}

	toggleRepeatOne() {
		this.repeatOne = !this.repeatOne;
	}

	async previous() {
		if (this.audio && this.audio.currentTime > 5) {
			this.audio.currentTime = 0;
			return;
		}
		const targetIndex = (this.currentIndex ?? 0) - 1;
		const target = this.queue[targetIndex];
		if (!target) return;
		this.finalize("previous");
		this.beginOptimisticSelection(target, targetIndex);
		try {
			await this.load(playPrevious);
		} catch {
			this.playing = false;
		}
	}

	async toggle() {
		if (!this.audio || !this.current) return;
		if (!this.audio.src) {
			const current = this.current;
			const resumeAt = this.positionMs;
			try {
				await this.load(() => preparePlayback(current), resumeAt);
			} catch {
				// The player error state is rendered in the transport bar.
			}
			return;
		}
		if (this.audio.error) {
			this.sourceRetryCount = 0;
			await this.handleMediaError();
			return;
		}
		if (this.audio.paused) {
			try {
				await this.audio.play();
			} catch (error) {
				this.error = error instanceof Error ? error.message : String(error);
			}
		} else this.audio.pause();
	}

	preservePlaybackAfterNavigation() {
		const audio = this.audio;
		if (!this.current || !audio?.src) return;
		const resumeIfInterrupted = () => {
			if (this.current && audio.paused) void audio.play().catch(() => {});
		};
		requestAnimationFrame(resumeIfInterrupted);
		setTimeout(resumeIfInterrupted, 150);
	}

	seek(value: number) {
		if (!Number.isFinite(value)) return;
		const maximum =
			this.durationMs > 0 ? this.durationMs : Number.MAX_SAFE_INTEGER;
		const target = Math.max(0, Math.min(value, maximum));
		this.positionMs = target;
		this.pendingSeekMs = target;
		if (this.audio?.src) this.audio.currentTime = target / 1000;
		this.persistSnapshot();
		this.syncSystemMediaSession(true);
	}

	isLiked(songId: string) {
		return this.likedSongIds.includes(songId);
	}

	seedLiked(songId: string) {
		if (!this.isLiked(songId))
			this.likedSongIds = [...this.likedSongIds, songId];
	}

	async changeListeningProfile(
		profileId: string,
		switchProfile: () => Promise<void>,
	) {
		if (this.profileId === profileId) return;
		this.finalize("stopped");
		await switchProfile();
		this.profileId = profileId;
		await this.refreshLikedSongs();
		// Playback remains shared, but the current song stays attributed only to
		// the profile that started it. The new profile begins learning next track.
	}

	async refreshLikedSongs() {
		try {
			this.likedSongIds = await getLikedSongIds();
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
		}
	}

	async toggleLike(song: Song) {
		const wasLiked = this.isLiked(song.id);
		this.likedSongIds = wasLiked
			? this.likedSongIds.filter((id) => id !== song.id)
			: [...this.likedSongIds, song.id];
		try {
			await setReaction(song, !wasLiked, false);
		} catch (error) {
			this.likedSongIds = wasLiked
				? [...this.likedSongIds, song.id]
				: this.likedSongIds.filter((id) => id !== song.id);
			this.error = error instanceof Error ? error.message : String(error);
		}
	}

	sync() {
		if (!this.audio) return;
		this.positionMs = Math.floor(this.audio.currentTime * 1000);
		this.durationMs = Number.isFinite(this.audio.duration)
			? Math.floor(this.audio.duration * 1000)
			: (this.current?.durationMs ?? 0);
		let bufferedEnd = 0;
		for (let index = 0; index < this.audio.buffered.length; index += 1) {
			bufferedEnd = Math.max(bufferedEnd, this.audio.buffered.end(index));
		}
		this.bufferedMs = Math.floor(bufferedEnd * 1000);
		if (this.durationMs > 0 && this.bufferedMs >= this.durationMs - 1_000)
			this.preloadUpcoming(2);
		if (Date.now() - this.lastSnapshotAt >= 5_000) this.persistSnapshot();
		this.syncSystemMediaSession();
	}

	onPlay() {
		this.playing = true;
		if (!this.startedAtMs) this.startedAtMs = Date.now();
		this.startListeningInterval();
		this.syncSystemMediaSession(true);
	}

	onPlaying() {
		this.clearBufferingTimer();
		if (this.error === "Buffering…") this.error = null;
		this.playing = true;
		this.startListeningInterval();
		this.syncSystemMediaSession(true);
	}

	onPause() {
		this.clearBufferingTimer();
		this.stopListeningInterval();
		this.playing = false;
		this.syncSystemMediaSession(true);
	}

	onWaiting() {
		this.stopListeningInterval();
		this.clearBufferingTimer();
		this.bufferingTimer = setTimeout(() => {
			if (this.audio && !this.audio.paused && !this.loading)
				this.error = "Buffering…";
		}, 1_500);
		this.stallRecoveryTimer = setTimeout(() => {
			if (this.audio && !this.audio.paused && !this.loading) {
				this.sourceRetryCount = 0;
				void this.handleMediaError();
			}
		}, 30_000);
	}

	onSeeking() {
		this.stopListeningInterval();
	}

	onSeeked() {
		this.sync();
		this.pendingSeekMs = null;
		this.persistSnapshot();
		if (this.audio && !this.audio.paused) this.startListeningInterval();
	}

	async ended() {
		if (!this.current || this.loading) return;
		this.sync();
		const expectedDuration = this.current.durationMs ?? this.durationMs;
		const completionTolerance = Math.max(3_000, expectedDuration * 0.05);
		if (
			expectedDuration > 0 &&
			this.positionMs + completionTolerance < expectedDuration
		) {
			this.sourceRetryCount = 0;
			this.error = "The stream ended early. Reconnecting…";
			await this.handleMediaError();
			return;
		}
		if (this.repeatOne && this.audio) {
			this.finalize("completed");
			this.beginSession();
			this.audio.currentTime = 0;
			this.positionMs = 0;
			await this.audio.play().catch((error) => {
				this.error = error instanceof Error ? error.message : String(error);
			});
			return;
		}
		const targetIndex = (this.currentIndex ?? -1) + 1;
		const target = this.queue[targetIndex];
		if (!target) return;
		this.finalize("completed");
		this.beginOptimisticSelection(target, targetIndex);
		try {
			await this.load(playNext, 0, 1);
		} catch {
			this.playing = false;
		}
	}

	async handleMediaError() {
		if (!this.current || this.loading) return;
		this.clearBufferingTimer();
		this.stopListeningInterval();
		const expectedId = this.current.id;
		const resumeAt =
			this.pendingSeekMs ??
			(this.audio
				? Math.floor(this.audio.currentTime * 1000)
				: this.positionMs);
		const maximumAttempts = 5;
		this.loading = true;
		try {
			while (
				this.sourceRetryCount < maximumAttempts &&
				this.current?.id === expectedId
			) {
				this.sourceRetryCount += 1;
				const attempt = this.sourceRetryCount;
				this.error = `Connection lost. Reconnecting ${attempt}/${maximumAttempts}…`;
				if (attempt > 1) {
					const delay = Math.min(8_000, 1_000 * 2 ** (attempt - 2));
					await new Promise((resolve) => setTimeout(resolve, delay));
				}
				try {
					const preparation = await refreshPlaybackSource();
					if (preparation.state.current?.id !== expectedId || !this.audio)
						return;
					this.replaceAudioSource(preparation.source.url);
					await this.restorePositionBeforePlaying(this.audio, resumeAt);
					await this.playWithTimeout(this.audio);
					this.sourceRetryCount = 0;
					this.error = null;
					return;
				} catch (error) {
					if (requiresYouTubeVerification(error)) {
						this.playing = false;
						this.sourceRetryCount = 0;
						this.error =
							"YouTube requires account verification. Connect YouTube Music in Settings, then press Play to retry this song.";
						return;
					}
					// A later attempt obtains a fresh URL and resumes the same song.
				}
			}
			this.playing = false;
			this.sourceRetryCount = 0;
			this.error =
				"The connection is still unavailable. Playback is paused; press Play to retry this song.";
		} finally {
			this.loading = false;
		}
	}

	private async initializeMediaControls() {
		if (!this.mediaControlsInitialized) {
			this.mediaControlsInitialized = true;

			if ("mediaSession" in navigator) {
				const handlers: Partial<
					Record<MediaSessionAction, MediaSessionActionHandler>
				> = {
					play: () => {
						if (this.audio?.paused) void this.toggle();
					},
					pause: () => {
						if (this.audio && !this.audio.paused) this.audio.pause();
					},
					previoustrack: () => void this.previous(),
					nexttrack: () => void this.next(),
					seekto: (details) => {
						if (details.seekTime !== undefined)
							this.seek(details.seekTime * 1000);
					},
					seekbackward: (details) =>
						this.seek(
							Math.max(0, this.positionMs - (details.seekOffset ?? 10) * 1000),
						),
					seekforward: (details) =>
						this.seek(
							Math.min(
								this.durationMs || Number.MAX_SAFE_INTEGER,
								this.positionMs + (details.seekOffset ?? 10) * 1000,
							),
						),
				};
				for (const [action, handler] of Object.entries(handlers)) {
					try {
						navigator.mediaSession.setActionHandler(
							action as MediaSessionAction,
							handler ?? null,
						);
					} catch {
						// Older engines expose Media Session but not every action.
					}
				}
			}
		}

		if (this.mediaControlListener || this.mediaControlRegistrationPending)
			return;
		this.mediaControlRegistrationPending = true;
		try {
			this.mediaControlListener = await addPluginListener<{
				action: "play" | "pause" | "next" | "previous" | "seek";
				positionMs?: number;
			}>("solmusic-storage", "media-control", (event) => {
				switch (event.action) {
					case "play":
						if (this.audio?.paused) void this.toggle();
						break;
					case "pause":
						if (this.audio && !this.audio.paused) this.audio.pause();
						break;
					case "next":
						void this.next();
						break;
					case "previous":
						void this.previous();
						break;
					case "seek":
						if (Number.isFinite(event.positionMs)) this.seek(event.positionMs!);
						break;
				}
			});
		} catch (error) {
			console.error("Could not register Android media-control listener", error);
			if (!this.mediaControlRetryTimer) {
				this.mediaControlRetryTimer = setTimeout(() => {
					this.mediaControlRetryTimer = null;
					void this.initializeMediaControls();
				}, 1_000);
			}
		} finally {
			this.mediaControlRegistrationPending = false;
		}
	}

	private syncSystemMediaSession(force = false) {
		const now = Date.now();
		if (!force && now - this.lastMediaSessionSyncAt < 1_000) return;
		this.lastMediaSessionSyncAt = now;
		const current = this.visibleCurrent;
		const duration = Math.max(
			0,
			this.visibleDurationMs || current?.durationMs || 0,
		);
		const visiblePosition = this.visiblePositionMs;
		const position = Math.max(
			0,
			duration > 0 ? Math.min(visiblePosition, duration) : visiblePosition,
		);
		const fingerprint = current
			? `${current.id}:${current.title}:${current.artistName}:${current.albumName ?? ""}:${current.thumbnailUrl ?? ""}`
			: "";

		if ("mediaSession" in navigator) {
			if (current && fingerprint !== this.lastMediaSessionFingerprint) {
				navigator.mediaSession.metadata = new MediaMetadata({
					title: current.title,
					artist: current.artistName,
					album: current.albumName ?? "SunnySong",
					artwork: current.thumbnailUrl ? [{ src: current.thumbnailUrl }] : [],
				});
			} else if (!current) navigator.mediaSession.metadata = null;
			navigator.mediaSession.playbackState = current
				? this.playing
					? "playing"
					: "paused"
				: "none";
			if (current && duration > 0) {
				try {
					navigator.mediaSession.setPositionState({
						duration: duration / 1000,
						playbackRate: 1,
						position: position / 1000,
					});
				} catch {
					// Metadata can arrive before the browser knows a valid duration.
				}
			}
		}
		this.lastMediaSessionFingerprint = fingerprint;

		void syncMediaSession({
			active: current !== null,
			title: current?.title ?? "",
			artist: current?.artistName ?? "",
			album: current?.albumName ?? null,
			artworkUrl: current?.thumbnailUrl ?? null,
			playing: this.playing,
			positionMs: Math.round(position),
			durationMs: Math.round(duration),
			canGoPrevious: (this.visibleCurrentIndex ?? 0) > 0 || position > 5_000,
			canGoNext:
				this.visibleCurrentIndex !== null &&
				this.visibleCurrentIndex + 1 < this.queue.length,
		}).catch(() => {
			// Desktop and normal browsers do not provide the Android native service.
		});
	}

	shutdown() {
		this.persistSnapshot();
		if (this.mediaControlRetryTimer) clearTimeout(this.mediaControlRetryTimer);
		this.mediaControlRetryTimer = null;
		this.mediaControlListener?.unregister();
		this.mediaControlListener = null;
		if ("mediaSession" in navigator) navigator.mediaSession.metadata = null;
		void syncMediaSession({
			active: false,
			title: "",
			artist: "",
			album: null,
			artworkUrl: null,
			playing: false,
			positionMs: 0,
			durationMs: 0,
			canGoPrevious: false,
			canGoNext: false,
		}).catch(() => {});
		void this.finalize("stopped");
	}

	private preloadUpcoming(maximum: number) {
		const currentId = this.current?.id;
		if (!currentId || this.warmedQueueForSongId === currentId) return;
		const start = (this.currentIndex ?? -1) + 1;
		const upcoming = this.queue.slice(
			start,
			start + Math.max(0, Math.min(2, maximum)),
		);
		if (!upcoming.length) return;
		this.warmedQueueForSongId = currentId;
		for (const song of upcoming) this.preload(song);
	}

	private beginOptimisticSelection(song: Song, index: number) {
		this.pendingSeekMs = null;
		if (!this.optimisticCurrent) {
			this.optimisticWasPlaying = Boolean(this.audio && !this.audio.paused);
		}
		if (this.audio && !this.audio.paused) this.audio.pause();
		this.optimisticCurrent = song;
		this.optimisticCurrentIndex = index >= 0 ? index : null;
		this.loading = true;
		this.error = null;
		this.syncSystemMediaSession(true);
	}

	private clearOptimisticSelection(resumePrevious: boolean) {
		const shouldResume = resumePrevious && this.optimisticWasPlaying;
		this.optimisticCurrent = null;
		this.optimisticCurrentIndex = null;
		this.optimisticWasPlaying = false;
		this.syncSystemMediaSession(true);
		if (shouldResume && this.audio?.src) {
			this.beginSession();
			void this.audio.play().catch(() => {});
		}
	}

	private async load(
		action: () => Promise<PlaybackPreparation>,
		resumeAtMs = 0,
		refillCount = 0,
	) {
		const generation = ++this.loadGeneration;
		this.loading = true;
		this.error = null;
		try {
			const preparation = await action();
			if (generation !== this.loadGeneration) return;
			const shouldHydrateQueue = preparation.state.queue.length <= 1;
			this.clearOptimisticSelection(false);
			this.applyState(preparation.state);
			this.positionMs = Math.max(0, resumeAtMs);
			this.durationMs = this.current?.durationMs ?? 0;
			this.beginSession();
			this.sourceRetryCount = 0;
			if (!this.audio) throw new Error("Audio player is not ready");
			this.replaceAudioSource(preparation.source.url);
			await this.restorePositionBeforePlaying(this.audio, resumeAtMs);
			if (generation !== this.loadGeneration) return;
			const songId = this.current?.id;
			if (songId && shouldHydrateQueue) void this.hydrateQueue(songId);
			else if (refillCount > 0) void this.refillQueue(refillCount);
			try {
				await this.playWithTimeout(this.audio);
			} catch (startupError) {
				if (generation !== this.loadGeneration || !songId) return;
				this.sourceRetryCount = 1;
				const refreshed = await refreshPlaybackSource();
				if (refreshed.state.current?.id !== songId) throw startupError;
				this.replaceAudioSource(refreshed.source.url);
				await this.restorePositionBeforePlaying(this.audio, resumeAtMs);
				if (generation !== this.loadGeneration) return;
				await this.playWithTimeout(this.audio);
			}
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.loadGeneration) {
				this.clearOptimisticSelection(true);
				this.error = error instanceof Error ? error.message : String(error);
			}
			throw error;
		} finally {
			if (generation === this.loadGeneration) this.loading = false;
		}
	}

	private applyState(state: PlaybackState) {
		if (state.current?.id !== this.current?.id) this.warmedQueueForSongId = "";
		this.current = state.current;
		this.queue = state.queue;
		this.currentIndex = state.currentIndex;
		this.syncSystemMediaSession(true);
	}

	private refillQueue(count: number) {
		this.pendingRefillCount += Math.max(0, Math.floor(count));
		if (this.refillPromise) return this.refillPromise;
		this.refillPromise = (async () => {
			while (this.pendingRefillCount > 0) {
				const requested = Math.min(20, this.pendingRefillCount);
				this.pendingRefillCount -= requested;
				const expectedSongId = this.current?.id;
				if (!expectedSongId) {
					this.pendingRefillCount = 0;
					break;
				}
				try {
					const state = await refillPlaybackQueue(requested);
					if (
						this.current?.id !== expectedSongId ||
						state.current?.id !== expectedSongId
					) {
						if (this.current) this.pendingRefillCount += requested;
						continue;
					}
					this.applyState(state);
					this.persistSnapshot();
				} catch {
					// Queue compensation is opportunistic and never interrupts playback.
				}
			}
		})().finally(() => {
			this.refillPromise = null;
			if (this.pendingRefillCount > 0) void this.refillQueue(0);
		});
		return this.refillPromise;
	}

	private async hydrateQueue(songId: string) {
		try {
			const state = await hydratePlaybackQueue(songId);
			if (this.current?.id !== songId || state.current?.id !== songId) return;
			this.applyState(state);
			this.persistSnapshot();
		} catch (error) {
			this.error = `Next Up unavailable: ${error instanceof Error ? error.message : String(error)}`;
		}
	}

	private replaceAudioSource(url: string) {
		if (!this.audio) return;
		this.bufferedMs = 0;
		this.stopListeningInterval();
		this.audio.pause();
		this.audio.removeAttribute("src");
		this.audio.load();
		this.audio.src = url;
		this.audio.load();
	}

	private async restorePositionBeforePlaying(
		audio: HTMLAudioElement,
		resumeAtMs: number,
	) {
		if (resumeAtMs <= 0) return;
		if (audio.readyState < HTMLMediaElement.HAVE_METADATA) {
			await new Promise<void>((resolve, reject) => {
				let timeout: ReturnType<typeof setTimeout> | undefined;
				const loaded = () => {
					cleanup();
					resolve();
				};
				const failed = () => {
					cleanup();
					reject(
						new Error(
							audio.error?.message || "Media metadata could not be loaded",
						),
					);
				};
				const cleanup = () => {
					if (timeout) clearTimeout(timeout);
					audio.removeEventListener("loadedmetadata", loaded);
					audio.removeEventListener("error", failed);
				};
				audio.addEventListener("loadedmetadata", loaded, { once: true });
				audio.addEventListener("error", failed, { once: true });
				timeout = setTimeout(() => {
					cleanup();
					reject(
						new Error("Timed out while restoring the saved playback position"),
					);
				}, 10_000);
			});
		}
		const maximum = Number.isFinite(audio.duration)
			? Math.max(0, audio.duration * 1000 - 1_000)
			: resumeAtMs;
		audio.currentTime = Math.min(resumeAtMs, maximum) / 1000;
		this.positionMs = Math.floor(audio.currentTime * 1000);
	}

	private async playWithTimeout(audio: HTMLAudioElement) {
		let timeout: ReturnType<typeof setTimeout> | undefined;
		let started: (() => void) | undefined;
		let failed: (() => void) | undefined;
		const playing = new Promise<void>((resolve, reject) => {
			started = () => resolve();
			failed = () =>
				reject(
					new Error(audio.error?.message || "Media failed while starting"),
				);
			audio.addEventListener("playing", started, { once: true });
			audio.addEventListener("error", failed, { once: true });
		});
		try {
			await audio.play();
			await Promise.race([
				playing,
				new Promise<never>((_, reject) => {
					timeout = setTimeout(
						() => reject(new Error("Playback timed out before audio started")),
						15_000,
					);
				}),
			]);
		} finally {
			if (timeout) clearTimeout(timeout);
			if (started) audio.removeEventListener("playing", started);
			if (failed) audio.removeEventListener("error", failed);
		}
	}

	private clearBufferingTimer() {
		if (this.bufferingTimer) clearTimeout(this.bufferingTimer);
		if (this.stallRecoveryTimer) clearTimeout(this.stallRecoveryTimer);
		this.bufferingTimer = null;
		this.stallRecoveryTimer = null;
	}

	private beginSession() {
		if (!this.profileId) return;
		this.eventId =
			globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.random()}`;
		this.sessionProfileId = this.profileId;
		this.startedAtMs = 0;
		this.listenedMs = 0;
		this.listeningSince = null;
	}

	private startListeningInterval() {
		if (this.eventId && this.listeningSince === null)
			this.listeningSince = performance.now();
	}

	private stopListeningInterval() {
		if (this.listeningSince === null) return;
		this.listenedMs += Math.max(0, performance.now() - this.listeningSince);
		this.listeningSince = null;
	}

	private finalize(reason: EndReason) {
		if (!this.current || !this.eventId || !this.sessionProfileId) return;
		this.stopListeningInterval();
		const eventId = this.eventId;
		const profileId = this.sessionProfileId;
		this.eventId = null;
		this.sessionProfileId = null;
		if (!this.startedAtMs) return;
		const duration = this.durationMs || this.current.durationMs;
		const summary: PlaybackSummary = {
			eventId,
			profileId,
			song: this.current,
			startedAtMs: this.startedAtMs,
			listenedMs: Math.max(0, Math.round(this.listenedMs)),
			durationMs: duration,
			reason,
		};
		if (!this.pendingSummaries.some((pending) => pending.eventId === eventId)) {
			this.pendingSummaries.push(summary);
			this.persistPending();
		}
		void this.flushPending();
	}

	private flushPending() {
		if (this.flushPromise) return this.flushPromise;
		this.flushPromise = (async () => {
			for (const summary of [...this.pendingSummaries]) {
				try {
					await reportPlayback(summary);
					this.pendingSummaries = this.pendingSummaries.filter(
						(pending) => pending.eventId !== summary.eventId,
					);
					this.persistPending();
				} catch {
					break;
				}
			}
		})().finally(() => {
			this.flushPromise = null;
		});
		return this.flushPromise;
	}

	private persistPending() {
		if (typeof localStorage !== "undefined") {
			localStorage.setItem(
				PENDING_SUMMARIES_KEY,
				JSON.stringify(this.pendingSummaries),
			);
		}
	}

	private persistSnapshot() {
		if (typeof localStorage === "undefined" || !this.current) return;
		this.lastSnapshotAt = Date.now();
		const snapshot: PlayerSnapshot = {
			current: this.current,
			queue: this.queue,
			currentIndex: this.currentIndex,
			positionMs: this.positionMs,
		};
		localStorage.setItem(PLAYER_SNAPSHOT_KEY, JSON.stringify(snapshot));
	}

	private readJson<T>(key: string, fallback: T): T {
		try {
			const raw = localStorage.getItem(key);
			return raw ? (JSON.parse(raw) as T) : fallback;
		} catch {
			return fallback;
		}
	}
}

export const player = new PlayerController();
