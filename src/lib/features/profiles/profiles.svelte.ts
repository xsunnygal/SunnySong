import {
	createListeningProfile,
	deleteListeningProfile,
	getActiveListeningProfile,
	getListeningProfiles,
	renameListeningProfile,
	setActiveListeningProfile,
	type ListeningProfile,
} from "$lib/api/backend";

class ProfilesController {
	items = $state<ListeningProfile[]>([]);
	active = $state<ListeningProfile | null>(null);
	loading = $state(false);
	changing = $state(false);
	error = $state<string | null>(null);
	revision = $state(0);
	private initialized = false;

	async initialize() {
		if (this.initialized) return;
		this.initialized = true;
		this.loading = true;
		this.error = null;
		try {
			const [items, active] = await Promise.all([
				getListeningProfiles(),
				getActiveListeningProfile(),
			]);
			this.items = items;
			this.active = active;
		} catch (error) {
			this.initialized = false;
			this.error = error instanceof Error ? error.message : String(error);
			throw error;
		} finally {
			this.loading = false;
		}
	}

	async create(name: string) {
		this.changing = true;
		this.error = null;
		try {
			const profile = await createListeningProfile(name);
			this.items = [...this.items, profile];
			return profile;
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
			throw error;
		} finally {
			this.changing = false;
		}
	}

	async rename(profileId: string, name: string) {
		this.changing = true;
		this.error = null;
		try {
			const profile = await renameListeningProfile(profileId, name);
			this.items = this.items.map((item) => item.id === profileId ? profile : item);
			if (this.active?.id === profileId) this.active = profile;
			return profile;
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
			throw error;
		} finally {
			this.changing = false;
		}
	}

	async select(profileId: string) {
		if (this.active?.id === profileId) return this.active;
		this.changing = true;
		this.error = null;
		try {
			const profile = await setActiveListeningProfile(profileId);
			this.active = profile;
			this.items = this.items.map((item) => item.id === profileId ? profile : item);
			this.revision += 1;
			return profile;
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
			throw error;
		} finally {
			this.changing = false;
		}
	}

	async remove(profileId: string) {
		this.changing = true;
		this.error = null;
		try {
			await deleteListeningProfile(profileId);
			this.items = this.items.filter((item) => item.id !== profileId);
			if (this.active?.id === profileId) {
				this.active = await getActiveListeningProfile();
				this.revision += 1;
			}
		} catch (error) {
			this.error = error instanceof Error ? error.message : String(error);
			throw error;
		} finally {
			this.changing = false;
		}
	}
}

export const profiles = new ProfilesController();
