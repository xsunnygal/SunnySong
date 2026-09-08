<script lang="ts">
  import { goto } from "$app/navigation";
  import { onMount } from "svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import {
    getArtistPage,
    getCollectionSongs,
    getLocalArtists,
    getSearchSuggestions,
    searchCatalog,
    searchLocalMusic,
    type ArtistPage,
    type CatalogArtist,
    type CatalogCollection,
    type CatalogFilter,
    type LocalArtist,
    type Song,
  } from "$lib/api/backend";
  import { library } from "$lib/features/library/library.svelte";
  import { player } from "$lib/features/player/player.svelte";

  const SUBSCRIPTIONS_KEY = "solmusic-artist-subscriptions";
  type LocalCatalogCollection = CatalogCollection & { localLibrary: true };
  const localCollections = new WeakSet<CatalogCollection>();
  const filters: { value: CatalogFilter; label: string }[] = [
    { value: "all", label: "All" },
    { value: "songs", label: "Songs" },
    { value: "channels", label: "Channels" },
    { value: "playlists", label: "Playlists" },
    { value: "albums", label: "Albums" },
  ];

  let query = $state("");
  let filter = $state<CatalogFilter>("all");
  let localSongs = $state<Song[]>([]);
  let localArtists = $state<LocalArtist[]>([]);
  let onlineSongs = $state<Song[]>([]);
  let onlineArtists = $state<CatalogArtist[]>([]);
  let onlineAlbums = $state<CatalogCollection[]>([]);
  let onlinePlaylists = $state<CatalogCollection[]>([]);
  let loading = $state(false);
  let message = $state("Search your local library and YouTube Music.");
  let searchFailed = $state(false);
  let selectedArtist = $state<CatalogArtist | null>(null);
  let artistPage = $state<ArtistPage | null>(null);
  let artistLoading = $state(false);
  let artistMessage = $state("");
  let artistFailed = $state(false);
  let selectedCollection = $state<CatalogCollection | null>(null);
  let collectionSongs = $state<Song[]>([]);
  let collectionLoading = $state(false);
  let collectionMessage = $state("");
  let collectionFailed = $state(false);
  let artistSongMode = $state<"top" | "latest">("top");
  let subscribedArtistIds = $state<string[]>([]);
  let timer: ReturnType<typeof setTimeout>;
  let searchInput = $state<HTMLInputElement | null>(null);
  let suggestions = $state<string[]>([]);
  let suggestionsOpen = $state(false);
  let suggestionIndex = $state(-1);
  let suggestionSequence = 0;
  let hasSearched = $state(false);
  let cachedLocalArtistsVersion = -1;
  let cachedLocalArtists: Promise<LocalArtist[]> | null = null;
  let sequence = 0;
  let artistSequence = 0;
  let collectionSequence = 0;
  let observedLibraryVersion = -1;
  let artistTouchX = 0;
  let artistTouchY = 0;

  const showSongs = $derived(filter === "all" || filter === "songs");
  const showArtists = $derived(filter === "all" || filter === "channels");
  const showAlbums = $derived(filter === "all" || filter === "albums");
  const showPlaylists = $derived(filter === "all" || filter === "playlists");
  const closeLocalArtists = $derived(localArtists.filter((artist) => namesAreClose(query, artist.name)).map(localCatalogArtist));
  const closeOnlineArtists = $derived(onlineArtists.filter((artist) => namesAreClose(query, artist.name)));
  const visibleCloseArtists = $derived.by(() => {
    const artists = [...closeLocalArtists, ...closeOnlineArtists];
    const unique = artists.filter((artist, index) => artists.findIndex((candidate) => normalized(candidate.name) === normalized(artist.name)) === index);
    return filter === "all" ? unique.slice(0, 2) : unique;
  });
  const localAlbums = $derived(collectionsFromSongs(localSongs));
  const visibleArtistSongs = $derived(artistPage ? (artistPage.topSongs.length ? artistPage.topSongs : artistPage.songs) : []);
  const artistArtwork = $derived(artistPage?.artist.thumbnailUrl ?? selectedArtist?.thumbnailUrl ?? null);

  function normalized(value: string) {
    return value.toLocaleLowerCase().normalize("NFKD").replace(/[\u0300-\u036f]/g, "").replace(/[^a-z0-9]+/g, " ").trim();
  }

  function editDistance(left: string, right: string) {
    const previous = Array.from({ length: right.length + 1 }, (_, index) => index);
    for (let leftIndex = 1; leftIndex <= left.length; leftIndex += 1) {
      let diagonal = previous[0];
      previous[0] = leftIndex;
      for (let rightIndex = 1; rightIndex <= right.length; rightIndex += 1) {
        const above = previous[rightIndex];
        previous[rightIndex] = Math.min(
          previous[rightIndex] + 1,
          previous[rightIndex - 1] + 1,
          diagonal + (left[leftIndex - 1] === right[rightIndex - 1] ? 0 : 1),
        );
        diagonal = above;
      }
    }
    return previous[right.length];
  }

  function namesAreClose(search: string, name: string) {
    const left = normalized(search);
    const right = normalized(name);
    if (!left || !right) return false;
    if (left === right || right.includes(left) || left.includes(right)) return true;
    return editDistance(left, right) <= Math.max(2, Math.floor(Math.max(left.length, right.length) * 0.22));
  }

  function localCatalogArtist(artist: LocalArtist): CatalogArtist {
    const matchingSong = localSongs.find((song) => normalized(song.artistName) === normalized(artist.name));
    return {
      id: `local:${artist.id}`,
      name: artist.name,
      thumbnailUrl: matchingSong?.thumbnailUrl ?? null,
      subtitle: `${artist.trackCount} ${artist.trackCount === 1 ? "song" : "songs"} · Local`,
      source: "local",
    };
  }

  function collectionsFromSongs(songs: Song[]): LocalCatalogCollection[] {
    const collections = new Map<string, LocalCatalogCollection>();
    for (const song of songs) {
      if (!song.albumName) continue;
      const id = song.albumId ?? `local-album:${normalized(song.artistName)}:${normalized(song.albumName)}`;
      if (!collections.has(id)) {
        const collection: LocalCatalogCollection = {
          id,
          title: song.albumName,
          subtitle: `${song.artistName} · Local album`,
          thumbnailUrl: song.thumbnailUrl,
          kind: "album",
          localLibrary: true,
        };
        localCollections.add(collection);
        collections.set(id, collection);
      }
    }
    return [...collections.values()];
  }

  function allLocalArtists() {
    if (!cachedLocalArtists || cachedLocalArtistsVersion !== library.version) {
      cachedLocalArtistsVersion = library.version;
      cachedLocalArtists = getLocalArtists("", 100, 0);
    }
    return cachedLocalArtists;
  }

  function searchKeydown(event: KeyboardEvent) {
    if ((event.key === "ArrowDown" || event.key === "ArrowUp") && suggestionsOpen && suggestions.length) {
      event.preventDefault();
      suggestionIndex = (suggestionIndex + (event.key === "ArrowDown" ? 1 : -1) + suggestions.length) % suggestions.length;
    } else if (event.key === "Enter") {
      event.preventDefault();
      clearTimeout(timer);
      if (suggestionsOpen && suggestionIndex >= 0) query = suggestions[suggestionIndex];
      suggestionsOpen = false;
      closeArtist();
      void runSearch();
    } else if (event.key === "Escape") {
      if (suggestionsOpen) {
        event.stopPropagation();
        suggestionsOpen = false;
        suggestionIndex = -1;
      } else {
        query = "";
        clearTimeout(timer);
        closeArtist();
        clearResults();
        hasSearched = false;
        searchFailed = false;
        message = "Search your local library and YouTube Music.";
        (event.currentTarget as HTMLInputElement).blur();
      }
    }
  }

  function changed() {
    clearTimeout(timer);
    const value = query.trim();
    suggestionIndex = -1;
    closeArtist();
    sequence += 1;
    clearResults();
    hasSearched = false;
    message = "";
    searchFailed = false;
    if (!value) {
      suggestionSequence += 1;
      suggestions = [];
      suggestionsOpen = false;
      if (!hasSearched) message = "Search your local library and YouTube Music.";
      return;
    }
    const current = ++suggestionSequence;
    timer = setTimeout(async () => {
      try {
        const next = await getSearchSuggestions(value, 8);
        if (current !== suggestionSequence || query.trim() !== value) return;
        suggestions = next;
        suggestionsOpen = next.length > 0;
      } catch {
        if (current === suggestionSequence) suggestionsOpen = false;
      }
    }, 100);
  }

  function chooseSuggestion(value: string) {
    query = value;
    suggestionsOpen = false;
    suggestionIndex = -1;
    void runSearch();
  }

  function selectFilter(value: CatalogFilter) {
    if (filter === value) return;
    filter = value;
    closeArtist();
    if (query.trim() && hasSearched) void runSearch();
  }

  function clearResults() {
    localSongs = [];
    localArtists = [];
    onlineSongs = [];
    onlineArtists = [];
    onlineAlbums = [];
    onlinePlaylists = [];
    loading = false;
  }

  async function runSearch() {
    const current = ++sequence;
    const value = query.trim();
    if (!value) return;
    loading = true;
    hasSearched = true;
    suggestionsOpen = false;
    suggestionSequence += 1;
    message = "";
    searchFailed = false;
    clearResults();
    loading = true;
    try {
      const tasks: Promise<void>[] = [
        (showSongs || showAlbums || showArtists ? searchLocalMusic(value) : Promise.resolve([])).then((songs) => {
          if (current !== sequence) return;
          localSongs = songs;
          if (songs[0]) player.preload(songs[0]);
        }),
        (showArtists ? allLocalArtists() : Promise.resolve([])).then((artists) => {
          if (current === sequence) localArtists = artists;
        }),
      ];
      if (library.discoveryEnabled) {
        const onlineFilters: CatalogFilter[] = filter === "all"
          ? ["songs", "channels", "albums", "playlists"]
          : [filter];
        for (const onlineFilter of onlineFilters) {
          tasks.push(searchCatalog(value, onlineFilter).then((online) => {
            if (current !== sequence) return;
            if (onlineFilter === "songs") {
              onlineSongs = online.songs;
              if (!localSongs.length && online.songs[0]) player.preload(online.songs[0]);
            } else if (onlineFilter === "channels") onlineArtists = online.artists;
            else if (onlineFilter === "albums") onlineAlbums = online.albums;
            else if (onlineFilter === "playlists") onlinePlaylists = online.playlists;
          }));
        }
      }
      const results = await Promise.allSettled(tasks);
      if (current !== sequence) return;
      const failures = results.filter((result) => result.status === "rejected");
      const resultCount = localSongs.length + localArtists.length + onlineSongs.length + onlineArtists.length + onlineAlbums.length + onlinePlaylists.length;
      searchFailed = failures.length > 0;
      if (resultCount && failures.length) message = typeof navigator !== "undefined" && !navigator.onLine ? "You’re offline. Local results are shown; online categories could not be loaded." : "Some search categories could not be loaded.";
      else if (resultCount) message = "";
      else if (failures[0]?.status === "rejected") message = typeof navigator !== "undefined" && !navigator.onLine ? "Search is unavailable while offline. Check your connection and retry." : failures[0].reason instanceof Error ? failures[0].reason.message : String(failures[0].reason);
      else message = "No matching music found.";
    } catch (error) {
      if (current === sequence) {
        searchFailed = true;
        message = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (current === sequence) loading = false;
    }
  }

  async function openArtist(artist: CatalogArtist) {
    const current = ++artistSequence;
    selectedArtist = artist;
    artistSongMode = "top";
    artistMessage = "";
    artistFailed = false;
    artistLoading = true;
    artistPage = null;
    if (artist.source === "local") {
      try {
        const results = await searchLocalMusic(artist.name);
        if (current !== artistSequence) return;
        const songs = results.filter((song) => normalized(song.artistName) === normalized(artist.name));
        artistPage = {
          artist,
          topSongs: [],
          songs,
          latestReleases: [],
          albums: collectionsFromSongs(songs),
          singles: [],
          playlists: [],
        };
      } catch (error) {
        if (current === artistSequence) {
          artistFailed = true;
          artistMessage = error instanceof Error ? error.message : String(error);
        }
      } finally {
        if (current === artistSequence) artistLoading = false;
      }
      return;
    }
    try {
      const page = await getArtistPage(artist.id);
      if (current !== artistSequence) return;
      artistPage = page;
      const firstSong = page.topSongs[0] ?? page.songs[0];
      if (firstSong) player.preload(firstSong);
    } catch (error) {
      if (current === artistSequence) {
        artistFailed = true;
        artistMessage = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (current === artistSequence) artistLoading = false;
    }
  }

  function isLocalCollection(collection: CatalogCollection): collection is LocalCatalogCollection {
    return localCollections.has(collection);
  }

  async function openCollection(collection: CatalogCollection) {
    const current = ++collectionSequence;
    selectedCollection = collection;
    collectionSongs = [];
    collectionMessage = "";
    collectionFailed = false;
    collectionLoading = true;
    try {
      let songs: Song[];
      if (isLocalCollection(collection)) {
        const sourceSongs = [...(artistPage?.topSongs ?? []), ...(artistPage?.songs ?? []), ...localSongs];
        songs = sourceSongs.filter((song) => song.albumId === collection.id || (song.albumName && normalized(song.albumName) === normalized(collection.title)));
      } else {
        songs = await getCollectionSongs(collection.id);
      }
      if (current !== collectionSequence) return;
      collectionSongs = songs;
      if (!songs.length) collectionMessage = "No songs were reported for this collection.";
      else player.preload(songs[0]);
    } catch (error) {
      if (current === collectionSequence) {
        collectionFailed = true;
        collectionMessage = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (current === collectionSequence) collectionLoading = false;
    }
  }

  function closeCollection() {
    collectionSequence += 1;
    selectedCollection = null;
    collectionSongs = [];
    collectionLoading = false;
    collectionMessage = "";
    collectionFailed = false;
  }

  function closeArtist() {
    artistSequence += 1;
    closeCollection();
    selectedArtist = null;
    artistPage = null;
    artistLoading = false;
    artistMessage = "";
    artistFailed = false;
  }

  function isSubscribed(artistId: string) {
    return subscribedArtistIds.includes(artistId);
  }

  function toggleSubscription(artist: CatalogArtist) {
    subscribedArtistIds = isSubscribed(artist.id)
      ? subscribedArtistIds.filter((id) => id !== artist.id)
      : [...subscribedArtistIds, artist.id];
    localStorage.setItem(SUBSCRIPTIONS_KEY, JSON.stringify(subscribedArtistIds));
  }

  function artistTouchStart(event: TouchEvent) {
    artistTouchX = event.touches[0].clientX;
    artistTouchY = event.touches[0].clientY;
  }

  function artistTouchEnd(event: TouchEvent) {
    const touch = event.changedTouches[0];
    const deltaX = touch.clientX - artistTouchX;
    const deltaY = touch.clientY - artistTouchY;
    if (Math.abs(deltaX) < 55 || Math.abs(deltaX) < Math.abs(deltaY) * 1.25) return;
    artistSongMode = deltaX < 0 ? "latest" : "top";
  }

  $effect(() => {
    const version = library.version;
    if (observedLibraryVersion >= 0 && version !== observedLibraryVersion && query.trim()) void runSearch();
    observedLibraryVersion = version;
  });

  onMount(() => {
    void library.initialize();
    queueMicrotask(() => searchInput?.focus());
    const hashQuery = location.hash.includes("?") ? location.hash.slice(location.hash.indexOf("?") + 1) : "";
    const params = new URLSearchParams(hashQuery);
    const routedQuery = params.get("q")?.trim() ?? "";
    const artistId = params.get("artistId")?.trim() ?? "";
    const artistName = params.get("artistName")?.trim() ?? routedQuery;
    if (routedQuery) query = routedQuery;
    if (artistId && artistName) {
      void openArtist({
        id: artistId,
        name: artistName,
        thumbnailUrl: null,
        subtitle: null,
        source: artistId.startsWith("UC") ? "youtube" : "local",
      });
    } else if (routedQuery) void runSearch();
    try {
      const stored = JSON.parse(localStorage.getItem(SUBSCRIPTIONS_KEY) ?? "[]");
      if (Array.isArray(stored)) subscribedArtistIds = stored.filter((id): id is string => typeof id === "string");
    } catch {
      subscribedArtistIds = [];
    }
  });
</script>

<svelte:head><title>Search · SunnySong</title></svelte:head>

<p class="sr-only" role="status" aria-live="polite">{loading ? "Searching" : artistLoading ? "Loading artist" : collectionLoading ? "Loading collection" : ""}</p>
<div class="search-header">
  <button class="icon-button" type="button" aria-label={selectedCollection ? "Back to artist" : selectedArtist ? "Back to search results" : "Back to Home"} onclick={() => selectedCollection ? closeCollection() : selectedArtist ? closeArtist() : void goto("#/")}>←</button>
  <label class="search-field">
    <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m16 16 4 4" /></svg>
    <span class="sr-only">Search music</span>
    <input bind:this={searchInput} role="combobox" aria-autocomplete="list" aria-expanded={suggestionsOpen} aria-controls="search-suggestions" aria-activedescendant={suggestionIndex >= 0 ? `suggestion-${suggestionIndex}` : undefined} placeholder="Search songs, artists, or lyrics…" bind:value={query} oninput={changed} onkeydown={searchKeydown} onfocus={() => suggestionsOpen = suggestions.length > 0} />
    {#if suggestionsOpen}<div id="search-suggestions" class="search-suggestions" role="listbox" aria-label="Search suggestions">{#each suggestions as suggestion, index}<button id={`suggestion-${index}`} type="button" role="option" aria-selected={index === suggestionIndex} class:active={index === suggestionIndex} onpointerdown={(event) => event.preventDefault()} onclick={() => chooseSuggestion(suggestion)}>{suggestion}</button>{/each}</div>{/if}
  </label>
</div>

{#if selectedArtist || selectedCollection}
  <main class="artist-browser">
    {#if selectedCollection}
      <header class="artist-profile-large collection-profile">
        {#if selectedCollection.thumbnailUrl}<img src={library.image(selectedCollection.thumbnailUrl) ?? ""} alt="" />{:else}<span class="artist-avatar-fallback" aria-hidden="true">♫</span>{/if}
        <div><small>{selectedCollection.kind}</small><h1>{selectedCollection.title}</h1><p>{selectedCollection.subtitle ?? selectedArtist?.name ?? "Collection"}</p></div>
      </header>
      {#if collectionSongs.length}<div class="collection-actions"><button class="primary-action" type="button" onclick={() => player.playAll(collectionSongs)}>Play all</button><button class="secondary-action" type="button" onclick={() => player.shuffle(collectionSongs)}>Shuffle</button></div>{/if}
      {#if collectionLoading}<div class="song-list" aria-label="Loading collection"><div class="row-skeleton skeleton"></div><div class="row-skeleton skeleton"></div><div class="row-skeleton skeleton"></div></div>
      {:else if collectionMessage}<p class="inline-message" role={collectionFailed ? "alert" : "status"}>{collectionMessage} {#if collectionFailed}<button class="text-button" type="button" onclick={() => selectedCollection && openCollection(selectedCollection)}>Retry</button>{/if}</p>
      {:else}<div class="song-list artist-songs">{#each collectionSongs as song (song.id)}<SongRow {song} detail={isLocalCollection(selectedCollection) ? "LOCAL" : "YT"} />{/each}</div>{/if}
    {:else if selectedArtist}
    <header class="artist-profile-large">
      {#if artistArtwork}<img src={library.image(artistArtwork) ?? ""} alt={`${artistPage?.artist.name ?? selectedArtist.name} artist`} />{:else}<span class="artist-avatar-fallback" aria-hidden="true">♫</span>{/if}
      <div><small>{selectedArtist.source === "local" ? "Local artist" : "Artist"}</small><h1>{artistPage?.artist.name ?? selectedArtist.name}</h1><p>{artistPage?.artist.subtitle ?? selectedArtist.subtitle ?? ""}</p></div>
      <button class="primary-action subscribe-button" class:subscribed={isSubscribed(selectedArtist.id)} type="button" aria-pressed={isSubscribed(selectedArtist.id)} onclick={() => selectedArtist && toggleSubscription(selectedArtist)}>{isSubscribed(selectedArtist.id) ? "Subscribed" : "Subscribe"}</button>
    </header>

    {#if artistLoading}<div class="song-list" aria-label="Loading artist"><div class="row-skeleton skeleton"></div><div class="row-skeleton skeleton"></div><div class="row-skeleton skeleton"></div></div>
    {:else if artistMessage}<p class="inline-message" role="alert">{artistMessage} {#if artistFailed}<button class="text-button" type="button" onclick={() => selectedArtist && openArtist(selectedArtist)}>Retry</button>{/if}</p>
    {:else if artistPage}
      <section class="artist-song-section" aria-label="Artist songs and latest releases" ontouchstart={artistTouchStart} ontouchend={artistTouchEnd}>
        {#if visibleArtistSongs.length}<div class="collection-actions"><button class="primary-action" type="button" onclick={() => player.playAll(visibleArtistSongs)}>Play all</button><button class="secondary-action" type="button" onclick={() => player.shuffle(visibleArtistSongs)}>Shuffle</button></div>{/if}
        <div class="section-heading"><h2>{artistPage.topSongs.length ? "Top Songs" : "Songs"}</h2>{#if artistPage.latestReleases.length}<div class="segmented-control compact"><button class:active={artistSongMode === "top"} aria-pressed={artistSongMode === "top"} onclick={() => artistSongMode = "top"}>Top</button><button class:active={artistSongMode === "latest"} aria-pressed={artistSongMode === "latest"} onclick={() => artistSongMode = "latest"}>Latest</button></div>{/if}</div>
        {#if artistSongMode === "top"}
          {#if visibleArtistSongs.length}<div class="song-list artist-songs">{#each visibleArtistSongs as song (song.id)}<SongRow {song} detail={selectedArtist.source === "local" ? "LOCAL" : "YT"} />{/each}</div>{:else}<p class="inline-message">No songs were reported for this artist.</p>{/if}
        {:else}
          <div class="catalog-strip">{#each artistPage.latestReleases as item (item.id)}<button class="catalog-card catalog-card-button" type="button" onclick={() => openCollection(item)}>{#if item.thumbnailUrl}<img src={library.image(item.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span aria-hidden="true">♫</span>{/if}<strong>{item.title}</strong><small>{item.subtitle ?? "Latest release"}</small></button>{/each}</div>
        {/if}
      </section>

      {#each [["Albums", artistPage.albums], ["Singles & EPs", artistPage.singles], ["Playlists", artistPage.playlists]] as section}
        {@const items = section[1] as CatalogCollection[]}
        {#if items.length}<section class="catalog-section"><h2>{section[0]}</h2><div class="catalog-strip">{#each items as item (item.id)}<button class="catalog-card catalog-card-button" type="button" onclick={() => openCollection(item)}>{#if item.thumbnailUrl}<img src={library.image(item.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span aria-hidden="true">♫</span>{/if}<strong>{item.title}</strong><small>{item.subtitle ?? item.kind}</small></button>{/each}</div></section>{/if}
      {/each}
    {/if}
    {/if}
  </main>
{:else}
  <div class="search-filter-bar" role="toolbar" aria-label="Search result type">
    {#each filters as option}<button type="button" class:active={filter === option.value} aria-pressed={filter === option.value} onclick={() => selectFilter(option.value)}>{option.label}</button>{/each}
  </div>

  <main class="catalog-search-results">
    {#if message}<p class="inline-message" role={searchFailed ? "alert" : "status"}>{message} {#if searchFailed}<button class="text-button" type="button" onclick={runSearch}>Retry</button>{/if}</p>{/if}

    {#if showArtists && visibleCloseArtists.length}
      <section class="artist-match-section"><h1 class="compact-title">Artists</h1><div class="artist-match-grid">
        {#each visibleCloseArtists as artist (artist.id)}
          <article class="artist-mini-profile">
            <button class="artist-profile-link" type="button" onclick={() => openArtist(artist)}>
              {#if artist.thumbnailUrl}<img src={library.image(artist.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span class="artist-avatar-fallback" aria-hidden="true">♫</span>{/if}
              <span><strong>{artist.name}</strong><small>{artist.subtitle ?? (artist.source === "local" ? "Local artist" : "YouTube Music artist")}</small></span>
            </button>
            <button class="secondary-action subscribe-button" class:subscribed={isSubscribed(artist.id)} type="button" aria-pressed={isSubscribed(artist.id)} onclick={() => toggleSubscription(artist)}>{isSubscribed(artist.id) ? "Subscribed" : "Subscribe"}</button>
          </article>
        {/each}
      </div></section>
    {/if}

    {#if showSongs && (localSongs.length || onlineSongs.length)}
      <section><h2 class="compact-title">Songs</h2><div class="song-list">{#each localSongs as song (song.id)}<SongRow {song} detail="LOCAL" />{/each}{#each onlineSongs as song (song.id)}<SongRow {song} detail="YT" />{/each}</div></section>
    {/if}

    {#if showAlbums && (localAlbums.length || onlineAlbums.length)}
      <section class="catalog-section"><h2>Albums</h2><div class="catalog-strip">{#each [...localAlbums, ...onlineAlbums] as item (item.id)}<button class="catalog-card catalog-card-button" type="button" onclick={() => openCollection(item)}>{#if item.thumbnailUrl}<img src={library.image(item.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span aria-hidden="true">♫</span>{/if}<strong>{item.title}</strong><small>{item.subtitle ?? "Album"}</small></button>{/each}</div></section>
    {/if}

    {#if showPlaylists && onlinePlaylists.length}
      <section class="catalog-section"><h2>Playlists</h2><div class="catalog-strip">{#each onlinePlaylists as item (item.id)}<button class="catalog-card catalog-card-button" type="button" onclick={() => openCollection(item)}>{#if item.thumbnailUrl}<img src={library.image(item.thumbnailUrl) ?? ""} alt="" loading="lazy" />{:else}<span aria-hidden="true">♫</span>{/if}<strong>{item.title}</strong><small>{item.subtitle ?? "Playlist"}</small></button>{/each}</div></section>
    {/if}
    {#if loading}<div class="song-list progressive-loading" aria-hidden="true"><div class="row-skeleton skeleton"></div><div class="row-skeleton skeleton"></div></div>{/if}
  </main>
{/if}
