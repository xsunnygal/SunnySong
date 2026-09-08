<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import SongRow from "$lib/components/SongRow.svelte";
  import {
    getLibraryAlbums,
    getLibraryAlbumTracks,
    getLibraryFolders,
    getLibraryTracks,
    getPlaylists,
    createPlaylist,
    getPlaylistTracks,
    addSongToPlaylist,
    deletePlaylist,
    removeSongFromPlaylist,
    renamePlaylist,
    reorderPlaylistTracks,
    downloadSong,
    exportPlaylistM3u,
    getDownloads,
    getLikedSongs,
    getListeningRecap,
    getMusicDirectories,
    getRecentSongs,
    importPlaylistM3u,
    pickAndroidDownloadDirectory,
    type LibraryAlbum,
    type LibraryFolder,
    type LibraryTrack,
    type Playlist,
    type PlaylistTrack,
    type Song,
  } from "$lib/api/backend";
  import { library } from "$lib/features/library/library.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  type AutomaticCollection = "liked" | "downloads" | "recent" | "most-played";
  type LibraryView = "overview" | "songs" | "albums" | "folders" | "album" | "playlists" | "playlist" | "automatic";
  const TRACK_PREVIEW_COUNT = 6;
  const ALBUM_PREVIEW_COUNT = 6;

  let view = $state<LibraryView>("overview");
  let tracks = $state<LibraryTrack[]>([]);
  let albums = $state<LibraryAlbum[]>([]);
  let folders = $state<LibraryFolder[]>([]);
  let selectedAlbum = $state<LibraryAlbum | null>(null);
  let playlists = $state<Playlist[]>([]);
  let selectedPlaylist = $state<Playlist | null>(null);
  let playlistTracks = $state<PlaylistTrack[]>([]);
  let newPlaylistName = $state("");
  let creatingPlaylist = $state(false);
  let albumTracks = $state<LibraryTrack[]>([]);
  let tracksHaveMore = $state(false);
  let albumsHaveMore = $state(false);
  let loading = $state(true);
  let loadingMore = $state(false);
  let message = $state("");
  let loadError = $state("");
  let observedLibraryVersion = -1;
  let requestGeneration = 0;
  let itemQuery = $state("");
  let sourceFilter = $state<"all" | "local" | "jellyfin">("all");
  let sort = $state<"title" | "artist" | "duration">("title");
  let playlistMenuOpen = $state(false);
  let playlistEditing = $state(false);
  let playlistBusy = $state(false);
  let automaticCollection = $state<AutomaticCollection | null>(null);
  let automaticSongs = $state<Song[]>([]);
  let selectionMode = $state(false);
  let selectedSongIds = $state<string[]>([]);
  let bulkBusy = $state(false);
  let bulkPlaylistOpen = $state(false);
  let filesystemPlaylistIoSupported = $state(false);
  let playlistIoBusy = $state(false);
  const albumTrackCache = new Map<string, LibraryTrack[]>();

  const sourceLabel = (source: "local" | "jellyfin") => source === "jellyfin" ? "Jellyfin" : "On this device";
  const visibleTracks = $derived(tracks.filter((item) => sourceFilter === "all" || item.source === sourceFilter).filter((item) => !itemQuery.trim() || `${item.song.title} ${item.song.artistName} ${item.song.albumName ?? ""}`.toLowerCase().includes(itemQuery.trim().toLowerCase())).toSorted((left, right) => sort === "artist" ? left.song.artistName.localeCompare(right.song.artistName) : sort === "duration" ? (left.song.durationMs ?? 0) - (right.song.durationMs ?? 0) : left.song.title.localeCompare(right.song.title)));
  const visibleAlbums = $derived(albums.filter((item) => sourceFilter === "all" || item.source === sourceFilter).filter((item) => !itemQuery.trim() || `${item.title} ${item.artistName}`.toLowerCase().includes(itemQuery.trim().toLowerCase())).toSorted((left, right) => left.title.localeCompare(right.title)));
  const duration = (milliseconds: number | null) => milliseconds ? `${Math.floor(milliseconds / 60_000)}:${String(Math.floor(milliseconds / 1_000) % 60).padStart(2, "0")}` : "";
  const automaticTitle = (kind: AutomaticCollection | null) => kind === "liked" ? "Liked Songs" : kind === "downloads" ? "Downloads" : kind === "recent" ? "Recently Played" : "Most Played";

  function cancelSelection() {
    selectionMode = false;
    selectedSongIds = [];
    bulkPlaylistOpen = false;
  }

  function beginSelection() {
    playlistEditing = false;
    playlistMenuOpen = false;
    selectionMode = true;
    selectedSongIds = [];
  }

  function toggleSelected(songId: string) {
    selectedSongIds = selectedSongIds.includes(songId) ? selectedSongIds.filter((id) => id !== songId) : [...selectedSongIds, songId];
  }

  function selectedFrom(songs: Song[]) {
    const selected = new Set(selectedSongIds);
    return songs.filter((song) => selected.has(song.id));
  }

  function toggleSelectAll(songs: Song[]) {
    const ids = [...new Set(songs.map((song) => song.id))];
    selectedSongIds = ids.length > 0 && ids.every((id) => selectedSongIds.includes(id)) ? [] : ids;
  }

  async function runBulk(action: () => Promise<void>, success: string) {
    if (bulkBusy) return;
    bulkBusy = true;
    message = "";
    try {
      await action();
      message = success;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      bulkBusy = false;
    }
  }

  async function bulkPlayNext(songs: Song[]) {
    await runBulk(async () => {
      for (const song of [...songs].reverse()) await player.playNextSong(song);
    }, `${songs.length} song${songs.length === 1 ? "" : "s"} added next.`);
  }

  async function bulkAddToQueue(songs: Song[]) {
    await runBulk(async () => {
      for (const song of songs) await player.addToQueue(song);
    }, `${songs.length} song${songs.length === 1 ? "" : "s"} added to the queue.`);
  }

  async function bulkSetLiked(songs: Song[]) {
    const liking = songs.some((song) => !player.isLiked(song.id));
    await runBulk(async () => {
      for (const song of songs) if (player.isLiked(song.id) !== liking) await player.toggleLike(song);
    }, `${songs.length} song${songs.length === 1 ? "" : "s"} ${liking ? "liked" : "unliked"}.`);
  }

  async function bulkDownload(songs: Song[]) {
    await runBulk(async () => {
      const isAndroid = /Android/i.test(navigator.userAgent);
      let directoryId: number | null = null;
      let androidUri: string | null = null;
      if (isAndroid) {
        try {
          const saved = JSON.parse(localStorage.getItem("solmusic-android-download-directories") ?? "[]");
          androidUri = Array.isArray(saved) && typeof saved[0]?.uri === "string" ? saved[0].uri : null;
        } catch { androidUri = null; }
        if (!androidUri) {
          const directory = await pickAndroidDownloadDirectory();
          if (!directory.canWrite || !directory.persisted) throw new Error("Android did not grant persistent write access to this folder");
          androidUri = directory.uri;
          localStorage.setItem("solmusic-android-download-directories", JSON.stringify([directory]));
        }
      } else {
        const directories = await getMusicDirectories();
        directoryId = directories[0]?.id ?? null;
        if (directoryId === null) throw new Error("Add a local music folder in Settings before downloading.");
      }
      let completed = 0;
      revisions.downloadsChanged();
      try {
        for (const song of songs) {
          await downloadSong(song, directoryId, androidUri);
          completed += 1;
        }
      } finally {
        revisions.downloadsChanged();
        if (completed > 0) revisions.libraryChanged();
      }
    }, `${songs.length} song${songs.length === 1 ? "" : "s"} downloaded.`);
  }

  async function showBulkPlaylists() {
    if (bulkBusy) return;
    bulkPlaylistOpen = !bulkPlaylistOpen;
    if (!bulkPlaylistOpen) return;
    try { playlists = await getPlaylists(); }
    catch (error) { message = error instanceof Error ? error.message : String(error); }
  }

  async function bulkAddToPlaylist(playlist: Playlist, songs: Song[]) {
    await runBulk(async () => {
      for (const song of songs) await addSongToPlaylist(playlist.id, song);
      playlists = await getPlaylists();
      revisions.libraryChanged();
      bulkPlaylistOpen = false;
    }, `${songs.length} song${songs.length === 1 ? "" : "s"} added to ${playlist.name}.`);
  }

  async function bulkRemoveFromPlaylist(songs: Song[]) {
    if (!selectedPlaylist || !confirm(`Remove ${songs.length} selected song${songs.length === 1 ? "" : "s"} from “${selectedPlaylist.name}”?`)) return;
    const playlistId = selectedPlaylist.id;
    await runBulk(async () => {
      for (const song of songs) await removeSongFromPlaylist(playlistId, song.id);
      playlistTracks = playlistTracks.filter((item) => !songs.some((song) => song.id === item.song.id));
      if (selectedPlaylist?.id === playlistId) selectedPlaylist = { ...selectedPlaylist, trackCount: playlistTracks.length };
      playlists = await getPlaylists();
      revisions.libraryChanged();
      cancelSelection();
    }, `${songs.length} song${songs.length === 1 ? "" : "s"} removed from the playlist.`);
  }

  async function loadOverview() {
    const generation = ++requestGeneration;
    loading = true;
    message = "";
    loadError = "";
    try {
      const [trackResult, albumResult, folderResult, playlistResult] = await Promise.allSettled([
        getLibraryTracks(TRACK_PREVIEW_COUNT + 1),
        getLibraryAlbums(ALBUM_PREVIEW_COUNT + 1),
        getLibraryFolders(),
        getPlaylists(),
      ]);
      if (generation !== requestGeneration) return;
      if (trackResult.status === "fulfilled") {
        tracksHaveMore = trackResult.value.length > TRACK_PREVIEW_COUNT;
        tracks = trackResult.value.slice(0, TRACK_PREVIEW_COUNT);
      }
      if (albumResult.status === "fulfilled") {
        albumsHaveMore = albumResult.value.length > ALBUM_PREVIEW_COUNT;
        albums = albumResult.value.slice(0, ALBUM_PREVIEW_COUNT);
      }
      if (folderResult.status === "fulfilled") folders = folderResult.value;
      if (playlistResult.status === "fulfilled") playlists = playlistResult.value;
      const failures = [trackResult, albumResult, folderResult, playlistResult].filter((result) => result.status === "rejected");
      if (failures.length) loadError = failures.length === 4 ? "Could not load the library." : "Some library sections could not be loaded.";
      else if (!tracks.length && !albums.length && !folders.length && !playlists.length) message = "Add a music folder or enable and sync a Jellyfin music library in Settings.";
    } finally {
      if (generation === requestGeneration) loading = false;
    }
  }

  async function openSongs() {
    cancelSelection();
    view = "songs";
    if (tracks.length >= 100 || !tracksHaveMore) return;
    loading = true;
    message = "";
    loadError = "";
    try {
      const next = await getLibraryTracks(100 - tracks.length, tracks.length);
      tracks = [...tracks, ...next];
      tracksHaveMore = tracks.length >= 100 && next.length > 0;
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally { loading = false; }
  }

  async function loadMoreSongs() {
    if (loadingMore) return;
    loadingMore = true;
    loadError = "";
    try {
      const next = await getLibraryTracks(100, tracks.length);
      tracks = [...tracks, ...next];
      tracksHaveMore = next.length === 100;
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally { loadingMore = false; }
  }

  async function openAlbums() {
    cancelSelection();
    view = "albums";
    if (albums.length >= 100 || !albumsHaveMore) return;
    loading = true;
    message = "";
    loadError = "";
    try {
      const next = await getLibraryAlbums(100 - albums.length, albums.length);
      albums = [...albums, ...next];
      albumsHaveMore = albums.length >= 100 && next.length > 0;
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally { loading = false; }
  }

  async function loadMoreAlbums() {
    if (loadingMore) return;
    loadingMore = true;
    loadError = "";
    try {
      const next = await getLibraryAlbums(100, albums.length);
      albums = [...albums, ...next];
      albumsHaveMore = next.length === 100;
    } catch (error) {
      loadError = error instanceof Error ? error.message : String(error);
    } finally { loadingMore = false; }
  }

  function openFolders() {
    cancelSelection();
    view = "folders";
  }

  function openPlaylists() {
    cancelSelection();
    view = "playlists";
  }

  async function openAutomatic(kind: AutomaticCollection) {
    const generation = ++requestGeneration;
    cancelSelection();
    automaticCollection = kind;
    automaticSongs = [];
    view = "automatic";
    loading = true;
    message = "";
    loadError = "";
    try {
      let songs: Song[];
      if (kind === "liked") songs = (await getLikedSongs(100, 0)).items;
      else if (kind === "downloads") {
        const records = await getDownloads(100, 0);
        songs = [...new Map(records.filter((item) => item.status === "completed").map((item) => [item.song.id, item.song])).values()];
      } else if (kind === "recent") songs = (await getRecentSongs(100, null)).items;
      else songs = (await getListeningRecap(null, null, 100)).topSongs.map((item) => item.song);
      if (generation === requestGeneration && view === "automatic" && automaticCollection === kind) automaticSongs = songs;
    } catch (error) {
      if (generation === requestGeneration) loadError = error instanceof Error ? error.message : String(error);
    } finally {
      if (generation === requestGeneration) loading = false;
    }
  }

  async function openPlaylist(playlist: Playlist) {
    cancelSelection();
    selectedPlaylist = playlist;
    view = "playlist";
    loading = true;
    message = "";
    loadError = "";
    try { playlistTracks = await getPlaylistTracks(playlist.id); }
    catch (error) { loadError = error instanceof Error ? error.message : String(error); }
    finally { loading = false; }
  }

  async function submitPlaylist() {
    const name = newPlaylistName.trim();
    if (!name || creatingPlaylist) return;
    creatingPlaylist = true;
    message = "";
    try {
      const playlist = await createPlaylist(name);
      playlists = [playlist, ...playlists];
      newPlaylistName = "";
    } catch (error) { message = error instanceof Error ? error.message : String(error); }
    finally { creatingPlaylist = false; }
  }

  async function exportCurrentPlaylist() {
    if (!selectedPlaylist || playlistIoBusy) return;
    const safeName = selectedPlaylist.name.replace(/[\\/:*?"<>|]/g, "-").trim() || "playlist";
    const path = await saveDialog({
      title: `Export ${selectedPlaylist.name}`,
      defaultPath: `${safeName}.m3u8`,
      filters: [{ name: "UTF-8 M3U playlist", extensions: ["m3u8", "m3u"] }],
    });
    if (!path) return;
    playlistIoBusy = true;
    playlistMenuOpen = false;
    message = "Exporting playlist…";
    try {
      const result = await exportPlaylistM3u(selectedPlaylist.id, path);
      message = `Exported ${result.exported} tracks. ${result.preservedAsMetadata ? `${result.preservedAsMetadata} unavailable or Jellyfin ${result.preservedAsMetadata === 1 ? "item was" : "items were"} preserved as SunnySong metadata.` : "All entries are portable."}`;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      playlistIoBusy = false;
    }
  }

  async function importM3uPlaylist() {
    if (playlistIoBusy) return;
    const selected = await openDialog({
      title: "Import M3U Playlist",
      multiple: false,
      filters: [{ name: "UTF-8 M3U playlist", extensions: ["m3u8", "m3u"] }],
    });
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (!path) return;
    const fileName = path.split(/[\\/]/).pop()?.replace(/\.(m3u8?|M3U8?)$/, "") || "Imported playlist";
    const name = prompt("Name this local playlist", fileName)?.trim();
    if (!name) return;
    playlistIoBusy = true;
    message = "Resolving playlist entries against your indexed library…";
    try {
      const result = await importPlaylistM3u(path, name);
      playlists = await getPlaylists();
      revisions.libraryChanged();
      const reasons = result.reasons.map((item) => `${item.count} ${item.reason.toLowerCase()}`).join(", ");
      message = `Imported ${result.imported} tracks into “${result.playlistName}”; skipped ${result.skipped}${reasons ? ` (${reasons})` : ""}.`;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      playlistIoBusy = false;
    }
  }

  async function renameCurrentPlaylist() {
    if (!selectedPlaylist || playlistBusy) return;
    const name = prompt("Playlist name", selectedPlaylist.name)?.trim();
    if (!name || name === selectedPlaylist.name) return;
    playlistBusy = true;
    try {
      selectedPlaylist = await renamePlaylist(selectedPlaylist.id, name);
      playlists = playlists.map((item) => item.id === selectedPlaylist?.id ? selectedPlaylist : item);
      revisions.libraryChanged();
    } catch (error) { message = error instanceof Error ? error.message : String(error); }
    finally { playlistBusy = false; playlistMenuOpen = false; }
  }

  async function deleteCurrentPlaylist() {
    if (!selectedPlaylist || playlistBusy || !confirm(`Delete “${selectedPlaylist.name}”? Songs and files will not be deleted.`)) return;
    playlistBusy = true;
    message = "";
    try {
      await deletePlaylist(selectedPlaylist.id);
      playlists = playlists.filter((item) => item.id !== selectedPlaylist?.id);
      revisions.libraryChanged();
      back();
    } catch (error) { message = error instanceof Error ? error.message : String(error); }
    finally { playlistBusy = false; playlistMenuOpen = false; }
  }

  async function removePlaylistSong(songId: string) {
    if (!selectedPlaylist || playlistBusy || !confirm("Remove this song from the playlist?")) return;
    const playlistId = selectedPlaylist.id;
    playlistBusy = true;
    message = "";
    try {
      await removeSongFromPlaylist(playlistId, songId);
      if (selectedPlaylist?.id === playlistId) {
        playlistTracks = playlistTracks.filter((item) => item.song.id !== songId);
        selectedPlaylist = { ...selectedPlaylist, trackCount: playlistTracks.length };
      }
      revisions.libraryChanged();
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      playlistBusy = false;
    }
  }

  async function movePlaylistSong(index: number, delta: number) {
    if (!selectedPlaylist || playlistBusy) return;
    const target = index + delta;
    if (target < 0 || target >= playlistTracks.length) return;
    const playlistId = selectedPlaylist.id;
    const next = [...playlistTracks];
    [next[index], next[target]] = [next[target], next[index]];
    playlistBusy = true;
    message = "";
    try {
      const reordered = await reorderPlaylistTracks(playlistId, next.map((item) => item.song.id));
      if (selectedPlaylist?.id === playlistId) playlistTracks = reordered;
      revisions.libraryChanged();
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      playlistBusy = false;
    }
  }

  async function openAlbum(album: LibraryAlbum) {
    cancelSelection();
    selectedAlbum = album;
    view = "album";
    const cached = albumTrackCache.get(album.id);
    if (cached) {
      albumTracks = cached;
      return;
    }
    loading = true;
    message = "";
    loadError = "";
    try {
      albumTracks = await getLibraryAlbumTracks(album.id);
      albumTrackCache.set(album.id, albumTracks);
    }
    catch (error) { loadError = error instanceof Error ? error.message : String(error); }
    finally { loading = false; }
  }

  function retryLoad() {
    if (view === "overview") void loadOverview();
    else if (view === "songs") void (tracks.length < 100 ? openSongs() : loadMoreSongs());
    else if (view === "albums") void (albums.length < 100 ? openAlbums() : loadMoreAlbums());
    else if (view === "automatic" && automaticCollection) void openAutomatic(automaticCollection);
    else if (view === "playlist" && selectedPlaylist) void openPlaylist(selectedPlaylist);
    else if (view === "album" && selectedAlbum) {
      albumTrackCache.delete(selectedAlbum.id);
      void openAlbum(selectedAlbum);
    }
  }

  function back() {
    if (view === "overview") void goto("#/");
    else {
      requestGeneration += 1;
      loading = false;
      loadError = "";
      view = "overview";
      selectedAlbum = null;
      selectedPlaylist = null;
      albumTracks = [];
      playlistTracks = [];
      playlistEditing = false;
      playlistMenuOpen = false;
      automaticCollection = null;
      automaticSongs = [];
      cancelSelection();
      itemQuery = "";
    }
  }

  $effect(() => {
    const version = library.version;
    if (observedLibraryVersion >= 0 && version !== observedLibraryVersion) {
      view = "overview";
      cancelSelection();
      albumTrackCache.clear();
      void loadOverview();
    }
    observedLibraryVersion = version;
  });

  onMount(() => {
    filesystemPlaylistIoSupported = !/Android|iPhone|iPad/i.test(navigator.userAgent);
    void loadOverview();
  });
</script>

{#snippet listActions(songs: Song[], canRemove = false)}
  {#if songs.length}
    {#if selectionMode}
      {@const selected = selectedFrom(songs)}
      <div class="selection-toolbar" role="toolbar" aria-label="Selection actions">
        <div class="selection-summary"><strong>{bulkBusy ? "Working…" : `${selected.length} selected`}</strong><button class="text-button" type="button" onclick={() => toggleSelectAll(songs)}>{songs.every((song) => selectedSongIds.includes(song.id)) ? "Clear all" : "Select all loaded"}</button><button class="text-button" type="button" onclick={cancelSelection}>Cancel</button></div>
        <div class="bulk-actions">
          <button class="secondary-action" type="button" disabled={!selected.length || bulkBusy} onclick={() => bulkPlayNext(selected)}>Play Next</button>
          <button class="secondary-action" type="button" disabled={!selected.length || bulkBusy} onclick={() => bulkAddToQueue(selected)}>Add to queue</button>
          <button class="secondary-action" type="button" disabled={!selected.length || bulkBusy} onclick={() => bulkSetLiked(selected)}>{selected.length && selected.every((song) => player.isLiked(song.id)) ? "Unlike" : "Like"}</button>
          <button class="secondary-action" type="button" disabled={!selected.length || bulkBusy} onclick={() => bulkDownload(selected)}>Download</button>
          <span class="bulk-playlist-wrap">
            <button class="secondary-action" type="button" aria-expanded={bulkPlaylistOpen} disabled={!selected.length || bulkBusy} onclick={showBulkPlaylists}>Add to playlist…</button>
            {#if bulkPlaylistOpen}<span class="bulk-playlist-menu">{#each playlists as playlist (playlist.id)}<button type="button" disabled={bulkBusy} onclick={() => bulkAddToPlaylist(playlist, selected)}><span>{playlist.name}</span><small>{playlist.trackCount} songs</small></button>{:else}<span class="bulk-menu-note">No playlists yet.</span>{/each}</span>{/if}
          </span>
          {#if canRemove}<button class="secondary-action danger" type="button" disabled={!selected.length || bulkBusy} onclick={() => bulkRemoveFromPlaylist(selected)}>Remove from playlist</button>{/if}
        </div>
      </div>
    {:else}
      <div class="collection-actions page-actions"><button class="primary-action" type="button" onclick={() => player.playAll(songs)}>Play all</button><button class="secondary-action" type="button" onclick={() => player.shuffle(songs)}>Shuffle</button><button class="secondary-action select-action" type="button" onclick={beginSelection}>Select</button></div>
    {/if}
  {/if}
{/snippet}

{#snippet songRows(songs: Song[], detailFor: (song: Song) => string)}
  <div class="song-list library-full-list">
    {#each songs as song (song.id)}
      {#if selectionMode}
        <div class="selectable-song" class:selected={selectedSongIds.includes(song.id)}>
          <div class="selection-row-content" inert><SongRow {song} detail={detailFor(song)} /></div>
          <button class="selection-hit" type="button" aria-label={`${selectedSongIds.includes(song.id) ? "Deselect" : "Select"} ${song.title}`} aria-pressed={selectedSongIds.includes(song.id)} onclick={() => toggleSelected(song.id)}><span aria-hidden="true">{selectedSongIds.includes(song.id) ? "✓" : ""}</span></button>
        </div>
      {:else}<SongRow {song} detail={detailFor(song)} />{/if}
    {/each}
  </div>
{/snippet}

<svelte:head><title>Library · SunnySong</title></svelte:head>
<header class="simple-page-header">
  <button class="icon-button" type="button" aria-label={view === "overview" ? "Back to Home" : "Back to Library"} onclick={back}>←</button>
  <div><h1>{view === "album" ? selectedAlbum?.title ?? "Album" : view === "playlist" ? selectedPlaylist?.name ?? "Playlist" : view === "automatic" ? automaticTitle(automaticCollection) : view === "overview" ? "Library" : view[0].toUpperCase() + view.slice(1)}</h1>{#if view === "album" && selectedAlbum}<p>{selectedAlbum.artistName} · {sourceLabel(selectedAlbum.source)}</p>{:else if view === "playlist" && selectedPlaylist}<p>{selectedPlaylist.trackCount} songs</p>{:else if view === "automatic"}<p>{automaticSongs.length} loaded</p>{/if}</div>
  {#if view === "playlist" && !selectionMode}<div class="compact-menu-wrap"><button class="icon-button" type="button" aria-label="Playlist options" aria-expanded={playlistMenuOpen} onclick={() => playlistMenuOpen = !playlistMenuOpen}>⋮</button>{#if playlistMenuOpen}<div class="compact-popover"><button type="button" disabled={playlistBusy} onclick={() => playlistEditing = !playlistEditing}>{playlistEditing ? "Finish editing" : "Edit tracks"}</button><button type="button" disabled={playlistBusy} onclick={renameCurrentPlaylist}>Rename…</button>{#if filesystemPlaylistIoSupported}<button type="button" disabled={playlistBusy || playlistIoBusy} onclick={exportCurrentPlaylist}>Export M3U8…</button>{/if}<button class="danger" type="button" disabled={playlistBusy} onclick={deleteCurrentPlaylist}>Delete playlist…</button></div>{/if}</div>{/if}
</header>
{#if view === "songs" || view === "albums"}<div class="library-toolbar"><label><span class="sr-only">Search loaded items</span><input placeholder="Search loaded items…" bind:value={itemQuery} /></label><select aria-label="Source" bind:value={sourceFilter}><option value="all">All sources</option><option value="local">Device</option><option value="jellyfin">Jellyfin</option></select>{#if view === "songs"}<select aria-label="Sort loaded songs" bind:value={sort}><option value="title">Title</option><option value="artist">Artist</option><option value="duration">Duration</option></select>{/if}</div>{#if tracksHaveMore || albumsHaveMore}<p class="loaded-scope-note">Search and sort apply to loaded items. Load more to expand the scope.</p>{/if}{/if}

<p class="sr-only" role="status" aria-live="polite">{loading ? "Loading library" : loadingMore ? "Loading more library items" : ""}</p>
{#if loadError}<p class="inline-message" role="alert">{loadError} <button class="text-button" type="button" onclick={retryLoad}>Retry</button></p>{/if}
{#if message}<p class="inline-message" role="status">{message} {#if view === "overview"}<a class="text-button" href="#/settings">Open Settings</a>{/if}</p>{/if}

{#if loading}
  <div class="library-overview-loading"><div class="library-grid">{#each Array(6) as _}<div class="library-card skeleton"></div>{/each}</div></div>
{:else if view === "overview"}
  <div class="library-overview">
    <section class="library-section" aria-labelledby="automatic-collections-title">
      <div class="section-heading"><h2 id="automatic-collections-title">Automatic collections</h2></div>
      <div class="automatic-collection-grid">
        <button type="button" onclick={() => openAutomatic("liked")}><span aria-hidden="true">♥</span><strong>Liked</strong><small>Your liked songs</small></button>
        <button type="button" onclick={() => openAutomatic("downloads")}><span aria-hidden="true">↓</span><strong>Downloads</strong><small>Saved audio</small></button>
        <button type="button" onclick={() => openAutomatic("recent")}><span aria-hidden="true">↻</span><strong>Recently Played</strong><small>Latest listening</small></button>
        <button type="button" onclick={() => openAutomatic("most-played")}><span aria-hidden="true">▥</span><strong>Most Played</strong><small>All-time recap</small></button>
      </div>
    </section>

    {#if tracks.length}
      <section class="library-section" aria-labelledby="library-songs-title">
        <div class="section-heading"><h2 id="library-songs-title">Songs</h2>{#if tracksHaveMore}<button class="library-more" type="button" aria-label="Show all songs" onclick={openSongs}>•••</button>{/if}</div>
        <div class="library-song-grid">{#each tracks as item (item.song.id)}<SongRow song={item.song} detail={item.sourceName} />{/each}</div>
      </section>
    {/if}

    {#if albums.length}
      <section class="library-section" aria-labelledby="library-albums-title">
        <div class="section-heading"><h2 id="library-albums-title">Albums</h2>{#if albumsHaveMore}<button class="library-more" type="button" aria-label="Show all albums" onclick={openAlbums}>•••</button>{/if}</div>
        <div class="library-grid">
          {#each albums as album (album.id)}
            <button class="library-card" type="button" onclick={() => openAlbum(album)}>
              {#if album.thumbnailUrl}<img src={library.image(album.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span class="library-art-fallback" aria-hidden="true">♫</span>{/if}
              <strong>{album.title}</strong><small>{album.artistName}</small><em>{sourceLabel(album.source)} · {album.trackCount} tracks</em>
            </button>
          {/each}
        </div>
      </section>
    {/if}

    <section class="library-section" aria-labelledby="library-playlists-title">
      <div class="section-heading"><h2 id="library-playlists-title">Playlists</h2><button class="library-more" type="button" aria-label="Show all playlists" onclick={openPlaylists}>•••</button></div>
      {#if playlists.length}
        <div class="library-grid">
          {#each playlists.slice(0, 6) as playlist (playlist.id)}
            <button class="library-card playlist-card" type="button" onclick={() => openPlaylist(playlist)}><span class="library-art-fallback" aria-hidden="true">≡</span><strong>{playlist.name}</strong><small>{playlist.trackCount} songs</small><em>Local playlist</em></button>
          {/each}
        </div>
      {:else}<p class="inline-message">Create a playlist to collect songs from your library or Discovery.</p>{/if}
    </section>

    {#if folders.length}
      <section class="library-section" aria-labelledby="library-folders-title">
        <div class="section-heading"><h2 id="library-folders-title">Folders & Libraries</h2>{#if folders.length > 4}<button class="library-more" type="button" aria-label="Show all folders and libraries" onclick={openFolders}>•••</button>{/if}</div>
        <div class="library-location-grid">
          {#each folders.slice(0, 4) as folder (folder.id)}
            <article class="library-location"><span aria-hidden="true">{folder.source === "jellyfin" ? "◉" : "▰"}</span><div><strong>{folder.name}</strong><small>{folder.detail}</small><em>{folder.trackCount} tracks · {sourceLabel(folder.source)}</em></div></article>
          {/each}
        </div>
      </section>
    {/if}
  </div>
{:else if view === "songs"}
  {@const songs = visibleTracks.map((item) => item.song)}
  {@render listActions(songs)}
  {@render songRows(songs, (song) => { const item = visibleTracks.find((entry) => entry.song.id === song.id); return item ? `${item.sourceName}${duration(song.durationMs) ? ` · ${duration(song.durationMs)}` : ""}` : ""; })}
  {#if !songs.length && !tracksHaveMore}<p class="inline-message">{itemQuery.trim() || sourceFilter !== "all" ? "No loaded songs match these filters." : "No songs in the library yet."}</p>{/if}
  {#if tracksHaveMore}<button class="load-more" type="button" disabled={loadingMore} onclick={loadMoreSongs}>{loadingMore ? "Loading…" : "Load more"}</button>{/if}
{:else if view === "albums"}
  <div class="library-album-column">
    {#each visibleAlbums as album (album.id)}
      <button class="library-album-row" type="button" onclick={() => openAlbum(album)}>{#if album.thumbnailUrl}<img src={library.image(album.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span class="library-row-fallback">♫</span>{/if}<span><strong>{album.title}</strong><small>{album.artistName}</small><em>{album.trackCount} tracks · {album.sourceName}</em></span><b aria-hidden="true">›</b></button>
    {/each}
  </div>
  {#if !visibleAlbums.length && !albumsHaveMore}<p class="inline-message">{itemQuery.trim() || sourceFilter !== "all" ? "No loaded albums match these filters." : "No albums in the library yet."}</p>{/if}
  {#if albumsHaveMore}<button class="load-more" type="button" disabled={loadingMore} onclick={loadMoreAlbums}>{loadingMore ? "Loading…" : "Load more"}</button>{/if}
{:else if view === "folders"}
  <div class="library-location-column">{#each folders as folder (folder.id)}<article class="library-location"><span aria-hidden="true">{folder.source === "jellyfin" ? "◉" : "▰"}</span><div><strong>{folder.name}</strong><small>{folder.detail}</small><em>{folder.trackCount} tracks · {sourceLabel(folder.source)}</em></div></article>{/each}</div>
  {#if !folders.length}<p class="inline-message">No folders or remote libraries are configured.</p>{/if}
{:else if view === "playlists"}
  <form class="playlist-create" onsubmit={(event) => { event.preventDefault(); void submitPlaylist(); }}>
    <label for="new-playlist-name">New playlist</label><div><input id="new-playlist-name" maxlength="80" placeholder="Playlist name" bind:value={newPlaylistName} /><button class="primary-action" type="submit" disabled={creatingPlaylist || !newPlaylistName.trim()}>{creatingPlaylist ? "Creating…" : "Create"}</button>{#if filesystemPlaylistIoSupported}<button class="secondary-action" type="button" disabled={playlistIoBusy} onclick={importM3uPlaylist}>{playlistIoBusy ? "Importing…" : "Import M3U…"}</button>{/if}</div>
  </form>
  <div class="library-album-column">{#each playlists as playlist (playlist.id)}<button class="library-album-row" type="button" onclick={() => openPlaylist(playlist)}><span class="library-row-fallback">≡</span><span><strong>{playlist.name}</strong><small>Local playlist</small><em>{playlist.trackCount} songs</em></span><b aria-hidden="true">›</b></button>{/each}</div>
  {#if !playlists.length}<p class="inline-message">No playlists yet.</p>{/if}
{:else if view === "playlist"}
  {@const songs = playlistTracks.map((item) => item.song)}
  {@render listActions(songs, true)}
  {#if playlistEditing}<div class="song-list library-full-list">{#each playlistTracks as item, index (item.song.id)}<div class="editable-song"><SongRow song={item.song} detail="PLAYLIST" /><div class="edit-buttons"><button class="icon-button" type="button" aria-label={`Move ${item.song.title} up`} disabled={playlistBusy || index === 0} onclick={() => movePlaylistSong(index, -1)}>↑</button><button class="icon-button" type="button" aria-label={`Move ${item.song.title} down`} disabled={playlistBusy || index === playlistTracks.length - 1} onclick={() => movePlaylistSong(index, 1)}>↓</button><button class="icon-button danger" type="button" aria-label={`Remove ${item.song.title} from playlist`} disabled={playlistBusy} onclick={() => removePlaylistSong(item.song.id)}>×</button></div></div>{/each}</div>{:else}{@render songRows(songs, () => "PLAYLIST")}{/if}
  {#if !playlistTracks.length}<p class="inline-message">This playlist is empty. Use a song’s ⋮ menu to add music.</p>{/if}
{:else if view === "album"}
  {@const songs = albumTracks.map((item) => item.song)}
  {@render listActions(songs)}
  {@render songRows(songs, (song) => { const item = albumTracks.find((entry) => entry.song.id === song.id); return item ? `${item.sourceName}${duration(song.durationMs) ? ` · ${duration(song.durationMs)}` : ""}` : ""; })}
{:else if view === "automatic"}
  {@render listActions(automaticSongs)}
  {@render songRows(automaticSongs, (song) => duration(song.durationMs))}
  {#if !automaticSongs.length}<p class="inline-message">No songs in this collection yet.</p>{/if}
{/if}
