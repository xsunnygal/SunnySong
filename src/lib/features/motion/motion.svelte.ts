class MotionController {
	enabled = $state(true);
	private initialized = false;

	initialize() {
		if (this.initialized || typeof window === "undefined") return;
		this.initialized = true;
		const stored = localStorage.getItem("solmusic-motion");
		this.enabled =
			stored === "on" ||
			(stored === null &&
				!window.matchMedia("(prefers-reduced-motion: reduce)").matches);
		this.apply();
	}

	set(enabled: boolean) {
		this.enabled = enabled;
		if (typeof localStorage !== "undefined") {
			localStorage.setItem("solmusic-motion", enabled ? "on" : "off");
		}
		this.apply();
	}

	private apply() {
		if (typeof document === "undefined") return;
		document.documentElement.dataset.motion = this.enabled ? "on" : "off";
	}
}

export const motion = new MotionController();
