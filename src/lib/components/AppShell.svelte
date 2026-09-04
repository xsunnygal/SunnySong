<script lang="ts">
  import { onBackButtonPress } from "@tauri-apps/api/app";

  import { beforeNavigate, goto } from "$app/navigation";
  import { onMount, type Snippet } from "svelte";
  import { fly } from "svelte/transition";
  import SongActionsMenu from "$lib/components/SongActionsMenu.svelte";
  import { backgroundApp, getSongLyrics, type SongLyrics } from "$lib/api/backend";
  import { downloads } from "$lib/features/downloads/downloads.svelte";
  import { motion } from "$lib/features/motion/motion.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { profiles } from "$lib/features/profiles/profiles.svelte";
  import { library } from "$lib/features/library/library.svelte";
  import { openSongArtist } from "$lib/features/search/artist-navigation";
  import { theme } from "$lib/features/theme/theme.svelte";

  interface Props { children: Snippet; }
  let { children }: Props = $props();
  let menuOpen = $state(false);
  let nowPlayingOpen = $state(false);
  let nowPlayingTouchX = 0;
  let nowPlayingTouchY = 0;
  let nowPlayingGestureEnabled = false;
  let nowPlayingCanClose = false;
  let profilesOpen = $state(false);
  let profileMode = $state<"list" | "create" | "rename">("list");
  let profileName = $state("");
  let renameProfileId = $state<string | null>(null);
  let deleteProfileId = $state<string | null>(null);
  let profileFeedback = $state("");
  let lyricsOpen = $state(false);
  let lyrics = $state<SongLyrics | null>(null);
  let lyricsLoading = $state(false);
  let lyricsLoaded = $state(false);
  let lyricsError = $state("");
  let lyricsRequest = 0;
  const lyricsCache = new Map<string, SongLyrics | null>();
  const lyricsFollowingBySong = new Map<string, boolean>();
  let lyricsPanel = $state<HTMLElement | null>(null);
  let lyricsProgrammaticScrollUntil = 0;
  let lyricsFollowing = $state(true);
  let lyricsFollowingSongId = "";
  let queueList = $state<HTMLOListElement | null>(null);

  interface TimedLyricLine { timeMs: number; text: string; }
  const timedLyrics = $derived(parseTimedLyrics(lyrics?.text ?? ""));
  const activeLyricIndex = $derived.by(() => {
    let active = -1;
    for (let index = 0; index < timedLyrics.length; index += 1) {
      if (timedLyrics[index].timeMs <= player.visiblePositionMs + 250) active = index;
      else break;
    }
    return active;
  });

  function parseTimedLyrics(text: string): TimedLyricLine[] {
    const lines: TimedLyricLine[] = [];
    for (const raw of text.split(/\r?\n/)) {
      const timestamps = [...raw.matchAll(/\[(\d{1,3}):(\d{2})(?:[.:](\d{1,3}))?\]/g)];
      const content = raw.replace(/\[[^\]]+\]/g, "").trim();
      for (const match of timestamps) {
        const fraction = match[3] ? Number(match[3].padEnd(3, "0").slice(0, 3)) : 0;
        lines.push({ timeMs: Number(match[1]) * 60_000 + Number(match[2]) * 1_000 + fraction, text: content || "♪" });
      }
    }
    return lines.sort((left, right) => left.timeMs - right.timeMs);
  }

  function setLyricsFollowing(value: boolean) {
    const songId = player.visibleCurrent?.id;
    if (!songId) return;
    lyricsFollowing = value;
    lyricsFollowingBySong.set(songId, value);
  }

  function lyricsScrolled() {
    if (Date.now() < lyricsProgrammaticScrollUntil || activeLyricIndex < 0 || !lyricsPanel) return;
    const active = lyricsPanel.querySelector<HTMLElement>(`[data-lyric-index="${activeLyricIndex}"]`);
    if (!active) return;
    const panel = lyricsPanel.getBoundingClientRect();
    const line = active.getBoundingClientRect();
    const distance = Math.abs((line.top + line.height / 2) - (panel.top + panel.height / 2));
    if (distance > panel.height * 0.35) setLyricsFollowing(false);
  }

  function resumeLyrics() {
    setLyricsFollowing(true);
    scrollActiveLyric();
  }

  function scrollActiveLyric() {
    if (!lyricsPanel || activeLyricIndex < 0) return;
    lyricsProgrammaticScrollUntil = Date.now() + 700;
    lyricsPanel.querySelector<HTMLElement>(`[data-lyric-index="${activeLyricIndex}"]`)?.scrollIntoView({ behavior: "smooth", block: "center" });
  }

  beforeNavigate(({ willUnload }) => {
    if (!willUnload) {
      nowPlayingOpen = false;
      lyricsOpen = false;
      if (player.playing) player.preservePlaybackAfterNavigation();
    }
  });

  onMount(() => {
    theme.initialize();
    motion.initialize();
    void library.initialize();
    void profiles.initialize()
      .then(() => player.initialize(profiles.active?.id))
      .catch(() => player.initialize());

    let disposed = false;
    let removeBackListener: (() => void) | undefined;
    const closeNowPlayingForNavigation = () => { nowPlayingOpen = false; };
    window.addEventListener("solmusic:close-now-playing", closeNowPlayingForNavigation);
    void onBackButtonPress(() => {
      const awayFromHome = window.location.hash !== "#/" && window.location.hash !== "" && window.location.hash !== "#";
      if (awayFromHome || nowPlayingOpen || menuOpen || profilesOpen || downloads.open) {
        menuOpen = false;
        nowPlayingOpen = false;
        profilesOpen = false;
        deleteProfileId = null;
        downloads.close();
        if (awayFromHome) void goto("#/");
      } else {
        void backgroundApp();
      }
    }).then((listener) => {
      if (disposed) listener.unregister();
      else removeBackListener = () => listener.unregister();
    }).catch(() => {
      // System back integration is only available in the native Android shell.
    });

    return () => {
      disposed = true;
      window.removeEventListener("solmusic:close-now-playing", closeNowPlayingForNavigation);
      removeBackListener?.();
    };
  });

  async function loadLyrics(songId: string) {
    if (lyricsCache.has(songId)) {
      lyrics = lyricsCache.get(songId) ?? null;
      lyricsLoaded = true;
      lyricsLoading = false;
      lyricsError = "";
      return;
    }
    const request = ++lyricsRequest;
    lyricsLoading = true;
    lyricsLoaded = false;
    lyricsError = "";
    try {
      const result = await getSongLyrics(songId);
      if (request !== lyricsRequest || player.visibleCurrent?.id !== songId) return;
      if (lyricsCache.size >= 100) lyricsCache.delete(lyricsCache.keys().next().value!);
      lyricsCache.set(songId, result);
      lyrics = result;
      lyricsLoaded = true;
    } catch (error) {
      if (request === lyricsRequest) lyricsError = error instanceof Error ? error.message : String(error);
    } finally {
      if (request === lyricsRequest) lyricsLoading = false;
    }
  }

  function toggleLyrics() {
    lyricsOpen = !lyricsOpen;
    const songId = player.visibleCurrent?.id;
    if (lyricsOpen && songId) void loadLyrics(songId);
  }

  $effect(() => {
    const songId = player.visibleCurrent?.id;
    if (songId && songId !== lyricsFollowingSongId) {
      lyricsFollowingSongId = songId;
      lyricsFollowing = lyricsFollowingBySong.get(songId) ?? true;
    }
    const windowTitle = player.visibleCurrent ? `${player.visibleCurrent.title} · ${player.visibleCurrent.artistName}` : "SunnySong";
    document.title = windowTitle;
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) => getCurrentWindow().setTitle(windowTitle)).catch(() => {});
    if (lyricsOpen && songId) void loadLyrics(songId);
  });

  $effect(() => {
    const index = activeLyricIndex;
    if (lyricsOpen && timedLyrics.length && lyricsFollowing && index >= 0) queueMicrotask(scrollActiveLyric);
  });

  $effect(() => {
    const index = player.visibleCurrentIndex;
    if (nowPlayingOpen && index !== null) queueMicrotask(() => queueList?.querySelector<HTMLElement>("[aria-current='true']")?.scrollIntoView({ behavior: "smooth", block: "center" }));
  });

  const time = (milliseconds: number) => {
    const seconds = Math.floor(milliseconds / 1000);
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  };

  function nowPlayingTouchStart(event: TouchEvent) {
    const target = event.target as HTMLElement;
    nowPlayingGestureEnabled = !target.closest("button, input, a");
    nowPlayingCanClose = (event.currentTarget as HTMLElement).scrollTop <= 0;
    const touch = event.touches[0];
    nowPlayingTouchX = touch.clientX;
    nowPlayingTouchY = touch.clientY;
  }

  function nowPlayingTouchEnd(event: TouchEvent) {
    if (!nowPlayingGestureEnabled) return;
    const touch = event.changedTouches[0];
    const deltaX = touch.clientX - nowPlayingTouchX;
    const deltaY = touch.clientY - nowPlayingTouchY;
    if (Math.abs(deltaX) > 60 && Math.abs(deltaX) > Math.abs(deltaY) * 1.25) {
      event.preventDefault();
      if (deltaX < 0) void player.next();
      else void player.previous();
    } else if (nowPlayingCanClose && deltaY > 90 && Math.abs(deltaY) > Math.abs(deltaX) * 1.25) {
      event.preventDefault();
      nowPlayingOpen = false;
    }
  }


  function openProfiles() {
    menuOpen = false;
    profileMode = "list";
    profileName = "";
    renameProfileId = null;
    deleteProfileId = null;
    profilesOpen = true;
  }

  async function switchProfile(profileId: string) {
    if (profiles.active?.id === profileId || profiles.changing) return;
    try {
      await player.changeListeningProfile(profileId, () => profiles.select(profileId).then(() => undefined));
      profileFeedback = `Switched to ${profiles.active?.name ?? "profile"}`;
      profilesOpen = false;
      await goto("#/");
      setTimeout(() => { profileFeedback = ""; }, 2200);
    } catch {
      // The profile controller exposes the actionable error inside the dialog.
    }
  }

  async function saveProfile() {
    const name = profileName.trim();
    if (!name || profiles.changing) return;
    try {
      if (profileMode === "create") {
        const created = await profiles.create(name);
        await switchProfile(created.id);
      } else if (renameProfileId) {
        await profiles.rename(renameProfileId, name);
        profileMode = "list";
        profileName = "";
        renameProfileId = null;
      }
    } catch {
      // The profile controller exposes the actionable error inside the dialog.
    }
  }

  function beginRename(profileId: string, name: string) {
    renameProfileId = profileId;
    profileName = name;
    profileMode = "rename";
  }

  async function confirmDeleteProfile() {
    if (!deleteProfileId || profiles.items.length <= 1) return;
    const deletingActive = profiles.active?.id === deleteProfileId;
    const fallback = profiles.items.find((item) => item.id !== deleteProfileId);
    try {
      if (deletingActive && fallback) {
        await player.changeListeningProfile(fallback.id, async () => {
          await profiles.remove(deleteProfileId!);
        });
      } else await profiles.remove(deleteProfileId);
      deleteProfileId = null;
      profileMode = "list";
    } catch {
      // The profile controller exposes the actionable error inside the dialog.
    }
  }


  function keyboard(event: KeyboardEvent) {
    const target = event.target as HTMLElement | null;
    const typing = target?.matches("input, textarea, [contenteditable='true']");
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      void goto("#/search");
      menuOpen = false;
    } else if (event.key === "Escape") {
      menuOpen = false;
      nowPlayingOpen = false;
      profilesOpen = false;
      deleteProfileId = null;
      downloads.close();
    } else if (event.code === "Space" && !typing) {
      event.preventDefault();
      void player.toggle();
    }
  }
