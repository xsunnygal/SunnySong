<script lang="ts">
  import type { Song } from "$lib/api/backend";
  import SongActionsMenu from "$lib/components/SongActionsMenu.svelte";
  import { library } from "$lib/features/library/library.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { openSongArtist } from "$lib/features/search/artist-navigation";

  interface Props { song: Song; detail?: string; }
  let { song, detail }: Props = $props();
</script>

<div class="song-row" role="group" aria-label={`${song.title} by ${song.artistName}`} onfocusin={() => player.preload(song)}>
  <div class="song-main">
    <button class="song-art-play" type="button" onclick={() => player.playSong(song)} aria-label={`Play ${song.title} by ${song.artistName}`}>
      {#if library.artwork(song)}<img src={library.artwork(song) ?? ""} alt="" loading="lazy" />{:else}<span class="fallback" aria-hidden="true">♫</span>{/if}
    </button>
    <span class="song-copy"><button class="song-title" type="button" onclick={() => player.playSong(song)}>{song.title}</button><span><button class="artist-link" type="button" onclick={() => openSongArtist(song)}>{song.artistName}</button>{detail ? ` · ${detail}` : ""}</span></span>
  </div>
  <SongActionsMenu {song} compact />
  <button class="icon-button play-button" type="button" aria-label={`Play ${song.title}`} onclick={() => player.playSong(song)}>
    <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7V5Z" /></svg>
  </button>
</div>
