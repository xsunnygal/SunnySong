<script lang="ts">
  import { addSongToPlaylist, getPlaylists, type Playlist, type Song } from "$lib/api/backend";
  import { downloads } from "$lib/features/downloads/downloads.svelte";
  import { player } from "$lib/features/player/player.svelte";

  interface Props { song: Song; compact?: boolean; }
  let { song, compact = false }: Props = $props();
  let open = $state(false);
  let choosingPlaylist = $state(false);
  let playlists = $state<Playlist[]>([]);
  let loadingPlaylists = $state(false);
  let error = $state("");

  function close() {
    open = false;
    choosingPlaylist = false;
    error = "";
  }

  function toggle(event: MouseEvent) {
    event.stopPropagation();
    open ? close() : open = true;
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
    error = "";
    try {
      await addSongToPlaylist(playlist.id, song);
      close();
    } catch (reason) {
      error = reason instanceof Error ? reason.message : String(reason);
    }
  }
</script>

<div class="song-actions" class:compact>
  <button class="icon-button song-actions-trigger" type="button" aria-label={`More options for ${song.title}`} aria-expanded={open} onclick={toggle}>⋮</button>
  {#if open}
    <button class="song-actions-scrim" type="button" aria-label="Close song options" onclick={close}></button>
    <div class="song-actions-menu" role="menu" aria-label={`Options for ${song.title}`}>
      {#if choosingPlaylist}
        <button type="button" class="song-action-back" onclick={() => choosingPlaylist = false}>← Add to playlist</button>
        {#if loadingPlaylists}<p>Loading playlists…</p>
        {:else if playlists.length}
          {#each playlists as playlist (playlist.id)}<button type="button" role="menuitem" onclick={() => addToPlaylist(playlist)}><span>{playlist.name}</span><small>{playlist.trackCount} songs</small></button>{/each}
        {:else}<a href="#/library" onclick={close}>Create a playlist in Library</a>{/if}
      {:else}
        <button type="button" role="menuitem" onclick={() => { void player.toggleLike(song); close(); }}>{player.isLiked(song.id) ? "Unlike" : "Like"}</button>
        <button type="button" role="menuitem" onclick={() => { void downloads.request(song); close(); }}>Download</button>
        <button type="button" role="menuitem" onclick={showPlaylists}>Add to playlist…</button>
      {/if}
      {#if error}<p class="song-actions-error" role="alert">{error}</p>{/if}
    </div>
  {/if}
</div>
