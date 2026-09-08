<script lang="ts">
  import { tick } from "svelte";
  import { addSongToPlaylist, getPlaylists, type Playlist, type Song } from "$lib/api/backend";
  import { downloads } from "$lib/features/downloads/downloads.svelte";
  import { player } from "$lib/features/player/player.svelte";

  interface Props { song: Song; compact?: boolean; }
  let { song, compact = false }: Props = $props();
  let open = $state(false);
  let choosingPlaylist = $state(false);
  let playlists = $state<Playlist[]>([]);
  let loadingPlaylists = $state(false);
  let mutationBusy = $state(false);
  let error = $state("");
  let trigger = $state<HTMLButtonElement | null>(null);
  let menu = $state<HTMLDivElement | null>(null);
  const shareable = $derived(!song.id.startsWith("local:") && !song.id.startsWith("jellyfin:") && /^[A-Za-z0-9_-]{11}$/.test(song.id));

  function close(restoreFocus = false) {
    open = false;
    choosingPlaylist = false;
    error = "";
    if (restoreFocus) queueMicrotask(() => trigger?.focus());
  }

  function toggle(event: MouseEvent) {
    event.stopPropagation();
    if (open) close(true);
    else {
      open = true;
      void tick().then(() => menu?.querySelector<HTMLElement>("[role='menuitem']")?.focus());
    }
  }

  function menuKeydown(event: KeyboardEvent) {
    const items = [...(menu?.querySelectorAll<HTMLElement>("[role='menuitem']") ?? [])];
    const index = items.indexOf(document.activeElement as HTMLElement);
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      items[(index + (event.key === "ArrowDown" ? 1 : -1) + items.length) % items.length]?.focus();
    }
  }

  async function shareSong() {
    const url = `https://music.youtube.com/watch?v=${song.id}`;
    try {
      if (navigator.share) await navigator.share({ title: `${song.title} · ${song.artistName}`, url });
      else await navigator.clipboard.writeText(url);
      close(true);
    } catch (reason) {
      if ((reason as DOMException)?.name !== "AbortError") error = "Could not share this song.";
    }
  }

  async function showPlaylists() {
    choosingPlaylist = true;
    loadingPlaylists = true;
    error = "";
    try { playlists = await getPlaylists(); }
    catch (reason) { error = reason instanceof Error ? reason.message : String(reason); }
    finally { loadingPlaylists = false; }
  }

  async function addToPlaylist(playlist: Playlist) {
    if (mutationBusy) return;
    mutationBusy = true;
    error = "";
    try {
      await addSongToPlaylist(playlist.id, song);
      close();
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    } finally {
      mutationBusy = false;
    }
  }
</script>

<div class="song-actions" class:compact>
  <button bind:this={trigger} class="icon-button song-actions-trigger" type="button" aria-haspopup="menu" aria-label={`More options for ${song.title}`} aria-expanded={open} onclick={toggle}>⋮</button>
  {#if open}
    <button class="song-actions-scrim" type="button" aria-label="Close song options" onclick={() => close(true)}></button>
    <div bind:this={menu} class="song-actions-menu" role="menu" aria-label={`Options for ${song.title}`} tabindex="-1" onkeydown={menuKeydown}>
      {#if choosingPlaylist}
        <button type="button" class="song-action-back" onclick={() => choosingPlaylist = false}>← Add to playlist</button>
        {#if loadingPlaylists}<p>Loading playlists…</p>
        {:else if playlists.length}
          {#each playlists as playlist (playlist.id)}<button type="button" role="menuitem" disabled={mutationBusy} onclick={() => addToPlaylist(playlist)}><span>{playlist.name}</span><small>{playlist.trackCount} songs</small></button>{/each}
        {:else}<a href="#/library" onclick={() => close()}>Create a playlist in Library</a>{/if}
      {:else}
        <button class="primary-menu-action" type="button" role="menuitem" onclick={() => { void player.playNextSong(song); close(true); }}>Play Next</button>
        <button type="button" role="menuitem" onclick={() => { void player.toggleLike(song); close(true); }}>{player.isLiked(song.id) ? "Unlike" : "Like"}</button>
        <button type="button" role="menuitem" onclick={() => { void player.addToQueue(song); close(true); }}>Add to queue</button>
        <button type="button" role="menuitem" onclick={() => { void downloads.request(song); close(); }}>Download</button>
        <button type="button" role="menuitem" onclick={showPlaylists}>Add to playlist…</button>
        {#if shareable}<button type="button" role="menuitem" onclick={shareSong}>Share</button>{/if}
      {/if}
      {#if error}<p class="song-actions-error" role="alert">{error}</p>{/if}
    </div>
  {/if}
</div>
