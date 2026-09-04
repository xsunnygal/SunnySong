export type ThemePreference = "system" | "light" | "dark";

class ThemeController {
	preference = $state<ThemePreference>("system");
	private media: MediaQueryList | null = null;
	private initialized = false;

	initialize() {
		if (this.initialized || typeof window === "undefined") return;
		this.initialized = true;
		const stored = localStorage.getItem("solmusic-theme");
		if (stored === "light" || stored === "dark" || stored === "system") {
			this.preference = stored;
		}
		this.media = window.matchMedia("(prefers-color-scheme: dark)");
		this.media.addEventListener("change", this.systemChanged);
		this.apply();
	}

	set(preference: ThemePreference) {
		this.preference = preference;
		localStorage.setItem("solmusic-theme", preference);
		this.apply();
	}

	private systemChanged = () => {
		if (this.preference === "system") this.apply();
	};

	private apply() {
		if (typeof document === "undefined") return;
		const resolved = this.preference === "system"
			? this.media?.matches ? "dark" : "light"
			: this.preference;
		document.documentElement.dataset.theme = resolved;
		document.documentElement.style.colorScheme = resolved;
	}
}

export const theme = new ThemeController();
