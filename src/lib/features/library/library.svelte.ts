import {
	getDiscoveryEnabled,
	setAudioQuality as setBackendAudioQuality,
	setDiscoveryEnabled,
	type AudioQuality,
	type Song,
} from "$lib/api/backend";
import { revisions } from "$lib/features/revisions.svelte";

const DATA_SAVER_KEY = "solmusic-data-saver";
const AUDIO_QUALITY_KEY = "solmusic-audio-quality";

class LibraryController {
	discoveryEnabled = $state(false);
	dataSaverEnabled = $state(false);
	audioQuality = $state<AudioQuality>("high");
	changingAudioQuality = $state(false);
	initialized = $state(false);
	changingDiscovery = $state(false);
	version = $state(0);

	async initialize() {
		if (this.initialized) return;
		this.initialized = true;
		this.dataSaverEnabled = localStorage.getItem(DATA_SAVER_KEY) === "true";
		const storedQuality = localStorage.getItem(AUDIO_QUALITY_KEY);
		this.audioQuality =
			storedQuality === "low" || storedQuality === "medium"
				? storedQuality
				: "high";
		const [qualityResult, discoveryResult] = await Promise.allSettled([
			setBackendAudioQuality(this.audioQuality),
			getDiscoveryEnabled(),
		]);
		if (qualityResult.status === "rejected") {
			// The selected quality remains local and will be retried on the next change.
		}
		this.discoveryEnabled =
			discoveryResult.status === "fulfilled" ? discoveryResult.value : false;
	}

	setDataSaver(enabled: boolean) {
		this.dataSaverEnabled = enabled;
		localStorage.setItem(DATA_SAVER_KEY, String(enabled));
	}

	async setAudioQuality(quality: AudioQuality) {
		if (this.changingAudioQuality || quality === this.audioQuality) return;
		const previous = this.audioQuality;
		this.audioQuality = quality;
		this.changingAudioQuality = true;
		try {
			await setBackendAudioQuality(quality);
			localStorage.setItem(AUDIO_QUALITY_KEY, quality);
		} catch (error) {
			this.audioQuality = previous;
			throw error;
		} finally {
			this.changingAudioQuality = false;
		}
	}

	async setDiscovery(enabled: boolean) {
		if (this.changingDiscovery || enabled === this.discoveryEnabled) return;
		const previous = this.discoveryEnabled;
		this.discoveryEnabled = enabled;
		this.changingDiscovery = true;
		try {
			await setDiscoveryEnabled(enabled);
			this.version += 1;
			revisions.libraryChanged();
		} catch (error) {
			this.discoveryEnabled = previous;
			throw error;
		} finally {
			this.changingDiscovery = false;
		}
	}

	image(url: string | null) {
		if (!url) return null;

		if (!this.dataSaverEnabled || !url.startsWith("http")) return url;
		if (/googleusercontent\.com|ggpht\.com/.test(url)) {
			return /=w\d+-h\d+[^?#]*/.test(url)
				? url.replace(/=w\d+-h\d+[^?#]*/, "=w144-h144-l90-rj")
				: `${url}=w144-h144-l90-rj`;
		}
		return url.replace(/w\d+-h\d+/g, "w144-h144");
	}

	artwork(song: Song) {
		const indexedLibrary =
			song.id.startsWith("local:") || song.id.startsWith("jellyfin:");
		if (!this.discoveryEnabled && !indexedLibrary) return null;
		const fallback = /^[A-Za-z0-9_-]{11}$/.test(song.id)
			? `https://i.ytimg.com/vi/${song.id}/${this.dataSaverEnabled ? "mqdefault" : "hqdefault"}.jpg`
			: null;
		return this.image(song.thumbnailUrl ?? fallback);
	}
}

export const library = new LibraryController();
