<script lang="ts">
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import { getListeningRecap, type ListeningRecap } from "$lib/api/backend";
  import { player } from "$lib/features/player/player.svelte";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  type Period = "all" | "year" | "month";
  let period = $state<Period>("all");
  let recap = $state<ListeningRecap | null>(null);
  let loading = $state(false);
  let message = $state("");
  let loadedKey = $state("");
  let requestedKey = "";
  let requestGeneration = 0;

  const currentKey = () => `${profiles.active?.id ?? ""}:${profiles.revision}:${revisions.taste}:${period}`;
  function range(forPeriod: Period) {
    if (forPeriod === "all") return { from: null, to: null };
    const now = new Date();
    const from = forPeriod === "year" ? new Date(now.getFullYear(), 0, 1) : new Date(now.getFullYear(), now.getMonth(), 1);
    return { from: from.getTime(), to: null };
  }
  function duration(ms: number) { const hours = ms / 3_600_000; return hours >= 1 ? `${hours.toFixed(hours >= 10 ? 0 : 1)} hr` : `${Math.round(ms / 60_000)} min`; }

  async function load(key = currentKey(), requestedPeriod = period) {
    if (!profiles.active) return false;
    const generation = ++requestGeneration;
    const { from, to } = range(requestedPeriod);
    requestedKey = key;
    loading = true;
    message = "";
    try {
      const next = await getListeningRecap(from, to, 10);
      if (generation !== requestGeneration || currentKey() !== key || period !== requestedPeriod) return;
      recap = next;
      loadedKey = key;
    } catch (error) {
      if (generation === requestGeneration && currentKey() === key) message = error instanceof Error ? error.message : String(error);
    } finally { if (generation === requestGeneration) loading = false; }
    return true;
  }

  $effect(() => {
    const key = currentKey();
    if (profiles.active && key !== requestedKey) void load(key, period);
  });
  onMount(() => { if (!profiles.active) void profiles.initialize(); });
</script>

<svelte:head><title>Recap · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><div><h1>Listening Recap</h1></div></header>
<div class="segmented-control period-control" aria-label="Recap period">{#each [["all", "All time"], ["year", "This year"], ["month", "This month"]] as option}<button type="button" class:active={period === option[0]} aria-pressed={period === option[0]} onclick={() => period = option[0] as Period}>{option[1]}</button>{/each}</div>
<p class="sr-only" role="status" aria-live="polite">{loading ? "Loading listening recap" : ""}</p>
{#if message}<p class="inline-message" role="alert">{message} <button class="text-button" type="button" onclick={() => load(currentKey(), period)}>Retry</button></p>{/if}
{#if recap && loadedKey === currentKey()}<p class="coverage-note" class:warning={!recap.coverage.complete}>{recap.coverage.note}</p><dl class="stat-grid"><div><dt>Listening</dt><dd>{duration(recap.totalListenedMs)}</dd></div><div><dt>Plays</dt><dd>{recap.plays}</dd></div><div><dt>Completed</dt><dd>{recap.completions}</dd></div><div><dt>Skipped</dt><dd>{recap.skips}</dd></div><div><dt>Songs</dt><dd>{recap.uniqueSongs}</dd></div><div><dt>Artists</dt><dd>{recap.uniqueArtists}</dd></div></dl>
<div class="recap-grid"><section><div class="section-heading"><h2>Top songs</h2>{#if recap.topSongs.length}<button class="text-button" type="button" onclick={() => player.playAll(recap!.topSongs.map((item) => item.song))}>Play all</button>{/if}</div><div class="song-list">{#each recap.topSongs as item (item.song.id)}<SongRow song={item.song} detail={`${duration(item.listenedMs)} · ${item.plays} plays`} />{/each}</div></section><section><h2>Top artists</h2><ol class="ranked-list">{#each recap.topArtists as item}<li><span><strong>{item.artistName}</strong><small>{item.plays} plays</small></span><b>{duration(item.listenedMs)}</b></li>{/each}</ol></section></div>{:else if !loading && loadedKey === currentKey()}<p class="inline-message">No persisted listening events in this period.</p>{/if}
