class ContentRevisions {
	taste = $state(0);
	library = $state(0);
	downloads = $state(0);

	tasteChanged() {
		this.taste += 1;
	}

	libraryChanged() {
		this.library += 1;
		this.tasteChanged();
	}

	downloadsChanged() {
		this.downloads += 1;
	}
}

export const revisions = new ContentRevisions();
