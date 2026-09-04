<script lang="ts">
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import { getRecentSongs, type Song } from "$lib/api/backend";
  import { profiles } from "$lib/features/profiles/profiles.svelte";

  let songs = $state<Song[]>([]);
  let cursor = $state<number | null>(null);
  let hasMore = $state(false);
  let loading = $state(false);
  let message = $state("");
  let mounted = false;
  let loadedProfileId = "";
  let loadedRevision = -1;

  async function loadMore(reset = false) {
    if (loading) return;
    loading = true;
    try {
      const page = await getRecentSongs(25, reset ? null : cursor);
      songs = reset ? page.items : [...songs, ...page.items.filter((song) => !songs.some((item) => item.id === song.id))];
      cursor = page.nextCursor;
      hasMore = page.hasMore;
      if (!songs.length) message = "Your listening history will appear here.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      loading = false;
    }
  }

  function activateProfile(profileId: string, revision: number) {
    if (!mounted || (profileId === loadedProfileId && revision === loadedRevision)) return;
    loadedProfileId = profileId;
    loadedRevision = revision;
    songs = [];
    cursor = null;
    hasMore = false;
    message = "";
    void loadMore(true);
  }

  $effect(() => {
    const profileId = profiles.active?.id;
    const revision = profiles.revision;
    if (profileId) activateProfile(profileId, revision);
  });

  onMount(() => {
    mounted = true;
    if (profiles.active) activateProfile(profiles.active.id, profiles.revision);
  });
</script>

<svelte:head><title>History · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><h1>History</h1></header>
{#if message}<p class="inline-message">{message}</p>{/if}
<div class="song-list">{#each songs as song (song.id)}<SongRow {song} />{/each}</div>
{#if hasMore}<button class="load-more" type="button" onclick={() => loadMore()} disabled={loading}>{loading ? "Loading…" : "Load more"}</button>{/if}
