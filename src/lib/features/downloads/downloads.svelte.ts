import {
	downloadSong,
	getMusicDirectories,
	pickAndroidDownloadDirectory,
	type AndroidStorageDirectory,
	type MusicDirectory,
	type Song,
} from "$lib/api/backend";
import { library } from "$lib/features/library/library.svelte";

const ANDROID_DIRECTORIES_KEY = "solmusic-android-download-directories";

class DownloadController {
	song = $state<Song | null>(null);
	open = $state(false);
	loading = $state(false);
	directories = $state<MusicDirectory[]>([]);
	androidDirectories = $state<AndroidStorageDirectory[]>([]);
	selectedDirectoryId = $state<number | null>(null);
	selectedAndroidUri = $state<string | null>(null);
	error = $state<string | null>(null);
	message = $state<string | null>(null);

	get isAndroid() {
		return (
			typeof navigator !== "undefined" && /Android/i.test(navigator.userAgent)
		);
	}

	async request(song: Song) {
		this.song = song;
		this.open = true;
		this.error = null;
		this.message = null;
		if (this.isAndroid) {
			this.androidDirectories = this.readAndroidDirectories();
			this.selectedAndroidUri = this.androidDirectories[0]?.uri ?? null;
			return;
		}
		try {
			this.directories = await getMusicDirectories();
			this.selectedDirectoryId = this.directories[0]?.id ?? null;
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
		}
	}

	close() {
		if (this.loading) return;
		this.open = false;
		this.song = null;
	}

	async addAndroidDirectory() {
		this.error = null;
		try {
			const directory = await pickAndroidDownloadDirectory();
			if (!directory.canWrite || !directory.persisted) {
				throw new Error(
					"Android did not grant persistent write access to this folder",
				);
			}
			this.androidDirectories = [
				directory,
				...this.androidDirectories.filter((item) => item.uri !== directory.uri),
			];
			this.selectedAndroidUri = directory.uri;
			localStorage.setItem(
				ANDROID_DIRECTORIES_KEY,
				JSON.stringify(this.androidDirectories),
			);
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
		}
	}

	async confirm() {
		if (!this.song || this.loading) return;
		if (this.isAndroid && !this.selectedAndroidUri) {
			this.error = "Choose an Android download folder first.";
			return;
		}
		if (!this.isAndroid && this.selectedDirectoryId === null) {
			this.error = "Add or choose a local music folder first.";
			return;
		}
		this.loading = true;
		this.error = null;
		this.message = "Downloading audio and writing metadata…";
		let completed = false;
		try {
			const result = await downloadSong(
				this.song,
				this.isAndroid ? null : this.selectedDirectoryId,
				this.isAndroid ? this.selectedAndroidUri : null,
			);
			this.message = `${result.fileName} was downloaded.`;
			if (!this.isAndroid) library.version += 1;
			completed = true;
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
			this.message = null;
		} finally {
			this.loading = false;
			if (completed) {
				this.open = false;
				this.song = null;
			}
		}
	}

	private readAndroidDirectories(): AndroidStorageDirectory[] {
		try {
			const parsed = JSON.parse(
				localStorage.getItem(ANDROID_DIRECTORIES_KEY) ?? "[]",
			);
			return Array.isArray(parsed)
				? parsed.filter(
						(item): item is AndroidStorageDirectory =>
							typeof item?.uri === "string" &&
							typeof item?.displayName === "string",
					)
				: [];
		} catch {
			return [];
		}
	}
}

export const downloads = new DownloadController();
