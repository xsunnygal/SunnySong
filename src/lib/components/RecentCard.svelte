<script lang="ts">
  import type { Song } from "$lib/api/backend";
  import { library } from "$lib/features/library/library.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { openSongArtist } from "$lib/features/search/artist-navigation";

  interface Props { song: Song; }
  let { song }: Props = $props();
</script>

<div class="recent-card" onfocusin={() => player.preload(song)}>
  <button class="recent-play" type="button" onclick={() => player.playSong(song)} aria-label={`Play ${song.title} by ${song.artistName}`}>
    {#if library.artwork(song)}<img src={library.artwork(song) ?? ""} alt="" loading="lazy" />{:else}<span class="recent-fallback" aria-hidden="true">♫</span>{/if}
  </button>
  <button class="recent-title" type="button" onclick={() => player.playSong(song)}>{song.title}</button>
  <button class="artist-link" type="button" onclick={() => openSongArtist(song)}>{song.artistName}</button>
</div>
