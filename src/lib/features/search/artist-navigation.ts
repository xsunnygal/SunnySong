import { goto } from "$app/navigation";
import type { Song } from "$lib/api/backend";

export function artistHref(song: Pick<Song, "artistId" | "artistName">) {
	const params = new URLSearchParams({ q: song.artistName });
	if (song.artistId) {
		params.set("artistId", song.artistId);
		params.set("artistName", song.artistName);
	}
	return `#/search?${params.toString()}`;
}

export function openSongArtist(song: Pick<Song, "artistId" | "artistName">) {
	window.dispatchEvent(new CustomEvent("solmusic:close-now-playing"));
	return goto(artistHref(song));
}
