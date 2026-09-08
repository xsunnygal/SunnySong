import { addPluginListener, type PluginListener } from "@tauri-apps/api/core";
import {
	clearUpcoming as clearUpcomingBackend,
	enqueueNext,
	enqueueSong,
	getLikedSongIds,
	getPlaybackState,
	hydratePlaybackQueue,
	playNext,
	moveQueueItem as moveQueueItemBackend,
	playPrevious,
	playQueueItem,
	prepareNextPlaybackSource,
	preparePlayback,
	refreshPlaybackSource,
	refillPlaybackQueue,
	removeQueueItem as removeQueueItemBackend,
	replaceQueue as replaceQueueBackend,
	reportPlayback,
	setReaction,
	syncMediaSession,
	warmPlaybackSource,
	type PlaybackPreparation,
	type PlaybackSource,
	type PlaybackState,
	type PlaybackSummary,
	type Song,
} from "$lib/api/backend";
import { revisions } from "$lib/features/revisions.svelte";

type EndReason =
	"completed" | "next" | "previous" | "replaced" | "stopped" | "failed";

const PENDING_SUMMARIES_KEY = "solmusic-pending-summaries";
const PLAYER_SNAPSHOT_KEY = "solmusic-player-snapshot";
const PLAYER_VOLUME_KEY = "solmusic-player-volume";
const PLAYER_SLEEP_TIMER_KEY = "solmusic-player-sleep-timer";
const PLAYER_CROSSFADE_KEY = "solmusic-player-crossfade";
const PLAYER_NORMALIZATION_KEY = "solmusic-player-normalization";
const SLEEP_FADE_MS = 10_000;

type PlayerErrorKind =
	"network" | "source" | "codec" | "authorization" | "interrupted" | "unknown";
type SleepTimerMode = "off" | "duration" | "end-of-song" | "end-of-queue";
type CrossfadeDuration = 0 | 2 | 5 | 10;
type NormalizationMode = "off" | "track" | "album";

export interface NormalizationGainMetadata {
	trackGainDb?: number | null;
	albumGainDb?: number | null;
	peak?: number | null;
	trackPeak?: number | null;
	albumPeak?: number | null;
}

interface PersistedSleepTimer {
	mode: SleepTimerMode;
	expiresAtMs: number | null;
}

interface PlayerSnapshot extends PlaybackState {
	positionMs: number;
}

interface ClassifiedPlayerError {
	kind: PlayerErrorKind;
	message: string;
	retryable: boolean;
}

interface PreparedNextDeck {
	currentSongId: string;
	currentIndex: number;
	songId: string;
	source: PlaybackSource;
	audio: HTMLAudioElement;
}

function errorMessage(error: unknown) {
	return error instanceof Error ? error.message : String(error);
}

function classifyBackendError(error: unknown): ClassifiedPlayerError {
	const raw = errorMessage(error).trim();
	const message = raw.toLowerCase();
	if (requiresYouTubeVerification(error)) {
		return {
			kind: "authorization",
			message:
				"YouTube needs account verification. Connect YouTube Music in Settings, then press Play to retry.",
			retryable: false,
		};
	}
	if (
		/network|connection|offline|timed? out|timeout|failed to fetch|dns|socket|econn|503|502|gateway/.test(
			message,
		)
	) {
		return {
			kind: "network",
			message:
				"The audio service is unreachable. Check your connection and retry.",
			retryable: true,
		};
	}
	if (
		/codec|decode|demux|media format|unsupported format|not supported/.test(
			message,
		)
	) {
		return {
			kind: "codec",
			message: "This audio format cannot be decoded on this device.",
			retryable: false,
		};
	}
	if (
		/not found|unavailable|private|removed|region|blocked|no (playback )?source|invalid (source|url)|403|404/.test(
			message,
		)
	) {
		return {
			kind: "source",
			message: "This song is unavailable from its current source.",
			retryable: false,
		};
	}
	if (
		/unauthori[sz]ed|forbidden|permission|sign[ -]?in|authentication/.test(
			message,
		)
	) {
		return {
			kind: "authorization",
			message:
				"Playback authorization expired. Reconnect the music service in Settings.",
			retryable: false,
		};
	}
	if (/abort|cancel|interrupted|notallowederror|user gesture/.test(message)) {
		return {
			kind: "interrupted",
			message: "Playback was interrupted. Press Play to continue.",
			retryable: false,
		};
	}
	return {
		kind: "unknown",
		message: raw
			? `Could not start playback: ${raw}`
			: "Could not start playback. Please retry.",
		retryable: false,
	};
}

function classifyMediaError(
	mediaError: MediaError | null,
): ClassifiedPlayerError {
	switch (mediaError?.code) {
		case MediaError.MEDIA_ERR_NETWORK:
			return {
				kind: "network",
				message: "The audio connection was interrupted.",
				retryable: true,
			};
		case MediaError.MEDIA_ERR_DECODE:
			return {
				kind: "codec",
				message: "This song could not be decoded on this device.",
				retryable: false,
			};
		case MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED:
			return {
				kind: "source",
				message: "This song's audio source or format is not supported.",
				retryable: false,
			};
		case MediaError.MEDIA_ERR_ABORTED:
			return {
				kind: "interrupted",
				message: "Audio loading was interrupted. Press Play to retry.",
				retryable: false,
			};
		default:
			return {
				kind: "unknown",
				message: mediaError?.message
					? `Playback failed: ${mediaError.message}`
					: "Playback failed. Press Play to retry.",
				retryable: false,
			};
	}
}

