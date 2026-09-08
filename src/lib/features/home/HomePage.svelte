<script module lang="ts">
  import type { Recommendation as CachedRecommendation } from "$lib/api/backend";

  interface ProfileQuickPickCache {
    picks: CachedRecommendation[];
    page: number;
    exhausted: boolean;
    tasteRevision: number;
  }
  const profileQuickPickCaches = new Map<string, ProfileQuickPickCache>();
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import RecentCard from "$lib/components/RecentCard.svelte";
  import SongRow from "$lib/components/SongRow.svelte";
  import { getDiscoverFeed, getQuickPicks, getRecentSongs, type DiscoverSection, type QuickPickOptions, type Recommendation, type Song } from "$lib/api/backend";

  import { library } from "$lib/features/library/library.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { revisions } from "$lib/features/revisions.svelte";

  let picks = $state<Recommendation[]>([]);
  let pickPage = $state(0);
  let picksLoading = $state(true);
  let picksError = $state("");
  let picksExhausted = $state(false);
  let recent = $state<Song[]>([]);
  let recentLoading = $state(true);
  let recentError = $state("");
  let picksRequestSequence = 0;
  let recentRequestSequence = 0;
  let picksRequest: Promise<void> | null = null;
  let picksPrefetching = $state(false);
  let pageWaiting = $state(false);
  let homeTouchX = 0;
  let homeTouchY = 0;
  let pullDistance = $state(0);
  let quickTouchX = 0;
  let quickTouchY = 0;
  let quickWheelDistance = 0;
  let quickWheelLockedUntil = 0;
  let quickWheelReset: ReturnType<typeof setTimeout> | undefined;
  let discoverSections = $state<DiscoverSection[]>([]);
  let discoverLoading = $state(false);
  let discoverMessage = $state("");
  let discoverLoadedKey = $state("");
  let discoverRequestedKey = "";
  let discoverRequestGeneration = 0;
  let mounted = false;
  let loadedProfileId = "";
  let loadedProfileRevision = -1;
  let pickOptionsOpen = $state(false);
  let pickOptions = $state<QuickPickOptions>({ diverse: true, newSongs: true, rediscover: true });
  const QUICK_PICK_PAGE_SIZE = 4;

  function pickFingerprint(pick: Recommendation) {
    return `${pick.song.artistName.toLocaleLowerCase().replace(/[^a-z0-9]+/g, " ").trim()}::${pick.song.title.toLocaleLowerCase().replace(/[^a-z0-9]+/g, " ").trim()}`;
  }

  const visiblePicks = $derived(picks.slice(
    pickPage * QUICK_PICK_PAGE_SIZE,
    pickPage * QUICK_PICK_PAGE_SIZE + QUICK_PICK_PAGE_SIZE,
  ));

  async function loadRecent(reset = false) {
    if (recentLoading && !reset) return;
    const requestSequence = ++recentRequestSequence;
    const profileId = loadedProfileId;
    recentLoading = true;
    recentError = "";
    try {
      const page = await getRecentSongs(10, null);
      if (requestSequence !== recentRequestSequence || loadedProfileId !== profileId) return;
      recent = page.items;
    } catch (error) {
      if (requestSequence === recentRequestSequence && loadedProfileId === profileId) recentError = error instanceof Error ? error.message : String(error);
    } finally {
      if (requestSequence === recentRequestSequence) recentLoading = false;
    }
  }

  async function loadPicks(reset = false, count = QUICK_PICK_PAGE_SIZE, replacementExclude: string[] = []) {
    if (picksRequest && !reset) {
      await picksRequest;
      return;
    }
    const requestSequence = ++picksRequestSequence;
    const request = (async () => {
      if (reset) picksLoading = true;
      else picksPrefetching = true;
      picksError = "";
      try {
        const replacing = reset ? [...picks] : [];
        const existing = reset ? [] : [...picks];
        const accumulated: Recommendation[] = [];
        const rejectedIds: string[] = [];
        const blockedFingerprints = new Set([...replacing, ...existing].map(pickFingerprint));
        let noMoreResults = false;
        for (let attempt = 0; attempt < 3 && accumulated.length < count; attempt += 1) {
          const excluded = [...new Set([...replacementExclude, ...rejectedIds, ...existing.map((pick) => pick.song.id), ...accumulated.map((pick) => pick.song.id)])];
          const next = await getQuickPicks(count - accumulated.length, excluded, pickOptions);
          if (requestSequence !== picksRequestSequence) return;
          if (!next.length) {
            noMoreResults = true;
            break;
          }
          const before = accumulated.length;
          for (const pick of next) {
            const fingerprint = pickFingerprint(pick);
            if (blockedFingerprints.has(fingerprint)) {
              rejectedIds.push(pick.song.id);
              continue;
            }
            if (![...existing, ...accumulated].some((item) => item.song.id === pick.song.id)) {
              accumulated.push(pick);
              blockedFingerprints.add(fingerprint);
            }
          }
          if (accumulated.length === before && rejectedIds.length === 0) {
            noMoreResults = true;
            break;
          }
          picks = reset ? [...accumulated] : [...existing, ...accumulated];
          if (reset) pickPage = 0;
          cachePicks();
        }
        for (const pick of accumulated) if (pick.reasons.includes("Liked track")) player.seedLiked(pick.song.id);
        picks = reset ? accumulated : [...existing, ...accumulated];
        picksExhausted = noMoreResults;
        if (reset) pickPage = 0;
        cachePicks();
      } catch (error) {
        if (requestSequence === picksRequestSequence) picksError = error instanceof Error ? error.message : String(error);
      } finally {
        if (requestSequence === picksRequestSequence) {
          picksLoading = false;
          picksPrefetching = false;
        }
      }
    })();
    picksRequest = request;
    try { await request; }
    finally { if (picksRequest === request) picksRequest = null; }
  }

  async function ensurePage(page: number) {
    if (picksExhausted) return;
    if (picksRequest) await picksRequest;
    const targetCount = (page + 1) * QUICK_PICK_PAGE_SIZE;
    const missing = targetCount - picks.length;
    if (missing > 0 && !picksExhausted) await loadPicks(false, missing);
  }

  async function prefetchFollowingPage() {
    await ensurePage(pickPage + 1);
  }

  function cachePicks() {
    if (!loadedProfileId) return;
    profileQuickPickCaches.set(loadedProfileId, {
      picks: [...picks],
      page: pickPage,
      exhausted: picksExhausted,
      tasteRevision: revisions.taste,
    });
  }

  async function restoreCachedPage(requestedPage: number) {
    const requiredCount = (requestedPage + 1) * QUICK_PICK_PAGE_SIZE;
    if (picks.length < requiredCount && !picksExhausted) {
      await loadPicks(false, requiredCount - picks.length);
    }
    if (picks.length > requestedPage * QUICK_PICK_PAGE_SIZE) {
      pickPage = requestedPage;
      cachePicks();
      void prefetchFollowingPage();
    }
  }

  function optionsKey(profileId: string) {
    return `solmusic-quick-pick-options:${profileId}`;
  }

  function loadPickOptions(profileId: string): QuickPickOptions {
    try {
      const stored = JSON.parse(localStorage.getItem(optionsKey(profileId)) ?? "null");
      return {
        diverse: stored?.diverse !== false,
        newSongs: stored?.newSongs !== false,
        rediscover: stored?.rediscover !== false,
      };
    } catch { return { diverse: true, newSongs: true, rediscover: true }; }
  }

  function setPickOption(option: keyof QuickPickOptions, enabled: boolean) {
    pickOptions = { ...pickOptions, [option]: enabled };
    if (loadedProfileId) {
      localStorage.setItem(optionsKey(loadedProfileId), JSON.stringify(pickOptions));
      profileQuickPickCaches.delete(loadedProfileId);
    }
    void loadPicks(true, QUICK_PICK_PAGE_SIZE, picks.map((pick) => pick.song.id)).then(prefetchFollowingPage);
  }

  function activateProfile(profileId: string, profileRevision: number, tasteRevision: number) {
    const revision = profileRevision * 1_000_000 + tasteRevision;
    if (!mounted || (loadedProfileId === profileId && loadedProfileRevision === revision)) return;
    loadedProfileId = profileId;
    loadedProfileRevision = revision;
    pickOptions = loadPickOptions(profileId);
    pickOptionsOpen = false;
    picksRequestSequence += 1;
    recentRequestSequence += 1;
    const existingCache = profileQuickPickCaches.get(profileId);
    const cache = existingCache?.tasteRevision === tasteRevision ? existingCache : undefined;
    if (existingCache && !cache) profileQuickPickCaches.delete(profileId);
    picks = cache?.picks ?? [];
    picksExhausted = cache?.exhausted ?? false;
    const requestedPage = cache?.page ?? 0;
    const requestedPageComplete = picks.length >= (requestedPage + 1) * QUICK_PICK_PAGE_SIZE;
    const lastCompletePage = Math.max(0, Math.floor(picks.length / QUICK_PICK_PAGE_SIZE) - 1);
    pickPage = requestedPageComplete || picksExhausted
      ? requestedPage
      : Math.min(requestedPage, lastCompletePage);
    picksError = "";
    picksLoading = false;
    recent = [];
    recentError = "";
    recentLoading = false;
    void loadRecent(true);
    if (!cache?.picks.length) void loadPicks(true).then(prefetchFollowingPage);
    else if (!requestedPageComplete && !picksExhausted) void restoreCachedPage(requestedPage);
    else void prefetchFollowingPage();
  }

  function pickReached() {
    void prefetchFollowingPage();
  }

  function previousPicks() {
    if (pickPage === 0) return;
    pickPage -= 1;
    cachePicks();
    void prefetchFollowingPage();
  }

  function nextPicks() {
    if (pageWaiting || (picksExhausted && (pickPage + 1) * QUICK_PICK_PAGE_SIZE >= picks.length)) return;
    pickPage += 1;
    pageWaiting = true;
    void ensurePage(pickPage).finally(() => {
      pageWaiting = false;
      if (picksExhausted && pickPage * QUICK_PICK_PAGE_SIZE >= picks.length) pickPage = Math.max(0, pickPage - 1);
      else {
        cachePicks();
        void prefetchFollowingPage();
      }
    });
  }

  function homeTouchStart(event: TouchEvent) {
    const touch = event.touches[0];
    homeTouchX = touch.clientX;
    homeTouchY = touch.clientY;
  }

  function homeTouchMove(event: TouchEvent) {
    if (window.scrollY > 0 || picksLoading) return;
    const touch = event.touches[0];
    const deltaX = touch.clientX - homeTouchX;
    const deltaY = touch.clientY - homeTouchY;
    if (deltaY > 0 && Math.abs(deltaY) > Math.abs(deltaX) * 1.25) {
      pullDistance = Math.min(88, deltaY * 0.55);
    }
  }

  function homeTouchEnd() {
    const shouldRefresh = pullDistance >= 64;
    pullDistance = 0;
    if (shouldRefresh) void refreshHome();
  }

  function quickTouchStart(event: TouchEvent) {
    void prefetchFollowingPage();
    const touch = event.touches[0];
    quickTouchX = touch.clientX;
    quickTouchY = touch.clientY;
  }

  function quickTouchEnd(event: TouchEvent) {
    const touch = event.changedTouches[0];
    const deltaX = touch.clientX - quickTouchX;
    const deltaY = touch.clientY - quickTouchY;
    if (Math.abs(deltaX) < 55 || Math.abs(deltaX) < Math.abs(deltaY) * 1.25) return;
    event.preventDefault();
    if (deltaX < 0) nextPicks();
    else previousPicks();
  }

  function quickWheel(event: WheelEvent) {
    const delta = Math.abs(event.deltaX) > Math.abs(event.deltaY) ? event.deltaX : event.deltaY;
    if (!delta) return;
    event.preventDefault();
    clearTimeout(quickWheelReset);
    quickWheelDistance += delta;
    quickWheelReset = setTimeout(() => quickWheelDistance = 0, 140);
    if (Math.abs(quickWheelDistance) < 45 || Date.now() < quickWheelLockedUntil) return;
    const direction = quickWheelDistance;
    quickWheelDistance = 0;
    quickWheelLockedUntil = Date.now() + 420;
    if (direction > 0) nextPicks();
    else previousPicks();
  }


  function discoverKey() {
    return `${profiles.active?.id ?? ""}:${profiles.revision}:${revisions.taste}:${revisions.library}:${library.discoveryEnabled}`;
  }

  async function loadDiscover(key = discoverKey()) {
    if (!profiles.active) return;
    const generation = ++discoverRequestGeneration;
    discoverRequestedKey = key;
    discoverLoading = true;
    discoverMessage = "";
    try {
      const next = await getDiscoverFeed(10);
      if (generation !== discoverRequestGeneration || discoverKey() !== key) return;
      discoverSections = next;
      discoverLoadedKey = key;
      if (!next.some((section) => section.items.length)) discoverMessage = "Nothing to discover yet. Listen to or like a few songs, then refresh.";
    } catch (error) {
      if (generation === discoverRequestGeneration && discoverKey() === key) {
        discoverMessage = typeof navigator !== "undefined" && !navigator.onLine
          ? "Discover could not refresh while offline."
          : error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (generation === discoverRequestGeneration) discoverLoading = false;
    }
  }

  async function refreshHome() {
    const previousPickIds = picks.map((pick) => pick.song.id);
    await Promise.all([loadRecent(true), loadPicks(true, QUICK_PICK_PAGE_SIZE, previousPickIds), loadDiscover(discoverKey())]);
    void prefetchFollowingPage();
  }

  async function retryPicks() {
    picksError = "";
    if (!picks.length) await loadPicks(true);
    else await ensurePage(pickPage);
    if (!picksError) void prefetchFollowingPage();
  }

  $effect(() => {
    const profileId = profiles.active?.id;
    const profileRevision = profiles.revision;
    const tasteRevision = revisions.taste;
    if (profileId) activateProfile(profileId, profileRevision, tasteRevision);
  });

  $effect(() => {
    const key = discoverKey();
    if (profiles.active && library.initialized && key !== discoverRequestedKey) void loadDiscover(key);
  });

  onMount(() => {
    mounted = true;
    recentLoading = false;
    picksLoading = false;
    void Promise.all([profiles.initialize(), library.initialize()]);
    if (profiles.active) activateProfile(profiles.active.id, profiles.revision, revisions.taste);
    return () => clearTimeout(quickWheelReset);
  });
</script>

<svelte:head><title>Home · SunnySong</title><meta name="description" content="Your recent listening and local Quick Picks." /></svelte:head>

<div class="home-page" role="region" aria-label="Home content" ontouchstart={homeTouchStart} ontouchmove={homeTouchMove} ontouchend={homeTouchEnd} style={`--pull-distance: ${pullDistance}px`}>
  <div class="pull-refresh" class:ready={pullDistance >= 64} aria-hidden="true">{picksLoading ? "Refreshing…" : pullDistance >= 64 ? "Release to refresh" : "Pull to refresh"}</div>
  <p class="sr-only" role="status" aria-live="polite">{picksLoading ? "Loading Quick Picks" : recentLoading ? "Loading recently played tracks" : ""}</p>
  <section class="home-section" aria-labelledby="recent-title">
    <div class="section-heading">
      <h1 id="recent-title">Recently Played</h1>
      <a class="text-button" href="#/history">More <span aria-hidden="true">→</span></a>
    </div>
    {#if recent.length}
      <div class="recent-strip">
        {#each recent as song (song.id)}<RecentCard {song} />{/each}
        {#if recentLoading}<div class="recent-loading" aria-hidden="true"></div>{/if}
      </div>
      {#if recentError}<p class="inline-message" role="alert">Could not load more recent plays. <button class="text-button" type="button" onclick={() => loadRecent(false)}>Retry</button></p>{/if}
    {:else if recentLoading}
      <div class="recent-strip" aria-label="Loading recently played tracks">{#each Array(5) as _}<div class="recent-skeleton skeleton"></div>{/each}</div>
    {:else if recentError}
      <p class="inline-message" role="alert">Could not load recent plays. <button class="text-button" type="button" onclick={() => loadRecent(true)}>Retry</button></p>
    {:else}
      <p class="inline-message">Your recently played songs will appear here.</p>
    {/if}
  </section>

  <div class="home-section-group">
  <section class="home-section quick-picks" aria-labelledby="quick-picks-title">
    <div class="section-heading">
      <h2 id="quick-picks-title">Quick Picks</h2>
      <div class="quick-pick-heading-actions">
        <div class="quick-pick-options">
          <button class="icon-button" type="button" aria-label="Quick Pick options" aria-expanded={pickOptionsOpen} onclick={() => pickOptionsOpen = !pickOptionsOpen}>⋮</button>
          {#if pickOptionsOpen}<div class="quick-pick-options-menu">
            <label><input type="checkbox" checked={pickOptions.diverse} onchange={(event) => setPickOption("diverse", event.currentTarget.checked)} /><span><strong>More diverse</strong></span></label>
            <label><input type="checkbox" checked={pickOptions.newSongs} onchange={(event) => setPickOption("newSongs", event.currentTarget.checked)} /><span><strong>New songs</strong></span></label>
            <label><input type="checkbox" checked={pickOptions.rediscover} onchange={(event) => setPickOption("rediscover", event.currentTarget.checked)} /><span><strong>Rediscover</strong></span></label>
          </div>{/if}
        </div>
        <button class="icon-button desktop-refresh" type="button" aria-label="Refresh Home" onclick={refreshHome} disabled={picksLoading}>
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7" /></svg>
        </button>
      </div>
    </div>

    {#if visiblePicks.length || pickPage > 0 || picksLoading || picksPrefetching}
      {#key pickPage}
        <div class="quick-page" role="group" aria-label={`Quick Picks page ${pickPage + 1}. Swipe or scroll to change page.`} ontouchstart={quickTouchStart} ontouchend={quickTouchEnd} onwheel={quickWheel}>
          {#each Array(QUICK_PICK_PAGE_SIZE) as _, index}
            {@const pick = visiblePicks[index]}
            {#if pick}
              <div class="quick-pick-row" role="group" aria-label={`Quick Pick ${pickPage * QUICK_PICK_PAGE_SIZE + index + 1}`} onfocusin={pickReached} onpointerenter={pickReached}>
                <SongRow song={pick.song} detail={pick.song.id.startsWith("local:") || pick.song.id.startsWith("jellyfin:") ? "LIBRARY" : undefined} />
              </div>
            {:else if picksLoading || picksPrefetching || pageWaiting}<div class="row-skeleton skeleton" aria-hidden="true"></div>{/if}
          {/each}
        </div>
      {/key}
      {#if picksError}<p class="inline-message" role="alert">Could not load recommendations{typeof navigator !== "undefined" && !navigator.onLine ? " while offline" : ""}. <button class="text-button" type="button" onclick={retryPicks}>Retry</button></p>{/if}
      <div class="paging-controls">
        <button class="icon-button paging-arrow" type="button" aria-label="Previous Quick Picks" disabled={pickPage === 0 || pageWaiting} onclick={previousPicks}>←</button>
        <span class="page-indicator" aria-label={`Quick Picks page ${pickPage + 1}`}>{pickPage + 1}</span>
        <button class="icon-button paging-arrow" type="button" aria-label="More Quick Picks" disabled={picksExhausted && (pickPage + 1) * QUICK_PICK_PAGE_SIZE >= picks.length} onclick={nextPicks}>{pageWaiting ? "…" : "→"}</button>
      </div>
    {:else if picksLoading}
      <div class="quick-page" aria-label="Loading Quick Picks">{#each Array(QUICK_PICK_PAGE_SIZE) as _}<div class="row-skeleton skeleton"></div>{/each}</div>
    {:else}
      <div class="empty-home">
        <p class:danger={!!picksError} role={picksError ? "alert" : undefined}>{picksError ? `Could not load recommendations${typeof navigator !== "undefined" && !navigator.onLine ? " while offline" : ""}.` : "Start listening and recommendations will appear here."}</p>
        {#if picksError}<button class="primary-action" type="button" onclick={retryPicks}>Retry</button>{:else}<a class="primary-action" href="#/search">Search music</a>{/if}
      </div>
    {/if}
  </section>

  {#if !library.discoveryEnabled}<p class="inline-message">Online suggestions are off. Showing recommendations from your library.</p>{/if}
  <p class="sr-only" role="status" aria-live="polite">{discoverLoading ? "Loading Discover recommendations" : ""}</p>
  {#if discoverMessage}<p class="inline-message" role="alert">{discoverMessage} <button class="text-button" type="button" onclick={() => loadDiscover(discoverKey())}>Retry</button></p>{/if}
  {#if discoverLoading && discoverLoadedKey !== discoverKey()}<div class="song-list" aria-hidden="true">{#each Array(5) as _}<div class="row-skeleton skeleton"></div>{/each}</div>{/if}
  {#if discoverLoadedKey === discoverKey()}
    <div class="discover-sections home-discover-sections">
      {#each discoverSections as section (section.id)}
        {#if section.items.length}
          <section class="home-section">
            <div class="section-heading"><div><h2>{section.title}</h2></div><div class="collection-actions"><button class="text-button" type="button" onclick={() => player.playAll(section.items.map((item) => item.song))}>Play all</button><button class="text-button" type="button" onclick={() => player.shuffle(section.items.map((item) => item.song))}>Shuffle</button></div></div>
            <div class="song-list">{#each section.items as item (item.song.id)}<SongRow song={item.song} />{/each}</div>
          </section>
        {/if}
      {/each}
    </div>
  {/if}
  </div>
</div>
