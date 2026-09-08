<script lang="ts">
  import { onMount } from "svelte";
  import { getAppStatus } from "$lib/api/backend";

  let version = $state("0.4.2");

  onMount(() => {
    void getAppStatus().then((status) => version = status.version).catch(() => {});
  });
</script>

<svelte:head><title>About · SunnySong</title><meta name="description" content="SunnySong version and changelog." /></svelte:head>

<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><div><h1>About SunnySong</h1><p>Version {version}</p></div></header>
<main class="about-page">
  <section class="settings-group">
    <div><h2>0.4.2</h2><p>Focused navigation and interface polish</p></div>
    <ul class="changelog-list">
      <li>Moved personalized Discover sections onto Home beneath Quick Picks and added wheel and trackpad paging.</li>
      <li>Simplified navigation, page headings, song metadata, and Settings by removing unused or redundant controls and descriptions.</li>
      <li>Improved recent-history navigation, volume and normalization controls, centered secondary page headers, and Android launcher icon spacing.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.4.1</h2><p>Audio reliability and cleaner recommendations</p></div>
    <ul class="changelog-list">
      <li>Fixed silent playback on Linux and Android by keeping proxied audio out of WebKit's cross-origin Web Audio graph.</li>
      <li>Crossfade, sleep fades, and ReplayGain attenuation now use reliable native media-element volume controls.</li>
      <li>Quick Picks and automatic Next Up queues now keep only the highest-scored upload when an artist has duplicate, remix, remaster, or equivalent song variants.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.4.0</h2><p>Discovery, library, and listening update</p></div>
    <ul class="changelog-list">
      <li>Added Discover, improved live search, artist and collection browsing, personalized Quick Picks, and smarter continuous queues.</li>
      <li>Added synced follow-along lyrics, per-song timing adjustment, Play Next, persistent queues, gapless preloading, crossfade, sleep timers, and ReplayGain/R128 normalization.</li>
      <li>Expanded the local-first Library with dependable cover art, incremental scans, Jellyfin, automatic collections, playlists, downloads, bulk actions, M3U import/export, profiles, likes, history, and Recap.</li>
      <li>Added Android media controls and Android Auto browsing, Linux MPRIS integration, keyboard shortcuts, desktop backup/restore, clearer offline states, and bundled Linux audio codecs.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.3.1</h2><p>SunnySong rename</p></div>
    <ul class="changelog-list">
      <li>Renamed the application and all user-facing branding from SolMusic to SunnySong while preserving existing installation data and settings.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.3.0</h2><p>Playlists and player experience update</p></div>
    <ul class="changelog-list">
      <li>Added profile-scoped local playlists with Library creation, browsing, and Add to Playlist actions.</li>
      <li>Song actions now live in compact overflow menus with Like, Download, and playlist choices.</li>
      <li>Lyrics now replace the cover inside a clean, bold scrolling artwork panel.</li>
      <li>Now Playing minimizes for navigation, its queue follows the current song, and the entire mini-player surface opens it.</li>
      <li>Improved seek restoration, buffering recovery, repeat stability, and two-song queue source warming.</li>
      <li>Profile switches no longer attribute the already-playing song to the newly selected profile.</li>
      <li>Added per-profile Quick Pick controls for diversity, new music, and rediscovery.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.2.3</h2><p>Artist artwork fix</p></div>
    <ul class="changelog-list">
      <li>Artist pages now display the full artist artwork returned by YouTube Music, including pages opened from the player.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.2.2</h2><p>Catalog browsing and Android startup fixes</p></div>
    <ul class="changelog-list">
      <li>Fixed packaged Android builds opening a development localhost URL instead of bundled app assets.</li>
      <li>Album and playlist cards now recognize additional YouTube Music artwork formats.</li>
      <li>Albums, singles, releases, and playlists on artist pages can now be opened to browse their songs.</li>
      <li>Opening an artist from Now Playing now closes the player overlay, and mixed searches show at most two artist matches.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.2.1</h2><p>Progressive recommendations and queue polish</p></div>
    <ul class="changelog-list">
      <li>Quick Picks now replace old pages progressively, show loading slots immediately, and favor three Discovery recommendations per four-song page.</li>
      <li>Added semantic recommendation deduplication and YouTube thumbnail fallbacks.</li>
      <li>Search now renders local results and each YouTube Music category as soon as that category finishes, including best-effort lyric-phrase searches.</li>
      <li>Queue growth now compensates exactly for skipped or completed songs while retaining previously played entries.</li>
      <li>Added timestamp-aware follow-along lyrics, repeat-one, and tighter player action spacing.</li>
      <li>Local folder artwork matching now works regardless of filename capitalization.</li>
    </ul>
  </section>
  <section class="settings-group">
    <div><h2>0.2.0</h2><p>Responsive playback and discovery update</p></div>
    <ul class="changelog-list">
      <li>Added independent local listening profiles and profile-isolated recommendations.</li>
      <li>Made song selection optimistic and cached warmed playback sources.</li>
      <li>Added complete four-item Quick Pick paging with immediate loading pages.</li>
      <li>Fixed Jellyfin artwork in recommendations, recent songs, and playback queues.</li>
      <li>Added stable, recency-aware queue growth while retaining previously played tracks.</li>
      <li>Added embedded and YouTube Music lyrics and clickable artist/channel pages.</li>
      <li>Added Android media controls, background playback, downloads, and resilient buffering.</li>
    </ul>
  </section>
  <section class="settings-group"><div><h2>0.1.0</h2><p>Initial local-first preview</p></div><p>Initial desktop and Android player with local library and YouTube Music discovery support.</p></section>
</main>