function mediaSessionItem(song: Song) {
	return {
		id: song.id,
		title: song.title,
		artist: song.artistName,
		album: song.albumName,
		artworkUrl: song.thumbnailUrl,
		durationMs: song.durationMs ?? 0,
	};
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
	errorKind = $state<PlayerErrorKind | null>(null);
	likedSongIds = $state<string[]>([]);
	repeatMode = $state<"off" | "all" | "one">("off");
	volume = $state(1);
	muted = $state(false);
	sleepTimerMode = $state<SleepTimerMode>("off");
	sleepTimerExpiresAtMs = $state<number | null>(null);
	sleepTimerRemainingMs = $state<number | null>(0);
	crossfadeDuration = $state<CrossfadeDuration>(0);
	normalizationMode = $state<NormalizationMode>("off");
	normalizationGainDb = $state(0);
	normalizationMetadataAvailable = $state(false);
	private optimisticCurrent = $state<Song | null>(null);
	private optimisticCurrentIndex = $state<number | null>(null);
	private optimisticWasPlaying = false;
	private audio: HTMLAudioElement | null = null;
	private standbyAudio: HTMLAudioElement | null = null;
	private deckFadeLevels = new WeakMap<HTMLAudioElement, number>();
	private deckNormalizationLevels = new WeakMap<HTMLAudioElement, number>();
	private normalizationMetadata = new Map<string, NormalizationGainMetadata>();
	private crossfadeInProgress = false;
	private crossfadeGeneration = 0;
	private sleepTimerInterval: ReturnType<typeof setInterval> | null = null;
	private sleepFadeLevel = 1;
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
	private queueEditGeneration = 0;
	private activeAudioSongId: string | null = null;
	private preparedNextDeck: PreparedNextDeck | null = null;
	private preparingNextKey = "";
	private nextPreparationGeneration = 0;
	private nextPreparationRetryKey = "";
	private nextPreparationRetryAtMs = 0;

	get repeatOne() {
		return this.repeatMode === "one";
	}

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
		const storedVolume = Number(localStorage.getItem(PLAYER_VOLUME_KEY) ?? "1");
		this.volume = Number.isFinite(storedVolume)
			? Math.max(0, Math.min(1, storedVolume))
			: 1;
		this.crossfadeDuration = this.readStoredCrossfade();
		this.normalizationMode = this.readStoredNormalizationMode();
		this.restoreSleepTimer();
		this.audio = this.createAudioDeck();
		if (this.sleepTimerActive) this.startSleepTimerClock();
	}

	private createAudioDeck() {
		const audio = new Audio();
		audio.preload = "auto";
		audio.volume = this.volume;
		audio.muted = this.muted;
		this.deckFadeLevels.set(audio, 1);
		this.deckNormalizationLevels.set(audio, 1);
		audio.addEventListener("timeupdate", () => {
			if (audio === this.audio) this.sync();
		});
		audio.addEventListener("durationchange", () => {
			if (audio === this.audio) this.sync();
		});
		audio.addEventListener("progress", () => {
			if (audio === this.audio) this.sync();
		});
		audio.addEventListener("canplaythrough", () => {
			if (audio === this.audio) this.preloadUpcoming(2);
		});
		audio.addEventListener("play", () => {
			if (audio === this.audio) this.onPlay();
		});
		audio.addEventListener("playing", () => {
			if (audio === this.audio) this.onPlaying();
		});
		audio.addEventListener("pause", () => {
			if (audio === this.audio) this.onPause();
		});
		audio.addEventListener("waiting", () => {
			if (audio === this.audio) this.onWaiting();
		});
		audio.addEventListener("seeking", () => {
			if (audio === this.audio) this.onSeeking();
		});
		audio.addEventListener("seeked", () => {
			if (audio === this.audio) this.onSeeked();
		});
		audio.addEventListener("ended", () => {
			if (audio === this.audio) void this.ended();
		});
		audio.addEventListener("error", () => {
			if (audio === this.audio) void this.handleMediaError();
		});
		return audio;
	}

	async initialize(profileId?: string) {
		this.ensureAudio();
		if (profileId) this.profileId = profileId;
		if (this.initialized || typeof localStorage === "undefined") {
			void this.initializeMediaControls();
			return;
		}
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
		await this.initializeMediaControls();
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
		this.queueEditGeneration += 1;
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
		this.queueEditGeneration += 1;
		if (this.current?.id === song.id) {
			if (this.audio?.paused) await this.toggle();
			return;
		}
		if (!this.queue.some((item) => item.id === song.id)) {
			this.errorKind = "source";
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
		if (this.crossfadeInProgress) return;
		this.queueEditGeneration += 1;
		let targetIndex = (this.currentIndex ?? -1) + 1;
		if (targetIndex >= this.queue.length && this.repeatMode === "all")
			targetIndex = 0;
		const target = this.queue[targetIndex];
		if (!target) return;
		if (reason !== "completed") this.finalize(reason);
		this.beginOptimisticSelection(target, targetIndex);
		try {
			await this.load(
				targetIndex === 0 ? () => playQueueItem(target.id) : playNext,
				0,
				targetIndex === 0 || this.repeatMode === "all" ? 0 : 1,
			);
		} catch {
			this.playing = false;
		}
	}

	cycleRepeat() {
		this.repeatMode =
			this.repeatMode === "off"
				? "all"
				: this.repeatMode === "all"
					? "one"
					: "off";
	}

	toggleRepeatOne() {
		this.repeatMode = this.repeatMode === "one" ? "off" : "one";
	}

	setVolume(value: number) {
		const next = Math.max(0, Math.min(1, value));
		this.volume = next;
		this.muted = false;
		for (const audio of [this.audio, this.standbyAudio]) {
			if (audio) {
				audio.volume = next;
				audio.muted = false;
			}
		}
		if (typeof localStorage !== "undefined")
			localStorage.setItem(PLAYER_VOLUME_KEY, String(next));
	}

	toggleMute() {
		this.muted = !this.muted;
		for (const audio of [this.audio, this.standbyAudio]) {
			if (audio) audio.muted = this.muted;
		}
	}

	get sleepTimerActive() {
		return this.sleepTimerMode !== "off";
	}

	setSleepTimer(minutes: 15 | 30 | 45 | 60) {
		this.sleepTimerMode = "duration";
		this.sleepTimerExpiresAtMs = Date.now() + minutes * 60_000;
		this.sleepTimerRemainingMs = minutes * 60_000;
		this.sleepFadeLevel = 1;
		this.persistSleepTimer();
		this.startSleepTimerClock();
		this.updateDeckGains();
	}

	setSleepTimer15Minutes() {
		this.setSleepTimer(15);
	}

	setSleepTimer30Minutes() {
		this.setSleepTimer(30);
	}

	setSleepTimer45Minutes() {
		this.setSleepTimer(45);
	}

	setSleepTimer60Minutes() {
		this.setSleepTimer(60);
	}

	setSleepTimerEndOfSong() {
		this.sleepTimerMode = "end-of-song";
		this.sleepTimerExpiresAtMs = null;
		this.sleepTimerRemainingMs =
			this.durationMs > 0
				? Math.max(0, this.durationMs - this.positionMs)
				: null;
		this.sleepFadeLevel = 1;
		this.persistSleepTimer();
		this.startSleepTimerClock();
	}

	setSleepTimerEndOfQueue() {
		this.sleepTimerMode = "end-of-queue";
		this.sleepTimerExpiresAtMs = null;
		this.sleepTimerRemainingMs = this.estimateQueueRemainingMs();
		this.sleepFadeLevel = 1;
		this.persistSleepTimer();
		this.startSleepTimerClock();
	}

	cancelSleepTimer() {
		this.sleepTimerMode = "off";
		this.sleepTimerExpiresAtMs = null;
		if (this.sleepTimerInterval) clearInterval(this.sleepTimerInterval);
		this.sleepTimerInterval = null;
		this.sleepTimerRemainingMs = 0;
		this.sleepFadeLevel = 1;
		this.persistSleepTimer();
		this.updateDeckGains();
	}

	setCrossfadeDuration(seconds: CrossfadeDuration) {
		if (![0, 2, 5, 10].includes(seconds)) return;
		this.crossfadeDuration = seconds;
		if (typeof localStorage !== "undefined")
			localStorage.setItem(PLAYER_CROSSFADE_KEY, String(seconds));
	}

	setCrossfadeOff() {
		this.setCrossfadeDuration(0);
	}

	setCrossfade2Seconds() {
		this.setCrossfadeDuration(2);
	}

	setCrossfade5Seconds() {
		this.setCrossfadeDuration(5);
	}

	setCrossfade10Seconds() {
		this.setCrossfadeDuration(10);
	}

	setNormalizationMode(mode: NormalizationMode) {
		this.normalizationMode = mode;
		if (typeof localStorage !== "undefined")
			localStorage.setItem(PLAYER_NORMALIZATION_KEY, mode);
		this.applyNormalizationForCurrentSong();
	}

	setNormalizationOff() {
		this.setNormalizationMode("off");
	}

	setTrackNormalization() {
		this.setNormalizationMode("track");
	}

	setAlbumNormalization() {
		this.setNormalizationMode("album");
	}

	setNormalizationGainMetadata(
		songId: string,
		metadata: NormalizationGainMetadata | null,
	) {
		if (metadata) this.normalizationMetadata.set(songId, metadata);
		else this.normalizationMetadata.delete(songId);
		if (this.current?.id === songId) this.applyNormalizationForCurrentSong();
	}

	async playNextSong(song: Song) {
		const generation = ++this.queueEditGeneration;
		try {
			const state = await enqueueNext(song);
			if (generation !== this.queueEditGeneration) return;
			this.applyState(state);
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.queueEditGeneration) this.setBackendError(error);
		}
	}

	async addToQueue(song: Song) {
		const generation = ++this.queueEditGeneration;
		try {
			const state = await enqueueSong(song);
			if (generation !== this.queueEditGeneration) return;
			this.applyState(state);
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.queueEditGeneration) this.setBackendError(error);
		}
	}

	async removeQueueItem(index: number) {
		const generation = ++this.queueEditGeneration;
		try {
			const state = await removeQueueItemBackend(index);
			if (generation !== this.queueEditGeneration) return;
			this.applyState(state);
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.queueEditGeneration) this.setBackendError(error);
		}
	}

	async moveQueueItem(fromIndex: number, toIndex: number) {
		if (toIndex < 0 || toIndex >= this.queue.length || fromIndex === toIndex)
			return;
		const generation = ++this.queueEditGeneration;
		try {
			const state = await moveQueueItemBackend(fromIndex, toIndex);
			if (generation !== this.queueEditGeneration) return;
			this.applyState(state);
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.queueEditGeneration) this.setBackendError(error);
		}
	}

	async clearUpcoming() {
		const generation = ++this.queueEditGeneration;
		try {
			const state = await clearUpcomingBackend();
			if (generation !== this.queueEditGeneration) return;
			this.applyState(state);
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.queueEditGeneration) this.setBackendError(error);
		}
	}

	async shuffleUpcoming() {
		const currentId = this.current?.id;
		const currentIndex = this.currentIndex;
		if (
			!currentId ||
			currentIndex === null ||
			currentIndex + 1 >= this.queue.length
		)
			return;
		const generation = ++this.queueEditGeneration;
		const desired = [...this.queue.slice(currentIndex + 1)];
		for (let index = desired.length - 1; index > 0; index -= 1) {
			const target = Math.floor(Math.random() * (index + 1));
			[desired[index], desired[target]] = [desired[target], desired[index]];
		}
		let working = [...this.queue];
		try {
			for (let offset = 0; offset < desired.length; offset += 1) {
				if (
					generation !== this.queueEditGeneration ||
					this.current?.id !== currentId ||
					this.currentIndex !== currentIndex
				)
					return;
				const targetIndex = currentIndex + 1 + offset;
				const fromIndex = working.findIndex(
					(song, index) =>
						index >= targetIndex && song.id === desired[offset].id,
				);
				if (fromIndex < 0 || fromIndex === targetIndex) continue;
				const state = await moveQueueItemBackend(fromIndex, targetIndex);
				if (
					generation !== this.queueEditGeneration ||
					state.current?.id !== currentId ||
					state.currentIndex !== currentIndex
				)
					return;
				working = state.queue;
				this.applyState(state);
			}
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.queueEditGeneration) this.setBackendError(error);
		}
	}

	async replaceQueue(songs: Song[], startIndex = 0) {
		if (!songs.length) return;
		this.queueEditGeneration += 1;
		this.finalize("replaced");
		this.beginOptimisticSelection(songs[startIndex] ?? songs[0], startIndex);
		try {
			await this.load(() => replaceQueueBackend(songs, startIndex));
		} catch {
			// The player error state is rendered globally.
		}
	}

	async playAll(songs: Song[]) {
		await this.replaceQueue(songs, 0);
	}

	async shuffle(songs: Song[]) {
		const shuffled = [...songs];
		for (let index = shuffled.length - 1; index > 0; index -= 1) {
			const target = Math.floor(Math.random() * (index + 1));
			[shuffled[index], shuffled[target]] = [shuffled[target], shuffled[index]];
		}
		await this.replaceQueue(shuffled, 0);
	}

	async previous() {
		this.queueEditGeneration += 1;
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
				this.setPlaybackError(error);
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
			revisions.tasteChanged();
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
		this.prepareNextDeckIfReady();
		if (Date.now() - this.lastSnapshotAt >= 5_000) this.persistSnapshot();
		this.updateSleepTimer();
		this.maybeStartCrossfade();
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
		if (this.error === "Buffering…") {
			this.error = null;
			this.errorKind = null;
		}
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
			if (this.audio && !this.audio.paused && !this.loading) {
				this.errorKind = "network";
				this.error = "Buffering…";
			}
		}, 1_500);
		this.stallRecoveryTimer = setTimeout(() => {
			if (this.audio && !this.audio.paused && !this.loading) {
				this.sourceRetryCount = 0;
				void this.handleMediaError(undefined, true);
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
		if (!this.current || this.loading || this.crossfadeInProgress) return;
		this.sync();
		const expectedDuration = this.current.durationMs ?? this.durationMs;
		const completionTolerance = Math.max(3_000, expectedDuration * 0.05);
		if (
			expectedDuration > 0 &&
			this.positionMs + completionTolerance < expectedDuration
		) {
			this.sourceRetryCount = 0;
			this.errorKind = "network";
			this.error = "The stream ended early. Reconnecting…";
			await this.handleMediaError(undefined, true);
			return;
		}
		const nextIndex = (this.currentIndex ?? -1) + 1;
		const sleepAtThisBoundary =
			this.sleepTimerMode === "end-of-song" ||
			(this.sleepTimerMode === "end-of-queue" &&
				nextIndex >= this.queue.length);
		if (sleepAtThisBoundary) {
			this.finalize("completed");
			this.cancelSleepTimer();
			this.playing = false;
			this.syncSystemMediaSession(true);
			return;
		}
		if (this.repeatOne && this.audio) {
			this.finalize("completed");
			this.beginSession();
			this.audio.currentTime = 0;
			this.positionMs = 0;
			await this.audio.play().catch((error) => {
				this.setPlaybackError(error);
			});
			return;
		}
		let targetIndex = (this.currentIndex ?? -1) + 1;
		if (targetIndex >= this.queue.length && this.repeatMode === "all")
			targetIndex = 0;
		const target = this.queue[targetIndex];
		if (!target) return;
		if (await this.startPreparedDeckTransition(targetIndex, 0)) return;
		this.finalize("completed");
		this.beginOptimisticSelection(target, targetIndex);
		try {
			this.queueEditGeneration += 1;
			await this.load(
				targetIndex === 0 ? () => playQueueItem(target.id) : playNext,
				0,
				targetIndex === 0 || this.repeatMode === "all" ? 0 : 1,
			);
		} catch {
			this.playing = false;
		}
	}

	async handleMediaError(error?: unknown, knownNetworkOrStall = false) {
		if (!this.current || this.loading) return;
		this.clearBufferingTimer();
		this.stopListeningInterval();
		let classified = knownNetworkOrStall
			? {
					kind: "network" as const,
					message: "The audio connection stalled.",
					retryable: true,
				}
			: error
				? classifyBackendError(error)
				: classifyMediaError(this.audio?.error ?? null);
		if (!classified.retryable) {
			this.playing = false;
			this.sourceRetryCount = 0;
			this.errorKind = classified.kind;
			this.error = classified.message;
			return;
		}
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
				this.errorKind = "network";
				this.error = `Audio connection interrupted. Reconnecting ${attempt}/${maximumAttempts}…`;
				if (attempt > 1) {
					const delay = Math.min(8_000, 1_000 * 2 ** (attempt - 2));
					await new Promise((resolve) => setTimeout(resolve, delay));
				}
				try {
					const preparation = await refreshPlaybackSource();
					if (preparation.state.current?.id !== expectedId || !this.audio)
						return;
					this.feedNormalizationMetadata(preparation);
					this.replaceAudioSource(preparation.source.url);
					await this.restorePositionBeforePlaying(this.audio, resumeAt);
					await this.playWithTimeout(this.audio);
					this.sourceRetryCount = 0;
					this.error = null;
					this.errorKind = null;
					return;
				} catch (retryError) {
					classified = this.classifyPlaybackFailure(retryError);
					if (!classified.retryable) {
						this.playing = false;
						this.sourceRetryCount = 0;
						this.errorKind = classified.kind;
						this.error = classified.message;
						return;
					}
				}
			}
			this.playing = false;
			this.sourceRetryCount = 0;
			this.errorKind = "network";
			this.error =
				"The audio service is still unreachable. Playback is paused; press Play to retry.";
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
				action:
					| "play"
					| "pause"
					| "next"
					| "previous"
					| "seek"
					| "stop"
					| "play-from-media-id";
				positionMs?: number;
				mediaId?: string;
				songId?: string;
				queueIndex?: number;
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
					case "play-from-media-id": {
						const target = this.queue.find((song) => song.id === event.songId);
						const current = this.current;
						if (target) {
							void this.playQueuedSong(target);
							break;
						}
						if (current === null || current.id !== event.songId) break;
						void this.playSong(current);
						break;
					}
					case "stop":
						this.audio?.pause();
						this.playing = false;
						void this.syncInactiveMediaSession();
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
				(this.visibleCurrentIndex + 1 < this.queue.length ||
					(this.repeatMode === "all" && this.queue.length > 0)),
			queue: this.queue.slice(0, 40).map(mediaSessionItem),
			currentItem: current ? mediaSessionItem(current) : null,
			currentIndex: this.visibleCurrentIndex,
		}).catch(() => {
			// Desktop and normal browsers do not provide the Android native service.
		});
	}

	private syncInactiveMediaSession() {
		const current = this.visibleCurrent;
		return syncMediaSession({
			active: false,
			title: "",
			artist: "",
			album: null,
			artworkUrl: null,
			playing: false,
			positionMs: Math.round(this.visiblePositionMs),
			durationMs: Math.round(this.visibleDurationMs),
			canGoPrevious: false,
			canGoNext: false,
			queue: this.queue.slice(0, 40).map(mediaSessionItem),
			currentItem: current ? mediaSessionItem(current) : null,
			currentIndex: this.visibleCurrentIndex,
		}).catch(() => {
			// The media browser persists the last resumable snapshot when available.
		});
	}

	shutdown() {
		this.persistSnapshot();
		this.crossfadeGeneration += 1;
		if (this.sleepTimerInterval) clearInterval(this.sleepTimerInterval);
		this.sleepTimerInterval = null;
		this.audio?.pause();
		this.standbyAudio?.pause();
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
			queue: [],
			currentItem: null,
			currentIndex: null,
		}).catch(() => {});
		void this.finalize("stopped");
	}

	private readStoredCrossfade(): CrossfadeDuration {
		const value = Number(localStorage.getItem(PLAYER_CROSSFADE_KEY) ?? "0");
		return value === 2 || value === 5 || value === 10 ? value : 0;
	}

	private readStoredNormalizationMode(): NormalizationMode {
		const value = localStorage.getItem(PLAYER_NORMALIZATION_KEY);
		return value === "track" || value === "album" ? value : "off";
	}

	private restoreSleepTimer() {
		const stored = this.readJson<PersistedSleepTimer>(PLAYER_SLEEP_TIMER_KEY, {
			mode: "off",
			expiresAtMs: null,
		});
		if (
			stored.mode === "duration" &&
			(!stored.expiresAtMs || stored.expiresAtMs <= Date.now())
		) {
			this.cancelSleepTimer();
			return;
		}
		if (
			!["off", "duration", "end-of-song", "end-of-queue"].includes(stored.mode)
		)
			return;
		this.sleepTimerMode = stored.mode;
		this.sleepTimerExpiresAtMs = stored.expiresAtMs;
		this.updateSleepTimer();
	}

	private persistSleepTimer() {
		if (typeof localStorage === "undefined") return;
		const timer: PersistedSleepTimer = {
			mode: this.sleepTimerMode,
			expiresAtMs: this.sleepTimerExpiresAtMs,
		};
		localStorage.setItem(PLAYER_SLEEP_TIMER_KEY, JSON.stringify(timer));
	}

	private startSleepTimerClock() {
		if (this.sleepTimerInterval || typeof window === "undefined") return;
		this.sleepTimerInterval = setInterval(() => this.updateSleepTimer(), 250);
	}

	private updateSleepTimer() {
		let remaining: number | null = 0;
		if (this.sleepTimerMode === "duration") {
			remaining = Math.max(0, (this.sleepTimerExpiresAtMs ?? 0) - Date.now());
		} else if (this.sleepTimerMode === "end-of-song") {
			remaining =
				this.durationMs > 0
					? Math.max(0, this.durationMs - this.positionMs)
					: null;
		} else if (this.sleepTimerMode === "end-of-queue") {
			remaining = this.estimateQueueRemainingMs();
		}
		this.sleepTimerRemainingMs = remaining;
		const nextFade =
			this.sleepTimerMode !== "off" &&
			remaining !== null &&
			remaining >= 0 &&
			remaining <= SLEEP_FADE_MS
				? remaining / SLEEP_FADE_MS
				: 1;
		if (Math.abs(nextFade - this.sleepFadeLevel) > 0.01) {
			this.sleepFadeLevel = nextFade;
			this.updateDeckGains(250);
		}
		if (this.sleepTimerMode === "duration" && remaining === 0) {
			this.audio?.pause();
			this.standbyAudio?.pause();
			this.cancelSleepTimer();
			this.syncSystemMediaSession(true);
		}
	}

	private estimateQueueRemainingMs(): number | null {
		if (!this.current || this.currentIndex === null) return null;
		const currentDuration = this.durationMs || this.current.durationMs;
		if (!currentDuration) return null;
		let remaining = Math.max(0, currentDuration - this.positionMs);
		for (const song of this.queue.slice(this.currentIndex + 1)) {
			if (!song.durationMs) return null;
			remaining += song.durationMs;
		}
		return remaining;
	}

	private metadataForSong(song: Song | null) {
		return song ? (this.normalizationMetadata.get(song.id) ?? null) : null;
	}

	private applyNormalizationForCurrentSong() {
		const metadata = this.metadataForSong(this.current);
		const requested =
			this.normalizationMode === "track"
				? metadata?.trackGainDb
				: this.normalizationMode === "album"
					? metadata?.albumGainDb
					: 0;
		this.normalizationMetadataAvailable =
			this.normalizationMode !== "off" && Number.isFinite(requested);
		if (!this.normalizationMetadataAvailable) {
			this.normalizationGainDb = 0;
			if (this.audio) this.deckNormalizationLevels.set(this.audio, 1);
			this.updateDeckGains();
			return;
		}
		this.normalizationGainDb = Math.max(-24, Math.min(requested!, 0));
		if (this.audio)
			this.deckNormalizationLevels.set(
				this.audio,
				10 ** (this.normalizationGainDb / 20),
			);
		this.updateDeckGains(100);
	}

	private updateDeckGains(_rampMs = 0) {
		for (const audio of [this.audio, this.standbyAudio]) {
			if (!audio) continue;
			const fade = this.deckFadeLevels.get(audio) ?? 1;
			const normalization = this.deckNormalizationLevels.get(audio) ?? 1;
			const level = fade * this.sleepFadeLevel * normalization;
			audio.volume = Math.max(0, Math.min(1, this.volume * level));
		}
	}

	private setDeckFade(
		audio: HTMLAudioElement,
		level: number,
		durationMs: number,
	) {
		this.deckFadeLevels.set(audio, Math.max(0, Math.min(1, level)));
		const normalization = this.deckNormalizationLevels.get(audio) ?? 1;
		const target = level * this.sleepFadeLevel * normalization;
		const startedAt = performance.now();
		const start = audio.volume;
		const end = Math.max(0, Math.min(1, this.volume * target));
		const tick = () => {
			const progress = Math.min(
				1,
				(performance.now() - startedAt) / durationMs,
			);
			audio.volume = start + (end - start) * progress;
			if (progress < 1 && !audio.paused) requestAnimationFrame(tick);
		};
		requestAnimationFrame(tick);
	}

	private maybeStartCrossfade() {
		const audio = this.audio;
		if (
			this.crossfadeDuration === 0 ||
			this.crossfadeInProgress ||
			this.loading ||
			!audio ||
			audio.paused ||
			!Number.isFinite(audio.duration) ||
			this.sleepTimerMode === "end-of-song" ||
			this.repeatOne
		)
			return;
		const remaining = audio.duration - audio.currentTime;
		if (remaining <= 0 || remaining > this.crossfadeDuration) return;
		let targetIndex = (this.currentIndex ?? -1) + 1;
		if (
			this.sleepTimerMode === "end-of-queue" &&
			targetIndex >= this.queue.length
		)
			return;
		if (targetIndex >= this.queue.length && this.repeatMode === "all")
			targetIndex = 0;
		if (!this.queue[targetIndex]) return;
		void this.startCrossfade(targetIndex);
	}

	private async startPreparedDeckTransition(
		targetIndex: number,
		fadeDurationMs: number,
	) {
		const prepared = this.takePreparedNextDeck(targetIndex);
		const oldAudio = this.audio;
		const oldSong = this.current;
		const target = this.queue[targetIndex];
		if (!prepared || !oldAudio || !oldSong || !target) return false;

		this.crossfadeInProgress = true;
		const generation = ++this.crossfadeGeneration;
		const queueGeneration = ++this.queueEditGeneration;
		const reconciliation = playNext();
		const nextAudio = prepared.audio;
		this.finalize("completed");
		this.standbyAudio = oldAudio;
		this.deckFadeLevels.set(oldAudio, 1);
		this.deckFadeLevels.set(nextAudio, fadeDurationMs > 0 ? 0 : 1);
		this.deckNormalizationLevels.set(
			nextAudio,
			this.normalizationLevelForSong(target),
		);
		this.audio = nextAudio;
		this.activeAudioSongId = target.id;
		this.current = target;
		this.currentIndex = targetIndex;
		this.positionMs = 0;
		this.durationMs = target.durationMs ?? 0;
		this.bufferedMs = 0;
		this.beginSession();
		this.applyNormalizationForCurrentSong();
		this.updateDeckGains();
		this.syncSystemMediaSession(true);

		try {
			await this.playWithTimeout(nextAudio);
			if (fadeDurationMs > 0) {
				this.setDeckFade(oldAudio, 0, fadeDurationMs);
				this.setDeckFade(nextAudio, 1, fadeDurationMs);
			}

			let preparation: PlaybackPreparation | null = null;
			try {
				preparation = await reconciliation;
			} catch (error) {
				const state = await getPlaybackState().catch(() => null);
				if (state?.current?.id === target.id) this.applyState(state);
				else throw error;
			}
			if (preparation && preparation.state.current?.id !== target.id)
				throw new Error("The queue changed while the next deck was starting");
			if (
				preparation &&
				generation === this.crossfadeGeneration &&
				queueGeneration === this.queueEditGeneration
			) {
				this.feedNormalizationMetadata(preparation);
				this.applyState(preparation.state);
			}
			this.error = null;
			this.errorKind = null;
			this.persistSnapshot();
			if (targetIndex > 0) void this.refillQueue(1);
			if (fadeDurationMs > 0) {
				await new Promise((resolve) => setTimeout(resolve, fadeDurationMs));
				if (generation !== this.crossfadeGeneration) return true;
			}
			oldAudio.pause();
			oldAudio.removeAttribute("src");
			oldAudio.load();
			this.deckFadeLevels.set(oldAudio, 1);
			this.standbyAudio = oldAudio;
		} catch (error) {
			const reconciled = await reconciliation.catch(() => null);
			const state =
				reconciled?.state ?? (await getPlaybackState().catch(() => null));
			if (state?.current?.id === target.id) {
				this.applyState(state);
				await this.handleMediaError(error);
			} else {
				nextAudio.pause();
				nextAudio.removeAttribute("src");
				nextAudio.load();
				this.finalize("failed");
				this.audio = oldAudio;
				this.activeAudioSongId = oldSong.id;
				this.standbyAudio = nextAudio;
				this.deckFadeLevels.set(oldAudio, 1);
				this.updateDeckGains();
				if (state) this.applyState(state);
				this.setBackendError(error);
			}
		} finally {
			if (generation === this.crossfadeGeneration) {
				this.crossfadeInProgress = false;
				this.prepareNextDeckIfReady();
			}
		}
		return true;
	}

	private async startCrossfade(targetIndex: number) {
		if (
			await this.startPreparedDeckTransition(
				targetIndex,
				this.crossfadeDuration * 1000,
			)
		)
			return;
		const oldAudio = this.audio;
		const oldSongId = this.current?.id;
		if (!oldAudio || !oldSongId || this.crossfadeInProgress) return;
		this.crossfadeInProgress = true;
		const generation = ++this.crossfadeGeneration;
		const queueGeneration = ++this.queueEditGeneration;
		const durationMs = this.crossfadeDuration * 1000;
		let preparation: PlaybackPreparation;
		try {
			preparation = await (targetIndex === 0
				? playQueueItem(this.queue[targetIndex].id)
				: playNext());
		} catch (error) {
			if (generation === this.crossfadeGeneration) this.setBackendError(error);
			this.crossfadeInProgress = false;
			return;
		}
		if (
			generation !== this.crossfadeGeneration ||
			queueGeneration !== this.queueEditGeneration ||
			this.audio !== oldAudio ||
			this.current?.id !== oldSongId
		) {
			this.crossfadeInProgress = false;
			return;
		}
		this.feedNormalizationMetadata(preparation);
		const nextAudio = this.standbyAudio ?? this.createAudioDeck();
		this.standbyAudio = oldAudio;
		this.deckFadeLevels.set(oldAudio, 1);
		this.deckFadeLevels.set(nextAudio, 0);
		this.replaceAudioSource(preparation.source.url, nextAudio);
		this.finalize("completed");
		this.audio = nextAudio;
		this.activeAudioSongId = preparation.state.current?.id ?? null;
		this.applyState(preparation.state);
		this.positionMs = 0;
		this.durationMs = this.current?.durationMs ?? 0;
		this.beginSession();
		this.updateDeckGains();
		try {
			await this.playWithTimeout(nextAudio);
			this.onPlay();
			this.onPlaying();
			this.setDeckFade(oldAudio, 0, durationMs);
			this.setDeckFade(nextAudio, 1, durationMs);
			this.persistSnapshot();
			const songId = this.current?.id;
			if (songId && preparation.state.queue.length <= 1)
				void this.hydrateQueue(songId);
			else if (targetIndex > 0) void this.refillQueue(1);
			await new Promise((resolve) => setTimeout(resolve, durationMs));
			if (generation !== this.crossfadeGeneration) return;
			oldAudio.pause();
			oldAudio.removeAttribute("src");
			oldAudio.load();
			this.deckFadeLevels.set(oldAudio, 1);
			this.standbyAudio = oldAudio;
		} catch (error) {
			oldAudio.pause();
			await this.handleMediaError(error);
		} finally {
			if (generation === this.crossfadeGeneration) {
				this.crossfadeInProgress = false;
				this.prepareNextDeckIfReady();
			}
		}
	}

	private sourceIsFresh(source: PlaybackSource, marginMs = 30_000) {
		return (
			source.expiresAtMs === null || source.expiresAtMs > Date.now() + marginMs
		);
	}

	private normalizationLevelForSong(song: Song) {
		const metadata = this.metadataForSong(song);
		const requested =
			this.normalizationMode === "track"
				? metadata?.trackGainDb
				: this.normalizationMode === "album"
					? metadata?.albumGainDb
					: 0;
		return Number.isFinite(requested)
			? 10 ** (Math.max(-24, Math.min(requested!, 0)) / 20)
			: 1;
	}

	private invalidatePreparedNextDeck() {
		this.nextPreparationGeneration += 1;
		this.preparingNextKey = "";
		const prepared = this.preparedNextDeck;
		this.preparedNextDeck = null;
		if (prepared && prepared.audio !== this.audio) {
			prepared.audio.pause();
			prepared.audio.removeAttribute("src");
			prepared.audio.load();
		}
	}

	private reconcilePreparedNextDeck() {
		const current = this.current;
		const currentIndex = this.currentIndex;
		const target =
			currentIndex === null ? undefined : this.queue[currentIndex + 1];
		const prepared = this.preparedNextDeck;
		if (
			prepared &&
			(!current ||
				prepared.currentSongId !== current.id ||
				prepared.currentIndex !== currentIndex ||
				prepared.songId !== target?.id ||
				!this.sourceIsFresh(prepared.source))
		) {
			this.invalidatePreparedNextDeck();
		}
		const key =
			current && target ? `${current.id}:${currentIndex}:${target.id}` : "";
		if (this.preparingNextKey && this.preparingNextKey !== key)
			this.invalidatePreparedNextDeck();
		if (this.nextPreparationRetryKey && this.nextPreparationRetryKey !== key) {
			this.nextPreparationRetryKey = "";
			this.nextPreparationRetryAtMs = 0;
		}
		this.prepareNextDeckIfReady();
	}

	private currentDeckHasEnoughBuffer() {
		const audio = this.audio;
		if (!audio || this.activeAudioSongId !== this.current?.id || !audio.src)
			return false;
		if (audio.readyState >= HTMLMediaElement.HAVE_ENOUGH_DATA) return true;
		if (!Number.isFinite(audio.duration) || audio.duration <= 0) return false;
		for (let index = 0; index < audio.buffered.length; index += 1) {
			if (audio.buffered.end(index) >= audio.duration - 1) return true;
		}
		return false;
	}

	private prepareNextDeckIfReady() {
		if (
			this.crossfadeInProgress ||
			this.repeatOne ||
			!this.currentDeckHasEnoughBuffer()
		)
			return;
		const current = this.current;
		const currentIndex = this.currentIndex;
		const target =
			currentIndex === null ? undefined : this.queue[currentIndex + 1];
		if (!current || currentIndex === null || !target) return;
		const key = `${current.id}:${currentIndex}:${target.id}`;
		if (
			(this.nextPreparationRetryKey === key &&
				Date.now() < this.nextPreparationRetryAtMs) ||
			this.preparingNextKey === key ||
			(this.preparedNextDeck?.currentSongId === current.id &&
				this.preparedNextDeck.currentIndex === currentIndex &&
				this.preparedNextDeck.songId === target.id &&
				this.sourceIsFresh(this.preparedNextDeck.source))
		)
			return;

		this.invalidatePreparedNextDeck();
		const generation = this.nextPreparationGeneration;
		this.preparingNextKey = key;
		void prepareNextPlaybackSource(target.id)
			.then((source) => {
				if (
					generation !== this.nextPreparationGeneration ||
					this.current?.id !== current.id ||
					this.currentIndex !== currentIndex ||
					this.queue[currentIndex + 1]?.id !== target.id
				)
					return;
				if (!this.sourceIsFresh(source)) {
					this.nextPreparationRetryKey = key;
					this.nextPreparationRetryAtMs = Date.now() + 15_000;
					return;
				}
				if (source.normalizationGainMetadata) {
					this.setNormalizationGainMetadata(target.id, {
						trackGainDb: source.normalizationGainMetadata.trackGainDb,
						albumGainDb: source.normalizationGainMetadata.albumGainDb,
						trackPeak: source.normalizationGainMetadata.trackPeak,
						albumPeak: source.normalizationGainMetadata.albumPeak,
					});
				} else this.setNormalizationGainMetadata(target.id, null);
				const standby = this.standbyAudio ?? this.createAudioDeck();
				this.standbyAudio = standby;
				this.deckFadeLevels.set(standby, 1);
				this.deckNormalizationLevels.set(
					standby,
					this.normalizationLevelForSong(target),
				);
				this.replaceAudioSource(source.url, standby);
				this.nextPreparationRetryKey = "";
				this.nextPreparationRetryAtMs = 0;
				this.preparedNextDeck = {
					currentSongId: current.id,
					currentIndex,
					songId: target.id,
					source,
					audio: standby,
				};
				this.updateDeckGains();
			})
			.catch(() => {
				this.nextPreparationRetryKey = key;
				this.nextPreparationRetryAtMs = Date.now() + 15_000;
				// Preparation is opportunistic; the ordinary transition remains available.
			})
			.finally(() => {
				if (
					generation === this.nextPreparationGeneration &&
					this.preparingNextKey === key
				)
					this.preparingNextKey = "";
			});
	}

	private takePreparedNextDeck(targetIndex: number) {
		const target = this.queue[targetIndex];
		const prepared = this.preparedNextDeck;
		if (
			!target ||
			!prepared ||
			prepared.currentSongId !== this.current?.id ||
			prepared.currentIndex !== this.currentIndex ||
			prepared.songId !== target.id ||
			!this.sourceIsFresh(prepared.source, 10_000) ||
			prepared.audio.readyState < HTMLMediaElement.HAVE_FUTURE_DATA
		) {
			if (prepared) this.invalidatePreparedNextDeck();
			return null;
		}
		this.preparedNextDeck = null;
		this.preparingNextKey = "";
		return prepared;
	}

	private preloadUpcoming(maximum: number) {
		this.prepareNextDeckIfReady();
		const currentId = this.current?.id;
		if (!currentId || this.warmedQueueForSongId === currentId) return;
		const start = (this.currentIndex ?? -1) + 1;
		const upcoming = this.queue.slice(
			start,
			start + Math.max(0, Math.min(2, maximum)),
		);
		if (!upcoming.length) return;
		this.warmedQueueForSongId = currentId;
		for (const song of upcoming.slice(1)) this.preload(song);
	}

	private beginOptimisticSelection(song: Song, index: number) {
		this.invalidatePreparedNextDeck();
		this.crossfadeGeneration += 1;
		this.crossfadeInProgress = false;
		if (this.standbyAudio && !this.standbyAudio.paused)
			this.standbyAudio.pause();
		this.pendingSeekMs = null;
		if (!this.optimisticCurrent) {
			this.optimisticWasPlaying = Boolean(this.audio && !this.audio.paused);
		}
		if (this.audio && !this.audio.paused) this.audio.pause();
		this.optimisticCurrent = song;
		this.optimisticCurrentIndex = index >= 0 ? index : null;
		this.loading = true;
		this.error = null;
		this.errorKind = null;
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
		this.errorKind = null;
		try {
			const preparation = await action();
			if (generation !== this.loadGeneration) return;
			this.feedNormalizationMetadata(preparation);
			const shouldHydrateQueue = preparation.state.queue.length <= 1;
			this.clearOptimisticSelection(false);
			this.applyState(preparation.state);
			this.positionMs = Math.max(0, resumeAtMs);
			this.durationMs = this.current?.durationMs ?? 0;
			this.beginSession();
			this.sourceRetryCount = 0;
			if (!this.audio) throw new Error("Audio player is not ready");
			this.replaceAudioSource(preparation.source.url);
			this.activeAudioSongId = this.current?.id ?? null;
			await this.restorePositionBeforePlaying(this.audio, resumeAtMs);
			if (generation !== this.loadGeneration) return;
			const songId = this.current?.id;
			if (songId && shouldHydrateQueue) void this.hydrateQueue(songId);
			else if (refillCount > 0) void this.refillQueue(refillCount);
			try {
				await this.playWithTimeout(this.audio);
			} catch (startupError) {
				if (generation !== this.loadGeneration || !songId) return;
				const classified = this.classifyPlaybackFailure(startupError);
				if (!classified.retryable) throw startupError;
				this.sourceRetryCount = 1;
				this.errorKind = "network";
				this.error = "Audio connection interrupted. Reconnecting 1/5…";
				const refreshed = await refreshPlaybackSource();
				if (refreshed.state.current?.id !== songId) throw startupError;
				this.feedNormalizationMetadata(refreshed);
				this.replaceAudioSource(refreshed.source.url);
				await this.restorePositionBeforePlaying(this.audio, resumeAtMs);
				if (generation !== this.loadGeneration) return;
				await this.playWithTimeout(this.audio);
			}
			this.error = null;
			this.errorKind = null;
			this.persistSnapshot();
		} catch (error) {
			if (generation === this.loadGeneration) {
				this.clearOptimisticSelection(true);
				this.setPlaybackError(error);
			}
			throw error;
		} finally {
			if (generation === this.loadGeneration) this.loading = false;
		}
	}

	private feedNormalizationMetadata(preparation: PlaybackPreparation) {
		const songId = preparation.state.current?.id;
		if (!songId) return;
		const metadata = preparation.source.normalizationGainMetadata;
		this.setNormalizationGainMetadata(
			songId,
			metadata
				? {
						trackGainDb: metadata.trackGainDb,
						albumGainDb: metadata.albumGainDb,
						trackPeak: metadata.trackPeak,
						albumPeak: metadata.albumPeak,
					}
				: null,
		);
	}

	private applyState(state: PlaybackState) {
		if (state.current?.id !== this.current?.id) this.warmedQueueForSongId = "";
		this.current = state.current;
		this.queue = state.queue;
		this.currentIndex = state.currentIndex;
		this.reconcilePreparedNextDeck();
		this.applyNormalizationForCurrentSong();
		this.syncSystemMediaSession(true);
	}

	private refillQueue(count: number) {
		const queueGeneration = this.queueEditGeneration;
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
					if (this.queueEditGeneration !== queueGeneration) {
						this.pendingRefillCount = 0;
						break;
					}
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

	private replaceAudioSource(url: string, target = this.audio) {
		if (!target) return;
		if (target === this.audio) {
			this.bufferedMs = 0;
			this.stopListeningInterval();
		}
		target.pause();
		target.removeAttribute("src");
		target.load();
		target.src = url;
		target.load();
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

	private setBackendError(error: unknown) {
		const classified = classifyBackendError(error);
		this.errorKind = classified.kind;
		this.error = classified.message;
	}

	private classifyPlaybackFailure(error: unknown) {
		const backend = classifyBackendError(error);
		const message = errorMessage(error).toLowerCase();
		const isMediaFailure =
			error instanceof DOMException ||
			/media failed|media metadata|audio.*starting|playback timed out/.test(
				message,
			);
		return backend.kind === "unknown" && isMediaFailure && this.audio?.error
			? classifyMediaError(this.audio.error)
			: backend;
	}

	private setPlaybackError(error: unknown) {
		const classified = this.classifyPlaybackFailure(error);
		this.errorKind = classified.kind;
		this.error = classified.message;
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
					revisions.tasteChanged();
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