</script>

<svelte:window onkeydown={keyboard} onbeforeunload={() => player.shutdown()} />

<div class="app-shell" class:has-player={player.visibleCurrent}>
  <header class="top-bar">
    <div class="top-navigation">
      <button class="icon-button top-action" type="button" aria-label="Open menu" aria-expanded={menuOpen} onclick={() => menuOpen = !menuOpen}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="4" width="6" height="6" rx="1" /><rect x="14" y="4" width="6" height="6" rx="1" /><rect x="4" y="14" width="6" height="6" rx="1" /><rect x="14" y="14" width="6" height="6" rx="1" /></svg>
      </button>
      <a class="icon-button top-action home-mark" href="#/" aria-label="Home">
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m3 11 9-8 9 8" /><path d="M5 10v10h14V10M9 20v-6h6v6" /></svg>
      </a>
    </div>
    <div class="top-actions">
      <button class="profile-button desktop-profile-button" type="button" aria-label={`Profiles. Current profile ${profiles.active?.name ?? "Main"}`} onclick={openProfiles}>
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8" r="3" /><path d="M5 20c.6-4 3-6 7-6s6.4 2 7 6" /></svg><span>{profiles.active?.name ?? "Main"}</span>
      </button>
      <a class="icon-button top-action" href="#/search" aria-label="Search music">
        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m16 16 4 4" /></svg>
      </a>
    </div>
  </header>

  {#if menuOpen}
    <button class="menu-scrim" type="button" aria-label="Close menu" onclick={() => menuOpen = false}></button>
    <nav class="more-menu" aria-label="More navigation">
      <button class="menu-toggle" type="button" aria-pressed={library.discoveryEnabled} disabled={library.changingDiscovery} onclick={() => library.setDiscovery(!library.discoveryEnabled)}>
        <span><strong>Discovery</strong><small>Allows YouTube Music search and recommendations</small></span>
        <span class="switch" class:active={library.discoveryEnabled} aria-hidden="true"><i></i></span>
      </button>
      <div class="menu-divider"></div>
      <button class="menu-toggle mobile-profile-entry" type="button" onclick={openProfiles}>
        <span><strong>Profile</strong><small>{profiles.active?.name ?? "Main"}</small></span><span aria-hidden="true">›</span>
      </button>
      <a href="#/library" onclick={() => menuOpen = false}>Library</a>
      <a href="#/history" onclick={() => menuOpen = false}>History</a>
      <div class="menu-divider"></div>
      <a href="#/settings" onclick={() => menuOpen = false}>Settings</a>
      <a href="#/about" onclick={() => menuOpen = false}>About</a>
    </nav>
  {/if}

  <main class="content">{@render children()}</main>

  {#if player.visibleCurrent}
    <section class="mini-player" aria-label="Mini Player">
      <button class="mini-player-open" type="button" aria-label={`Open Now Playing for ${player.visibleCurrent.title}`} onclick={() => nowPlayingOpen = true}></button>
      <div class="mini-current">
        <button class="mini-open" type="button" onclick={() => nowPlayingOpen = true} aria-label={`Open Now Playing for ${player.visibleCurrent.title}`}>
          {#if library.artwork(player.visibleCurrent)}<img src={library.artwork(player.visibleCurrent) ?? ""} alt="" />{:else}<span class="mini-fallback" aria-hidden="true">♫</span>{/if}
        </button>
        <span><button class="mini-title" type="button" onclick={() => nowPlayingOpen = true}>{player.visibleCurrent.title}</button><button class="artist-link" type="button" onclick={() => openSongArtist(player.visibleCurrent!)}>{player.visibleCurrent.artistName}</button></span>
      </div>
      <div class="mini-progress" style={`--progress: ${player.visibleDurationMs ? (player.visiblePositionMs / player.visibleDurationMs) * 100 : 0}%; --buffered: ${player.visibleDurationMs && !player.loading ? Math.min(100, (player.bufferedMs / player.visibleDurationMs) * 100) : 0}%`}></div>
      <div class="mini-controls">
        <SongActionsMenu song={player.visibleCurrent} compact />
        <button class="icon-button desktop-only" type="button" aria-label="Previous song" onclick={() => player.previous()}>←</button>
        <button class="icon-button primary-control" type="button" aria-label={player.playing ? "Pause" : "Play"} disabled={player.loading} onclick={() => player.toggle()}>
          {#if player.loading}<span aria-hidden="true">…</span>{:else if player.playing}<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5v14M16 5v14" /></svg>{:else}<svg viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7V5Z" /></svg>{/if}
        </button>
        <button class="icon-button desktop-only" type="button" aria-label="Next song" onclick={() => player.next()}>→</button>
      </div>
    </section>
  {/if}

  {#if player.error}<p class="player-error" role="status">{player.error}</p>{/if}
  {#if profileFeedback}<p class="profile-toast" role="status">{profileFeedback}</p>{/if}
</div>

{#if profilesOpen}
  <div class="modal-layer profile-layer" role="presentation">
    <button class="modal-scrim" type="button" aria-label="Close profiles" disabled={profiles.changing} onclick={() => profilesOpen = false}></button>
    <div class="profile-dialog" role="dialog" aria-modal="true" aria-labelledby="profiles-title" tabindex="-1">
      <header><div><h2 id="profiles-title">{profileMode === "list" ? "Profiles" : profileMode === "create" ? "New Profile" : "Rename Profile"}</h2>{#if profileMode === "list"}<p>Independent listening identities</p>{/if}</div><button class="icon-button" type="button" aria-label="Close profiles" disabled={profiles.changing} onclick={() => profilesOpen = false}>×</button></header>
      {#if profileMode === "list"}
        <div class="profile-list">
          {#each profiles.items as profile (profile.id)}
            <div class="profile-row" class:active={profile.id === profiles.active?.id}>
              <button class="profile-select" type="button" disabled={profiles.changing} onclick={() => switchProfile(profile.id)}><span aria-hidden="true">{profile.id === profiles.active?.id ? "●" : "○"}</span><strong>{profile.name}</strong></button>
              <button class="profile-row-action" type="button" aria-label={`Rename ${profile.name}`} disabled={profiles.changing} onclick={() => beginRename(profile.id, profile.name)}>Rename</button>
              <button class="profile-row-action danger" type="button" aria-label={`Delete ${profile.name}`} disabled={profiles.changing || profiles.items.length === 1} onclick={() => deleteProfileId = profile.id}>Delete</button>
            </div>
          {/each}
        </div>
        {#if profiles.error}<p class="download-error" role="alert">{profiles.error}</p>{/if}
        <button class="add-profile-button" type="button" disabled={profiles.changing} onclick={() => { profileMode = "create"; profileName = ""; }}>+ Add Profile</button>
      {:else}
        <form class="profile-form" onsubmit={(event) => { event.preventDefault(); void saveProfile(); }}>
          <label for="profile-name">Name</label>
          <input id="profile-name" maxlength="40" autocomplete="off" bind:value={profileName} placeholder="Late Night" onkeydown={(event) => { if (event.key === "Enter" && !event.isComposing) event.currentTarget.blur(); }} />
          {#if profiles.error}<p class="download-error" role="alert">{profiles.error}</p>{/if}
          <footer><button class="text-button" type="button" disabled={profiles.changing} onclick={() => profileMode = "list"}>Cancel</button><button class="primary-action" type="submit" disabled={profiles.changing || !profileName.trim()}>{profiles.changing ? "Saving…" : profileMode === "create" ? "Create" : "Save"}</button></footer>
        </form>
      {/if}
    </div>
  </div>
{/if}

{#if deleteProfileId}
  <div class="modal-layer profile-delete-layer" role="presentation">
    <button class="modal-scrim" type="button" aria-label="Cancel profile deletion" disabled={profiles.changing} onclick={() => deleteProfileId = null}></button>
    <div class="profile-dialog delete-confirmation" role="alertdialog" aria-modal="true" aria-labelledby="delete-profile-title" tabindex="-1">
      <h2 id="delete-profile-title">Delete “{profiles.items.find((item) => item.id === deleteProfileId)?.name}”?</h2>
      <p>This removes this profile’s listening history, likes, recommendations, and tuning signals. Shared music and settings will not be removed.</p>
      <footer><button class="text-button" type="button" disabled={profiles.changing} onclick={() => deleteProfileId = null}>Cancel</button><button class="primary-action danger-action" type="button" disabled={profiles.changing} onclick={confirmDeleteProfile}>{profiles.changing ? "Deleting…" : "Delete"}</button></footer>
    </div>
  </div>
{/if}

{#if nowPlayingOpen && player.visibleCurrent}
  <section class="now-playing-view" aria-label="Now Playing" ontouchstart={nowPlayingTouchStart} ontouchend={nowPlayingTouchEnd} transition:fly={{ y: motion.enabled ? 48 : 0, duration: motion.enabled ? 220 : 0 }}>
    <header><button class="icon-button" type="button" aria-label="Close Now Playing" onclick={() => nowPlayingOpen = false}>←</button><span>Now Playing</span></header>
    <div class="now-playing-layout">
      <div class="player-detail">
        <div class="artwork-stage">
          {#if lyricsOpen}
            <section class="lyrics-panel artwork-lyrics" class:timed={timedLyrics.length > 0} aria-label="Lyrics" bind:this={lyricsPanel} onscroll={lyricsScrolled}>
              {#if lyricsLoading}<div class="lyrics-loading" role="status"><div class="skeleton"></div><div class="skeleton"></div><div class="skeleton"></div></div>
              {:else if lyricsError}<p class="lyrics-error" role="alert">{lyricsError} <button class="text-button" type="button" onclick={() => player.visibleCurrent && loadLyrics(player.visibleCurrent.id)}>Retry</button></p>
              {:else if lyricsLoaded && lyrics && timedLyrics.length}
                <div class="timed-lyrics">{#each timedLyrics as line, index}<button type="button" data-lyric-index={index} class:active={index === activeLyricIndex} class:past={index < activeLyricIndex} onclick={() => { player.seek(line.timeMs); setLyricsFollowing(true); }}>{line.text}</button>{/each}</div>
                {#if !lyricsFollowing}<button class="resume-lyrics" type="button" onclick={resumeLyrics}>Return to current lyric</button>{/if}
                <small>{lyrics.source === "embedded" ? "Synced lyrics embedded in local file" : lyrics.attribution ?? "YouTube Music"}</small>
              {:else if lyricsLoaded && lyrics}<pre>{lyrics.text}</pre><small>{lyrics.source === "embedded" ? "Embedded in local file" : lyrics.attribution ?? "YouTube Music"}</small>
              {:else if lyricsLoaded}<p>No lyrics available.</p>{/if}
            </section>
          {:else if library.artwork(player.visibleCurrent)}<img class="large-art" src={library.artwork(player.visibleCurrent) ?? ""} alt="" />
          {:else}<div class="large-art fallback" aria-hidden="true">♫</div>{/if}
        </div>
        <div class="track-heading"><div><h1>{player.visibleCurrent.title}</h1><button class="artist-link" type="button" onclick={() => openSongArtist(player.visibleCurrent!)}>{player.visibleCurrent.artistName}</button></div><SongActionsMenu song={player.visibleCurrent} /></div>
        <div class="full-progress" style={`--progress: ${player.visibleDurationMs ? Math.min(100, (player.visiblePositionMs / player.visibleDurationMs) * 100) : 0}%; --buffered: ${player.visibleDurationMs && !player.loading ? Math.min(100, (player.bufferedMs / player.visibleDurationMs) * 100) : 0}%`}><input aria-label="Playback position" type="range" min="0" max={player.visibleDurationMs || 1} value={player.visiblePositionMs} disabled={player.loading} oninput={(event) => player.seek(Number(event.currentTarget.value))} /><div><span>{time(player.visiblePositionMs)}</span><span>{time(player.visibleDurationMs)}</span></div></div>
        <div class="full-transport"><button class="icon-button repeat-button" class:active={player.repeatOne} type="button" aria-label={player.repeatOne ? "Turn off repeat song" : "Repeat this song"} aria-pressed={player.repeatOne} onclick={() => player.toggleRepeatOne()}>↻<small>1</small></button><button class="icon-button mobile-swipe-hidden" type="button" aria-label="Previous song" onclick={() => player.previous()}>←</button><button class="icon-button primary-control large" type="button" aria-label={player.playing ? "Pause" : "Play"} onclick={() => player.toggle()}>{player.playing ? "Ⅱ" : "▶"}</button><button class="icon-button mobile-swipe-hidden" type="button" aria-label="Next song" onclick={() => player.next()}>→</button></div>
        <button class="lyrics-toggle" class:active={lyricsOpen} type="button" aria-expanded={lyricsOpen} onclick={toggleLyrics}>LYRICS</button>
        <p class="mobile-gesture-hint">Swipe sideways to change track · swipe down to close</p>
      </div>
      <aside class="queue-panel" aria-labelledby="next-up-title"><h2 id="next-up-title">Queue</h2>{#if player.queue.length > 1}<ol bind:this={queueList}>{#each player.queue as song, index (song.id)}<li class:current={index === player.visibleCurrentIndex} class:played={index < (player.visibleCurrentIndex ?? 0)} aria-current={index === player.visibleCurrentIndex ? "true" : undefined}><button class="queue-play" type="button" onclick={() => player.playQueuedSong(song)} aria-label={`Play ${song.title}`}>{#if library.artwork(song)}<img src={library.artwork(song) ?? ""} alt="" loading="lazy" />{:else}<span class="queue-fallback" aria-hidden="true">♫</span>{/if}</button><span class="queue-track"><button class="queue-title" type="button" onclick={() => player.playQueuedSong(song)}>{song.title}</button><button class="artist-link" type="button" onclick={() => openSongArtist(song)}>{song.artistName}</button></span><SongActionsMenu {song} compact /></li>{/each}</ol>{:else}<p>Related songs are loading…</p>{/if}</aside>
    </div>
  </section>
{/if}

{#if downloads.open && downloads.song}
  <div class="modal-layer" role="presentation">
    <button class="modal-scrim" type="button" aria-label="Close download dialog" disabled={downloads.loading} onclick={() => downloads.close()}></button>
    <div class="download-dialog" role="dialog" aria-modal="true" aria-labelledby="download-title">
      <header>
        <div><h2 id="download-title">Download song</h2><p>{downloads.song.title} · {downloads.song.artistName}</p></div>
        <button class="icon-button" type="button" aria-label="Close download dialog" disabled={downloads.loading} onclick={() => downloads.close()}>×</button>
      </header>

      {#if downloads.isAndroid}
        <fieldset class="download-destinations">
          <legend>Android folder</legend>
          {#each downloads.androidDirectories as directory (directory.uri)}
            <label><input type="radio" name="android-download-directory" value={directory.uri} bind:group={downloads.selectedAndroidUri} disabled={downloads.loading} /><span><strong>{directory.displayName}</strong><small>Access retained by Android</small></span></label>
          {/each}
          {#if downloads.androidDirectories.length === 0}<p>No download folder selected yet.</p>{/if}
        </fieldset>
        <button class="primary-action choose-folder" type="button" disabled={downloads.loading} onclick={() => downloads.addAndroidDirectory()}>{downloads.androidDirectories.length ? "Choose another Android folder…" : "Choose Android folder…"}</button>
      {:else}
        <fieldset class="download-destinations">
          <legend>Music folder</legend>
          {#each downloads.directories as directory (directory.id)}
            <label><input type="radio" name="desktop-download-directory" value={directory.id} bind:group={downloads.selectedDirectoryId} disabled={downloads.loading} /><span><strong>{directory.path}</strong><small>{directory.trackCount} indexed songs</small></span></label>
          {/each}
          {#if downloads.directories.length === 0}<p>No local music folders are configured.</p>{/if}
        </fieldset>
        {#if downloads.directories.length === 0}<a class="primary-action choose-folder" href="#/settings" onclick={() => downloads.close()}>Add a music folder in Settings</a>{/if}
      {/if}

      {#if downloads.message}<p class="download-status" role="status">{downloads.message}</p>{/if}
      {#if downloads.error}<p class="download-error" role="alert">{downloads.error}</p>{/if}
      <footer>
        <button class="text-button" type="button" disabled={downloads.loading} onclick={() => downloads.close()}>Cancel</button>
        <button class="primary-action" type="button" disabled={downloads.loading || (downloads.isAndroid ? !downloads.selectedAndroidUri : downloads.selectedDirectoryId === null)} onclick={() => downloads.confirm()}>{downloads.loading ? "Downloading…" : "Download"}</button>
      </footer>
    </div>
  </div>
{/if}
