<script lang="ts">
  import { onMount } from "svelte";
  import { getDeveloperDiagnostics, type DeveloperDiagnostics } from "$lib/api/backend";

  let diagnostics = $state<DeveloperDiagnostics | null>(null);
  let error = $state("");
  let loading = $state(false);

  async function refresh() {
    loading = true;
    error = "";
    try { diagnostics = await getDeveloperDiagnostics(); }
    catch (cause) { error = cause instanceof Error ? cause.message : String(cause); }
    finally { loading = false; }
  }

  const bytes = (value: number) => value < 1024 * 1024
    ? `${Math.round(value / 1024)} KiB`
    : `${(value / 1024 / 1024).toFixed(1)} MiB`;

  onMount(() => { void refresh(); });
</script>

<svelte:head><title>Diagnostics · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/settings" aria-label="Back to Settings">←</a><h1>Developer Diagnostics</h1><button class="text-button diagnostics-refresh" type="button" onclick={refresh} disabled={loading}>{loading ? "Refreshing…" : "Refresh"}</button></header>
{#if error}<p class="inline-message">{error}</p>{/if}
{#if diagnostics}
  <div class="diagnostics-grid">
    <section class="diagnostic-card"><h2>Database</h2><dl><div><dt>Active profile</dt><dd>{diagnostics.activeProfile.name}</dd></div><div><dt>Profile ID</dt><dd>{diagnostics.activeProfile.id}</dd></div><div><dt>Integrity</dt><dd>{diagnostics.database.integrityStatus}</dd></div><div><dt>Schema</dt><dd>v{diagnostics.database.schemaVersion}</dd></div><div><dt>Size</dt><dd>{bytes(diagnostics.database.databaseSizeBytes)}</dd></div><div><dt>Tracks</dt><dd>{diagnostics.database.trackCount}</dd></div><div><dt>Artists</dt><dd>{diagnostics.database.artistCount}</dd></div><div><dt>History events</dt><dd>{diagnostics.database.historyEventCount}</dd></div><div><dt>Liked tracks</dt><dd>{diagnostics.database.likedSongCount}</dd></div><div><dt>Query time</dt><dd>{diagnostics.database.queryDurationMs} ms</dd></div></dl></section>
    <section class="diagnostic-card"><h2>Player</h2><dl><div><dt>Track ID</dt><dd>{diagnostics.player.current?.id ?? "None"}</dd></div><div><dt>Queue index</dt><dd>{diagnostics.player.currentIndex ?? "—"}</dd></div><div><dt>Queue size</dt><dd>{diagnostics.player.queue.length}</dd></div></dl></section>
  </div>
  <section class="recommendation-diagnostics"><h2>Recommendation explanations</h2>{#each diagnostics.recommendations as item, index (item.song.id)}<details><summary><span>{index + 1}. {item.song.title} — {item.song.artistName}</span><strong>{item.score.toFixed(2)}</strong></summary><p>Source: {item.source} · Policy: {item.policyVersion}</p><table><thead><tr><th>Component</th><th>Raw</th><th>Contribution</th></tr></thead><tbody>{#each item.components as component}<tr><td>{component.name}</td><td>{component.rawValue.toFixed(2)}</td><td class:negative={component.contribution < 0}>{component.contribution.toFixed(2)}</td></tr>{/each}</tbody></table></details>{/each}</section>
{/if}
