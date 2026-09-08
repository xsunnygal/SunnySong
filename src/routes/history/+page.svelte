<script lang="ts">
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import { clearHistory, deleteHistoryEvent, deleteSongHistory, getHistoryEvents, type HistoryEvent } from "$lib/api/backend";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  let events = $state<HistoryEvent[]>([]);
  let cursor = $state<number | null>(null);
  let hasMore = $state(false);
  let loading = $state(false);
  let message = $state("");
  let loadedKey = $state("");
  let requestedKey = "";
  let requestGeneration = 0;
  let busyEventIds = $state<string[]>([]);
  let destructiveBusy = $state(false);

  const currentKey = () => `${profiles.active?.id ?? ""}:${profiles.revision}:${revisions.taste}`;
  const day = (value: number) => new Date(value).toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric", year: "numeric" });
  const time = (value: number) => new Date(value).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const showDay = (index: number) => index === 0 || day(events[index - 1].startedAtMs) !== day(events[index].startedAtMs);

  async function load(reset = false, key = currentKey()) {
    if (!profiles.active || (!reset && loading)) return false;
    const generation = ++requestGeneration;
    const before = reset ? null : cursor;
    if (reset) {
      events = [];
      cursor = null;
      hasMore = false;
      loadedKey = "";
    }
    requestedKey = key;
    loading = true;
    message = "";
    try {
      const page = await getHistoryEvents(50, before);
      if (generation !== requestGeneration || currentKey() !== key) return;
      events = reset ? page.items : [...events, ...page.items.filter((event) => !events.some((existing) => existing.eventId === event.eventId))];
      cursor = page.nextCursor;
      hasMore = page.hasMore;
      loadedKey = key;
    } catch (error) {
      if (generation === requestGeneration && currentKey() === key) message = error instanceof Error ? error.message : String(error);
    } finally {
      if (generation === requestGeneration) loading = false;
    }
    return true;
  }

  async function removeEvent(event: HistoryEvent) {
    if (destructiveBusy || busyEventIds.includes(event.eventId) || !confirm(`Remove this listening event for “${event.song.title}”?`)) return;
    const key = currentKey();
    busyEventIds = [...busyEventIds, event.eventId];
    message = "";
    try {
      await deleteHistoryEvent(event.eventId);
      if (currentKey() === key) events = events.filter((item) => item.eventId !== event.eventId);
      revisions.tasteChanged();
    } catch (error) { if (currentKey() === key) message = error instanceof Error ? error.message : String(error); }
    finally { busyEventIds = busyEventIds.filter((id) => id !== event.eventId); }
  }

  async function removeSong(event: HistoryEvent) {
    if (destructiveBusy || busyEventIds.length > 0 || !confirm(`Remove every history event for “${event.song.title}”?`)) return;
    const key = currentKey();
    destructiveBusy = true;
    message = "";
    try {
      await deleteSongHistory(event.song.id);
      if (currentKey() === key) events = events.filter((item) => item.song.id !== event.song.id);
      revisions.tasteChanged();
    } catch (error) { if (currentKey() === key) message = error instanceof Error ? error.message : String(error); }
    finally { destructiveBusy = false; }
  }

  async function clearAll() {
    if (destructiveBusy || busyEventIds.length > 0 || !confirm("Clear all listening history for this profile? This cannot be undone.")) return;
    const key = currentKey();
    destructiveBusy = true;
    message = "";
    try {
      await clearHistory();
      if (currentKey() === key) { events = []; cursor = null; hasMore = false; }
      revisions.tasteChanged();
    } catch (error) { if (currentKey() === key) message = error instanceof Error ? error.message : String(error); }
    finally { destructiveBusy = false; }
  }

  $effect(() => {
    const key = currentKey();
    if (profiles.active && key !== requestedKey) void load(true, key);
  });
  onMount(() => { if (!profiles.active) void profiles.initialize(); });
</script>

<svelte:head><title>History · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><div><h1>History</h1></div>{#if events.length}<button class="text-button danger page-refresh" type="button" disabled={destructiveBusy || busyEventIds.length > 0} onclick={clearAll}>{destructiveBusy ? "Working…" : "Clear all"}</button>{/if}</header>
<p class="sr-only" role="status" aria-live="polite">{loading ? (events.length ? "Loading more history" : "Loading history") : destructiveBusy ? "Updating history" : ""}</p>
{#if message}<p class="inline-message" role="alert">{message} <button class="text-button" type="button" onclick={() => load(!events.length, currentKey())}>Retry</button></p>{/if}
<div class="history-events">{#each loadedKey === currentKey() ? events : [] as event, index (event.eventId)}{#if showDay(index)}<h2>{day(event.startedAtMs)}</h2>{/if}<div class="history-event"><SongRow song={event.song} detail={`${time(event.startedAtMs)} · ${Math.round(event.listenedMs / 60000)} min`} /><details class="compact-menu"><summary aria-label={`History options for ${event.song.title}`}>⋮</summary><div><button type="button" disabled={busyEventIds.includes(event.eventId) || destructiveBusy} onclick={() => removeEvent(event)}>{busyEventIds.includes(event.eventId) ? "Removing…" : "Remove event…"}</button><button class="danger" type="button" disabled={destructiveBusy || busyEventIds.length > 0} onclick={() => removeSong(event)}>Remove song history…</button></div></details></div>{/each}</div>
{#if !events.length && !loading && loadedKey === currentKey()}<p class="inline-message">Your listening events will appear here.</p>{/if}{#if hasMore}<button class="load-more" type="button" disabled={loading} onclick={() => load(false, currentKey())}>{loading ? "Loading…" : "Load more"}</button>{/if}
