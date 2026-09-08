<script lang="ts">
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import { getLikedSongs, type Song } from "$lib/api/backend";
  import { player } from "$lib/features/player/player.svelte";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  let songs = $state<Song[]>([]);
  let hasMore = $state(false);
  let loading = $state(false);
  let loadedKey = $state("");
  let requestedKey = "";
  let requestGeneration = 0;
  let message = $state("");
  const currentKey = () => `${profiles.active?.id ?? ""}:${profiles.revision}:${revisions.taste}`;

  async function load(reset = false, key = currentKey()) {
    if (!profiles.active || (!reset && loading)) return false;
    const generation = ++requestGeneration;
    const offset = reset ? 0 : songs.length;
    if (reset) {
      songs = [];
      hasMore = false;
      loadedKey = "";
    }
    requestedKey = key;
    loading = true;
    message = "";
    try {
      const page = await getLikedSongs(50, offset);
      if (generation !== requestGeneration || currentKey() !== key) return;
      songs = reset ? page.items : [...songs, ...page.items.filter((song) => !songs.some((existing) => existing.id === song.id))];
      hasMore = page.hasMore;
      loadedKey = key;
    } catch (error) {
      if (generation === requestGeneration && currentKey() === key) message = error instanceof Error ? error.message : String(error);
    } finally { if (generation === requestGeneration) loading = false; }
    return true;
  }

  $effect(() => {
    const key = currentKey();
    if (profiles.active && key !== requestedKey) void load(true, key);
  });
  onMount(() => { if (!profiles.active) void profiles.initialize(); });
</script>

<svelte:head><title>Liked Songs · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/library" aria-label="Back to Library">←</a><div><h1>Liked Songs</h1><p>{songs.length}{hasMore ? "+" : ""} loaded</p></div></header>
<p class="sr-only" role="status" aria-live="polite">{loading ? (songs.length ? "Loading more liked songs" : "Loading liked songs") : ""}</p>
{#if message}<p class="inline-message" role="alert">{message} <button class="text-button" type="button" onclick={() => load(!songs.length, currentKey())}>Retry</button></p>{/if}
{#if songs.length && loadedKey === currentKey()}<div class="collection-actions page-actions"><button class="primary-action" type="button" onclick={() => player.playAll(songs)}>Play all</button><button class="secondary-action" type="button" onclick={() => player.shuffle(songs)}>Shuffle</button></div><div class="song-list library-full-list">{#each songs as song (song.id)}<SongRow {song} detail={song.durationMs ? `${Math.floor(song.durationMs / 60000)}:${String(Math.floor(song.durationMs / 1000) % 60).padStart(2, "0")}` : undefined} />{/each}</div>{:else if !loading && loadedKey === currentKey()}<p class="inline-message">Songs you like will appear here.</p>{/if}
{#if hasMore}<button class="load-more" type="button" disabled={loading} onclick={() => load(false, currentKey())}>{loading ? "Loading…" : "Load more"}</button>{/if}
