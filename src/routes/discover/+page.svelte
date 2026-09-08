<script lang="ts">
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import { getDiscoverFeed, type DiscoverSection } from "$lib/api/backend";
  import { library } from "$lib/features/library/library.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  let sections = $state<DiscoverSection[]>([]);
  let loading = $state(false);
  let message = $state("");
  let loadedKey = $state("");
  let requestedKey = "";
  let requestGeneration = 0;
  const currentKey = () => `${profiles.active?.id ?? ""}:${profiles.revision}:${revisions.taste}:${revisions.library}:${library.discoveryEnabled}`;

  async function load(key = currentKey()) {
    if (!profiles.active) return false;
    const generation = ++requestGeneration;
    requestedKey = key;
    loading = true;
    message = "";
    try {
      const next = await getDiscoverFeed(10);
      if (generation !== requestGeneration || currentKey() !== key) return;
      sections = next;
      loadedKey = key;
      if (!next.some((section) => section.items.length)) message = "Nothing to discover yet. Listen to or like a few songs, then refresh.";
    } catch (error) {
      if (generation === requestGeneration && currentKey() === key) message = typeof navigator !== "undefined" && !navigator.onLine ? "Discover could not refresh while offline. Check your connection or turn off online Discovery to use local recommendations." : error instanceof Error ? error.message : String(error);
    } finally { if (generation === requestGeneration) loading = false; }
    return true;
  }

  $effect(() => {
    const key = currentKey();
    if (profiles.active && library.initialized && key !== requestedKey) void load(key);
  });
  onMount(() => { void Promise.all([profiles.initialize(), library.initialize()]); });
</script>

<svelte:head><title>Discover · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><div><h1>Discover</h1></div><button class="icon-button page-refresh" type="button" aria-label="Refresh Discover" disabled={loading} onclick={() => load(currentKey())}>↻</button></header>
{#if !library.discoveryEnabled}<p class="inline-message">Online suggestions are off. Showing recommendations from your local library.</p>{/if}
<p class="sr-only" role="status" aria-live="polite">{loading ? "Loading Discover recommendations" : ""}</p>
{#if message}<p class="inline-message" role="alert">{message} <button class="text-button" type="button" onclick={() => load(currentKey())}>Retry</button></p>{/if}
{#if loading && loadedKey !== currentKey()}<div class="song-list" aria-hidden="true">{#each Array(5) as _}<div class="row-skeleton skeleton"></div>{/each}</div>{/if}
{#if loadedKey === currentKey()}<div class="discover-sections">{#each sections as section (section.id)}{#if section.items.length}<section><div class="section-heading"><div><h2>{section.title}</h2></div><div class="collection-actions"><button class="text-button" type="button" onclick={() => player.playAll(section.items.map((item) => item.song))}>Play all</button><button class="text-button" type="button" onclick={() => player.shuffle(section.items.map((item) => item.song))}>Shuffle</button></div></div><div class="song-list">{#each section.items as item (item.song.id)}<SongRow song={item.song} detail="DISCOVER" />{/each}</div></section>{/if}{/each}</div>{/if}
