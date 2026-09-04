<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import {
    getLibraryAlbums,
    getLibraryAlbumTracks,
    getLibraryFolders,
    getLibraryTracks,
    getPlaylists,
    createPlaylist,
    getPlaylistTracks,
    type LibraryAlbum,
    type LibraryFolder,
    type LibraryTrack,
    type Playlist,
    type PlaylistTrack,
  } from "$lib/api/backend";
  import { library } from "$lib/features/library/library.svelte";

  type LibraryView = "overview" | "songs" | "albums" | "folders" | "album" | "playlists" | "playlist";
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
  let observedLibraryVersion = -1;
  let requestGeneration = 0;
  const albumTrackCache = new Map<string, LibraryTrack[]>();

  const sourceLabel = (source: "local" | "jellyfin") => source === "jellyfin" ? "Jellyfin" : "On this device";

  async function loadOverview() {
    const generation = ++requestGeneration;
    loading = true;
    message = "";
    try {
      const [trackItems, albumItems, folderItems, playlistItems] = await Promise.all([
        getLibraryTracks(TRACK_PREVIEW_COUNT + 1),
        getLibraryAlbums(ALBUM_PREVIEW_COUNT + 1),
        getLibraryFolders(),
        getPlaylists(),
      ]);
      if (generation !== requestGeneration) return;
      tracksHaveMore = trackItems.length > TRACK_PREVIEW_COUNT;
      albumsHaveMore = albumItems.length > ALBUM_PREVIEW_COUNT;
      tracks = trackItems.slice(0, TRACK_PREVIEW_COUNT);
      albums = albumItems.slice(0, ALBUM_PREVIEW_COUNT);
      folders = folderItems;
      playlists = playlistItems;
      if (!tracks.length && !albums.length && !folders.length && !playlists.length) {
        message = "Add a music folder or enable and sync a Jellyfin music library in Settings.";
      }
    } catch (error) {
      if (generation === requestGeneration) message = error instanceof Error ? error.message : String(error);
    } finally {
      if (generation === requestGeneration) loading = false;
    }
  }

  async function openSongs() {
    view = "songs";
    if (tracks.length >= 100 || !tracksHaveMore) return;
    loading = true;
    message = "";
    try {
      const next = await getLibraryTracks(100 - tracks.length, tracks.length);
      tracks = [...tracks, ...next];
      tracksHaveMore = tracks.length >= 100 && next.length > 0;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally { loading = false; }
  }

  async function loadMoreSongs() {
    if (loadingMore) return;
    loadingMore = true;
    try {
      const next = await getLibraryTracks(100, tracks.length);
      tracks = [...tracks, ...next];
      tracksHaveMore = next.length === 100;
    } finally { loadingMore = false; }
  }

  async function openAlbums() {
    view = "albums";
    if (albums.length >= 100 || !albumsHaveMore) return;
    loading = true;
    message = "";
    try {
      const next = await getLibraryAlbums(100 - albums.length, albums.length);
      albums = [...albums, ...next];
      albumsHaveMore = albums.length >= 100 && next.length > 0;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally { loading = false; }
  }

  async function loadMoreAlbums() {
    if (loadingMore) return;
    loadingMore = true;
    try {
      const next = await getLibraryAlbums(100, albums.length);
      albums = [...albums, ...next];
      albumsHaveMore = next.length === 100;
    } finally { loadingMore = false; }
  }

  function openFolders() {
    view = "folders";
  }

  function openPlaylists() {
    view = "playlists";
  }

  async function openPlaylist(playlist: Playlist) {
    selectedPlaylist = playlist;
    view = "playlist";
    loading = true;
    message = "";
    try { playlistTracks = await getPlaylistTracks(playlist.id); }
    catch (error) { message = error instanceof Error ? error.message : String(error); }
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

  async function openAlbum(album: LibraryAlbum) {
    selectedAlbum = album;
    view = "album";
    const cached = albumTrackCache.get(album.id);
    if (cached) {
      albumTracks = cached;
      return;
    }
    loading = true;
    message = "";
    try {
      albumTracks = await getLibraryAlbumTracks(album.id);
      albumTrackCache.set(album.id, albumTracks);
    }
    catch (error) { message = error instanceof Error ? error.message : String(error); }
    finally { loading = false; }
  }

  function back() {
    if (view === "overview") void goto("#/");
    else {
      view = "overview";
      selectedAlbum = null;
      selectedPlaylist = null;
      albumTracks = [];
      playlistTracks = [];
    }
  }

  $effect(() => {
    const version = library.version;
    if (observedLibraryVersion >= 0 && version !== observedLibraryVersion) {
      view = "overview";
      albumTrackCache.clear();
      void loadOverview();
    }
    observedLibraryVersion = version;
  });

  onMount(() => void loadOverview());
</script>

<svelte:head><title>Library · SunnySong</title></svelte:head>
<header class="simple-page-header">
  <button class="icon-button" type="button" aria-label={view === "overview" ? "Back to Home" : "Back to Library"} onclick={back}>←</button>
  <div><h1>{view === "album" ? selectedAlbum?.title ?? "Album" : view === "playlist" ? selectedPlaylist?.name ?? "Playlist" : view === "overview" ? "Library" : view[0].toUpperCase() + view.slice(1)}</h1>{#if view === "album" && selectedAlbum}<p>{selectedAlbum.artistName} · {sourceLabel(selectedAlbum.source)}</p>{:else if view === "playlist" && selectedPlaylist}<p>{selectedPlaylist.trackCount} songs</p>{/if}</div>
</header>

{#if message}<p class="inline-message">{message} {#if view === "overview"}<a class="text-button" href="#/settings">Open Settings</a>{/if}</p>{/if}

{#if loading}
  <div class="library-overview-loading"><div class="library-grid">{#each Array(6) as _}<div class="library-card skeleton"></div>{/each}</div></div>
{:else if view === "overview"}
  <div class="library-overview">
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
  <div class="song-list library-full-list">{#each tracks as item (item.song.id)}<SongRow song={item.song} detail={item.sourceName} />{/each}</div>
  {#if tracksHaveMore}<button class="load-more" type="button" disabled={loadingMore} onclick={loadMoreSongs}>{loadingMore ? "Loading…" : "Load more"}</button>{/if}
{:else if view === "albums"}
  <div class="library-album-column">
    {#each albums as album (album.id)}
      <button class="library-album-row" type="button" onclick={() => openAlbum(album)}>{#if album.thumbnailUrl}<img src={library.image(album.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span class="library-row-fallback">♫</span>{/if}<span><strong>{album.title}</strong><small>{album.artistName}</small><em>{album.trackCount} tracks · {album.sourceName}</em></span><b aria-hidden="true">›</b></button>
    {/each}
  </div>
  {#if albumsHaveMore}<button class="load-more" type="button" disabled={loadingMore} onclick={loadMoreAlbums}>{loadingMore ? "Loading…" : "Load more"}</button>{/if}
{:else if view === "folders"}
  <div class="library-location-column">{#each folders as folder (folder.id)}<article class="library-location"><span aria-hidden="true">{folder.source === "jellyfin" ? "◉" : "▰"}</span><div><strong>{folder.name}</strong><small>{folder.detail}</small><em>{folder.trackCount} tracks · {sourceLabel(folder.source)}</em></div></article>{/each}</div>
{:else if view === "playlists"}
  <form class="playlist-create" onsubmit={(event) => { event.preventDefault(); void submitPlaylist(); }}>
    <label for="new-playlist-name">New playlist</label><div><input id="new-playlist-name" maxlength="80" placeholder="Playlist name" bind:value={newPlaylistName} /><button class="primary-action" type="submit" disabled={creatingPlaylist || !newPlaylistName.trim()}>{creatingPlaylist ? "Creating…" : "Create"}</button></div>
  </form>
  <div class="library-album-column">{#each playlists as playlist (playlist.id)}<button class="library-album-row" type="button" onclick={() => openPlaylist(playlist)}><span class="library-row-fallback">≡</span><span><strong>{playlist.name}</strong><small>Local playlist</small><em>{playlist.trackCount} songs</em></span><b aria-hidden="true">›</b></button>{/each}</div>
{:else if view === "playlist"}
  {#if playlistTracks.length}<div class="song-list library-full-list">{#each playlistTracks as item (item.song.id)}<SongRow song={item.song} detail="PLAYLIST" />{/each}</div>{:else}<p class="inline-message">This playlist is empty. Use a song’s ⋮ menu to add music.</p>{/if}
{:else if view === "album"}
  <div class="song-list library-full-list">{#each albumTracks as item (item.song.id)}<SongRow song={item.song} detail={item.sourceName} />{/each}</div>
{/if}
