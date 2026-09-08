<script lang="ts">
  import { onMount } from "svelte";
  import { getDownloads, removeDownload, type DownloadRecord } from "$lib/api/backend";
  import { downloads } from "$lib/features/downloads/downloads.svelte";
  import { library } from "$lib/features/library/library.svelte";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  const PAGE_SIZE = 50;
  let items = $state<DownloadRecord[]>([]);
  let hasMore = $state(false);
  let loading = $state(false);
  let message = $state("");
  let loadedKey = $state("");
  let requestedKey = "";
  let requestGeneration = 0;
  let busyIds = $state<string[]>([]);
  const currentKey = () => `${profiles.active?.id ?? ""}:${profiles.revision}:${revisions.downloads}`;

  async function load(reset = false, key = currentKey()) {
    if (!profiles.active || (!reset && loading)) return false;
    const generation = ++requestGeneration;
    const offset = reset ? 0 : items.length;
    if (reset) {
      items = [];
      hasMore = false;
      loadedKey = "";
    }
    requestedKey = key;
    loading = true;
    message = "";
    try {
      const next = await getDownloads(PAGE_SIZE + 1, offset);
      if (generation !== requestGeneration || currentKey() !== key) return;
      const pageItems = next.slice(0, PAGE_SIZE);
      items = reset ? pageItems : [...items, ...pageItems.filter((item) => !items.some((existing) => existing.id === item.id))];
      hasMore = next.length > PAGE_SIZE;
      loadedKey = key;
    } catch (error) {
      if (generation === requestGeneration && currentKey() === key) message = error instanceof Error ? error.message : String(error);
    } finally { if (generation === requestGeneration) loading = false; }
    return true;
  }

  async function remove(item: DownloadRecord) {
    if (busyIds.includes(item.id)) return;
    const deleteFile = item.status === "completed" && !!item.location && confirm("Also delete the downloaded audio file? Cancel keeps the file and removes only this record.");
    if (!deleteFile && !confirm("Remove this download record? The audio file will be kept.")) return;
    const key = currentKey();
    busyIds = [...busyIds, item.id];
    message = "";
    try {
      await removeDownload(item.id, deleteFile);
      if (currentKey() === key) items = items.filter((entry) => entry.id !== item.id);
      revisions.downloadsChanged();
      if (deleteFile) {
        library.version += 1;
        revisions.libraryChanged();
      }
    } catch (error) { if (currentKey() === key) message = error instanceof Error ? error.message : String(error); }
    finally { busyIds = busyIds.filter((id) => id !== item.id); }
  }

  async function retry(item: DownloadRecord) {
    if (busyIds.includes(item.id)) return;
    busyIds = [...busyIds, item.id];
    message = "";
    try {
      await downloads.request(item.song);
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      busyIds = busyIds.filter((id) => id !== item.id);
    }
  }

  $effect(() => {
    const key = currentKey();
    if (profiles.active && key !== requestedKey) void load(true, key);
  });
  onMount(() => { if (!profiles.active) void profiles.initialize(); });
</script>

<svelte:head><title>Downloads · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><div><h1>Downloads</h1><p>{items.length}{hasMore ? "+" : ""} attempts loaded</p></div><button class="icon-button page-refresh" type="button" aria-label="Refresh downloads" disabled={loading} onclick={() => load(true, currentKey())}>↻</button></header>
<p class="sr-only" role="status" aria-live="polite">{loading ? (items.length ? "Loading more downloads" : "Loading downloads") : ""}</p>
{#if message}<p class="inline-message" role="alert">{message} <button class="text-button" type="button" onclick={() => load(!items.length, currentKey())}>Retry</button></p>{/if}
{#if items.length && loadedKey === currentKey()}<div class="record-list">{#each items as item (item.id)}<article class="record-row"><div><strong>{item.song.title}</strong><small>{item.song.artistName} · {new Date(item.createdAtMs).toLocaleString()}</small>{#if item.fileName}<small>{item.fileName}</small>{/if}{#if item.error}<small class="danger">{item.error}</small>{/if}</div><span class:danger={item.status === "failed"} class="status-chip">{item.status}</span><div class="inline-actions">{#if item.status === "failed"}<button class="text-button" type="button" disabled={busyIds.includes(item.id)} onclick={() => retry(item)}>Retry</button>{/if}<button class="text-button danger" type="button" disabled={busyIds.includes(item.id)} onclick={() => remove(item)}>{busyIds.includes(item.id) ? "Removing…" : "Remove"}</button></div></article>{/each}</div>{:else if !loading && loadedKey === currentKey()}<p class="inline-message">No download attempts yet.</p>{/if}
{#if hasMore && loadedKey === currentKey()}<button class="load-more" type="button" disabled={loading} onclick={() => load(false, currentKey())}>{loading ? "Loading…" : "Load more"}</button>{/if}
