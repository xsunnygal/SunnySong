<script lang="ts">
  import { onMount } from "svelte";
  import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
  import {
    addMusicDirectory,
    beginJellyfinQuickConnect,

    connectJellyfinPassword,
    exportBackup,
    finishJellyfinQuickConnect,
    getJellyfinServers,
    getLocalArtists,
    getMusicDirectories,

    refreshJellyfinLibraries,
    removeJellyfinServer,
    removeMusicDirectory,
    rescanMusicLibrary,
    setJellyfinLibraryEnabled,
    setLocalArtistEnabled,

    stageBackupRestore,
    validateJellyfinServer,
    type AudioQuality,
    type JellyfinPublicServerInfo,
    type JellyfinQuickConnectSession,
    type JellyfinServer,
    type LocalArtist,
    type MusicDirectory,

  } from "$lib/api/backend";
  import { library } from "$lib/features/library/library.svelte";
  import { motion } from "$lib/features/motion/motion.svelte";
  import { player } from "$lib/features/player/player.svelte";
  import { revisions } from "$lib/features/revisions.svelte";
  import { theme, type ThemePreference } from "$lib/features/theme/theme.svelte";

  const options: { value: ThemePreference; label: string }[] = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
  ];
  let directories = $state<MusicDirectory[]>([]);
  let artists = $state<LocalArtist[]>([]);
  let jellyfinServers = $state<JellyfinServer[]>([]);
  let jellyfinAddress = $state("");
  let jellyfinUsername = $state("");
  let jellyfinPassword = $state("");
  let jellyfinCandidate = $state<JellyfinPublicServerInfo | null>(null);
  let quickConnect = $state<JellyfinQuickConnectSession | null>(null);
  type JellyfinAction = "connecting" | "signing-in" | "starting-quick-connect" | "checking-authorization" | "refreshing";

  let addingJellyfin = $state(false);
  let jellyfinBusy = $state(false);
  let jellyfinAction = $state<JellyfinAction | null>(null);
  let refreshingJellyfinServerId = $state<number | null>(null);
  let jellyfinDiagnostics = $state<string[]>([]);
  let jellyfinServerMessages = $state<Record<number, string>>({});
  let artistQuery = $state("");
  let artistOffset = $state(0);
  let artistHasMore = $state(false);
  let busy = $state(false);
  let message = $state("");
  let searchTimer: ReturnType<typeof setTimeout>;
  let filesystemBackupSupported = $state(false);
  let backupBusy = $state(false);
  let backupMessage = $state("");
  const SHOW_DIAGNOSTICS = false;

  async function changeAudioQuality(quality: AudioQuality) {
    try {
      await library.setAudioQuality(quality);
      message = `Audio quality set to ${quality}. New streams will use this setting.`;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    }
  }

  async function refreshDirectories() {
    directories = await getMusicDirectories();
  }

  async function refreshJellyfinServers() {
    jellyfinServers = await getJellyfinServers();
  }

  function upsertJellyfinServer(server: JellyfinServer) {
    const exists = jellyfinServers.some((item) => item.id === server.id);
    jellyfinServers = exists
      ? jellyfinServers.map((item) => item.id === server.id ? server : item)
      : [...jellyfinServers, server];
  }

  function resetJellyfinForm() {
    addingJellyfin = false;
    jellyfinCandidate = null;
    quickConnect = null;
    jellyfinPassword = "";
  }

  async function checkJellyfinServer() {
    jellyfinBusy = true;
    jellyfinAction = "connecting";
    message = "Checking Jellyfin server…";
    try {
      jellyfinCandidate = await validateJellyfinServer(jellyfinAddress);
      jellyfinAddress = jellyfinCandidate.normalizedUrl;
      message = `Connected to ${jellyfinCandidate.name} ${jellyfinCandidate.version}. Sign in to continue.`;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      jellyfinBusy = false;
      jellyfinAction = null;
    }
  }

  async function signInJellyfin() {
    jellyfinBusy = true;
    jellyfinAction = "signing-in";
    message = "Signing in to Jellyfin…";
    try {
      const connected = await connectJellyfinPassword(jellyfinAddress, jellyfinUsername, jellyfinPassword);
      upsertJellyfinServer(connected);
      resetJellyfinForm();
      message = "Jellyfin server connected. Choose the music libraries to include.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      jellyfinBusy = false;
      jellyfinAction = null;
      jellyfinPassword = "";
    }
  }

  async function startQuickConnect() {
    jellyfinBusy = true;
    jellyfinAction = "starting-quick-connect";
    message = "Starting Jellyfin Quick Connect…";
    try {
      quickConnect = await beginJellyfinQuickConnect(jellyfinAddress);
      message = "Authorize this code in another Jellyfin client, then continue here.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      jellyfinBusy = false;
      jellyfinAction = null;
    }
  }

  async function completeQuickConnect() {
    if (!quickConnect) return;
    jellyfinBusy = true;
    jellyfinAction = "checking-authorization";
    message = "Checking Quick Connect authorization…";
    try {
      const connected = await finishJellyfinQuickConnect(jellyfinAddress, quickConnect.secret);
      if (!connected) {
        message = "Quick Connect has not been authorized yet.";
        return;
      }
      upsertJellyfinServer(connected);
      resetJellyfinForm();
      message = "Jellyfin server connected. Choose the music libraries to include.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      jellyfinBusy = false;
      jellyfinAction = null;
    }
  }

  async function toggleJellyfinLibrary(server: JellyfinServer, libraryId: string, enabled: boolean) {
    jellyfinBusy = true;
    const previous = server.libraries.find((item) => item.id === libraryId)?.enabled ?? !enabled;
    jellyfinServers = jellyfinServers.map((item) => item.id === server.id ? {
      ...item,
      libraries: item.libraries.map((libraryItem) => libraryItem.id === libraryId ? { ...libraryItem, enabled } : libraryItem),
    } : item);
    jellyfinServerMessages = { ...jellyfinServerMessages, [server.id]: enabled ? "Syncing this music library…" : "Removing this library from the active Library view…" };
    try {
      await setJellyfinLibraryEnabled(server.id, libraryId, enabled);
      library.version += 1;
      revisions.libraryChanged();
      jellyfinServerMessages = { ...jellyfinServerMessages, [server.id]: enabled ? "Music library synced and ready to play." : "Music library hidden. Cached metadata remains lightweight." };
    } catch (error) {
      jellyfinServers = jellyfinServers.map((item) => item.id === server.id ? {
        ...item,
        libraries: item.libraries.map((libraryItem) => libraryItem.id === libraryId ? { ...libraryItem, enabled: previous } : libraryItem),
      } : item);
      jellyfinServerMessages = { ...jellyfinServerMessages, [server.id]: error instanceof Error ? error.message : String(error) };
    } finally {
      jellyfinBusy = false;
    }
  }

  async function refreshJellyfin(server: JellyfinServer) {
    jellyfinBusy = true;
    jellyfinAction = "refreshing";
    refreshingJellyfinServerId = server.id;
    jellyfinServerMessages = { ...jellyfinServerMessages, [server.id]: "Checking every library and subfolder for audio…" };
    try {
      const result = await refreshJellyfinLibraries(server.id);
      jellyfinDiagnostics = result.diagnostics;
      upsertJellyfinServer(result.server);
      library.version += 1;
      revisions.libraryChanged();
      jellyfinServerMessages = { ...jellyfinServerMessages, [server.id]: `Found ${result.server.libraries.length} audio ${result.server.libraries.length === 1 ? "library" : "libraries"}; enabled libraries were synchronized.` };
    } catch (error) {
      await refreshJellyfinServers();
      jellyfinServerMessages = { ...jellyfinServerMessages, [server.id]: error instanceof Error ? error.message : String(error) };
    } finally {
      jellyfinBusy = false;
      jellyfinAction = null;
      refreshingJellyfinServerId = null;
    }
  }

  async function disconnectJellyfin(server: JellyfinServer) {
    if (jellyfinBusy || !confirm(`Disconnect ${server.name}? This only removes its sources from SunnySong and never deletes anything from Jellyfin.`)) return;
    jellyfinBusy = true;
    message = "";
    try {
      await removeJellyfinServer(server.id);
      jellyfinServers = jellyfinServers.filter((item) => item.id !== server.id);
      library.version += 1;
      revisions.libraryChanged();
      message = "Jellyfin server disconnected. No server files were changed.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      jellyfinBusy = false;
    }
  }

  async function loadArtists(reset = false) {
    const offset = reset ? 0 : artistOffset;
    const next = await getLocalArtists(artistQuery.trim(), 50, offset);
    artists = reset ? next : [...artists, ...next];
    artistOffset = offset + next.length;
    artistHasMore = next.length === 50;
  }

  function artistSearchChanged() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => void loadArtists(true), 180);
  }

  function submitOnEnter(event: KeyboardEvent, action: () => void) {
    if (event.key !== "Enter" || event.isComposing) return;
    event.preventDefault();
    (event.currentTarget as HTMLInputElement).blur();
    action();
  }

  function submitArtistSearch(event: KeyboardEvent) {
    if (event.key !== "Enter" || event.isComposing) return;
    event.preventDefault();
    clearTimeout(searchTimer);
    (event.currentTarget as HTMLInputElement).blur();
    void loadArtists(true);
  }

  async function addFolders() {
    const selected = await openDialog({ directory: true, multiple: true, title: "Add Music Folders" });
    const paths = selected ? (Array.isArray(selected) ? selected : [selected]) : [];
    if (!paths.length) return;
    busy = true;
    message = "Scanning selected music folders…";
    try {
      for (const path of paths) await addMusicDirectory(path);
      await Promise.all([refreshDirectories(), loadArtists(true)]);
      library.version += 1;
      revisions.libraryChanged();
      message = "Music folders added and scanned.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
      await refreshDirectories();
    } finally {
      busy = false;
    }
  }

  async function removeFolder(directory: MusicDirectory) {
    if (busy || !confirm(`Remove ${directory.path} from the library? Files on disk will not be deleted.`)) return;
    busy = true;
    message = "";
    try {
      await removeMusicDirectory(directory.id);
      await Promise.all([refreshDirectories(), loadArtists(true)]);
      library.version += 1;
      revisions.libraryChanged();
      message = "Folder removed from the active library. Files were not deleted.";
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      busy = false;
    }
  }

  async function rescan() {
    busy = true;
    message = "Rescanning local music…";
    try {
      const reports = await rescanMusicLibrary();
      await Promise.all([refreshDirectories(), loadArtists(true)]);
      library.version += 1;
      revisions.libraryChanged();
      const indexed = reports.reduce((sum, report) => sum + report.indexedTracks, 0);
      const issues = reports.filter((report) => report.status !== "READY").length;
      message = issues
        ? `Rescan finished: ${indexed.toLocaleString()} tracks indexed; ${issues} folder${issues === 1 ? "" : "s"} need attention.`
        : `Rescan complete: ${indexed.toLocaleString()} tracks indexed.`;
    } catch (error) {
      message = error instanceof Error ? error.message : String(error);
    } finally {
      busy = false;
    }
  }

  async function exportDataBackup() {
    if (backupBusy) return;
    const date = new Date().toISOString().slice(0, 10);
    const path = await saveDialog({
      title: "Export SunnySong Backup",
      defaultPath: `SunnySong-${date}.sqlite3`,
      filters: [{ name: "SunnySong SQLite backup", extensions: ["sqlite3"] }],
    });
    if (!path) return;
    backupBusy = true;
    backupMessage = "Creating a consistent database snapshot…";
    try {
      const status = await exportBackup(path);
      backupMessage = status.message;
    } catch (error) {
      backupMessage = error instanceof Error ? error.message : String(error);
    } finally {
      backupBusy = false;
    }
  }

  async function restoreDataBackup() {
    if (backupBusy) return;
    const selected = await openDialog({
      title: "Restore SunnySong Backup",
      multiple: false,
      filters: [{ name: "SunnySong SQLite backup", extensions: ["sqlite3"] }],
    });
    const path = Array.isArray(selected) ? selected[0] : selected;
    if (!path || !confirm("Validate and stage this backup? It will replace local library data, playlists, profiles, and history after you restart SunnySong. Credentials and cookies are kept separately.")) return;
    backupBusy = true;
    backupMessage = "Validating and staging restore…";
    try {
      const status = await stageBackupRestore(path);
      backupMessage = status.restartRequired ? `${status.message} Restart required.` : status.message;
    } catch (error) {
      backupMessage = error instanceof Error ? error.message : String(error);
    } finally {
      backupBusy = false;
    }
  }

  async function toggleArtist(artist: LocalArtist) {
    const previous = artist.enabled;
    artist.enabled = !previous;
    artists = [...artists];
    try {
      await setLocalArtistEnabled(artist.id, artist.enabled);
      library.version += 1;
      revisions.libraryChanged();
    } catch (error) {
      artist.enabled = previous;
      artists = [...artists];
      message = error instanceof Error ? error.message : String(error);
    }
  }

  onMount(() => {
    filesystemBackupSupported = !/Android|iPhone|iPad/i.test(navigator.userAgent);
    void library.initialize();
    void Promise.all([
      refreshDirectories(),
      refreshJellyfinServers(),
      loadArtists(true),
    ]).catch((error) => {
      message = error instanceof Error ? error.message : String(error);
    });
  });
</script>

<svelte:head><title>Settings · SunnySong</title></svelte:head>
<header class="simple-page-header"><a class="icon-button" href="#/" aria-label="Back to Home">←</a><h1>Settings</h1></header>
<div class="settings-stack">
  <section class="settings-group library-settings" aria-labelledby="local-library-title">
    <div class="settings-copy"><h2 id="local-library-title">Local Library</h2></div>
    <div class="library-settings-content">
      <div class="settings-subsection">
        <div class="settings-subheading"><h3>Music Folders</h3><button class="text-button" type="button" onclick={addFolders} disabled={busy}>+ Add Folder</button></div>
        {#if directories.length}
          <ul class="directory-list">
            {#each directories as directory (directory.id)}
              <li><span><strong>{directory.path}</strong><small>{directory.status === "READY" ? `Ready — ${directory.trackCount.toLocaleString()} tracks` : directory.status === "SCANNING" ? "Scanning…" : directory.status === "UNAVAILABLE" ? "Unavailable — indexed tracks preserved" : `Scan error — ${directory.lastError ?? "some files could not be read"}`}</small>{#if directory.lastScannedAtMs}<small>Last successful scan {new Date(directory.lastScannedAtMs).toLocaleString()}</small>{/if}</span><button class="text-button danger" type="button" disabled={busy} onclick={() => removeFolder(directory)}>Remove</button></li>
            {/each}
          </ul>
        {:else}<p class="inline-message">Add a folder to build your local library.</p>{/if}
        <button class="secondary-action" type="button" onclick={rescan} disabled={busy || !directories.length}>{busy ? "Scanning…" : "Rescan Library"}</button>
      </div>

      <div class="settings-subsection">
        <div class="settings-subheading"><h3>Jellyfin Servers</h3><button class="text-button" type="button" onclick={() => addingJellyfin = !addingJellyfin} disabled={jellyfinBusy}>+ Add Jellyfin Server</button></div>
        {#if addingJellyfin}
          <div class="jellyfin-connect-panel">
            <label><span>Server address</span><input type="url" placeholder="https://jellyfin.example.com" bind:value={jellyfinAddress} disabled={jellyfinBusy || !!jellyfinCandidate} onkeydown={(event) => submitOnEnter(event, () => { if (!jellyfinBusy && jellyfinAddress.trim()) void checkJellyfinServer(); })} /></label>
            {#if !jellyfinCandidate}
              <button class="secondary-action" type="button" onclick={checkJellyfinServer} disabled={jellyfinBusy || !jellyfinAddress.trim()}>{jellyfinAction === "connecting" ? "Connecting…" : "Continue"}</button>
            {:else if quickConnect}
              <div class="quick-connect-code"><small>Quick Connect code</small><strong>{quickConnect.code}</strong><p>Enter this code from Quick Connect in another Jellyfin client.</p></div>
              <div class="inline-actions"><button class="primary-action" type="button" onclick={completeQuickConnect} disabled={jellyfinBusy}>{jellyfinAction === "checking-authorization" ? "Checking…" : "I've authorized it"}</button><button class="text-button" type="button" onclick={() => quickConnect = null} disabled={jellyfinBusy}>Use password</button></div>
            {:else}
              <p class="inline-message">{jellyfinCandidate.name} · Jellyfin {jellyfinCandidate.version}</p>
              <button class="secondary-action" type="button" onclick={startQuickConnect} disabled={jellyfinBusy}>{jellyfinAction === "starting-quick-connect" ? "Starting…" : "Use Quick Connect"}</button>
              <div class="auth-divider"><span>or sign in with password</span></div>
              <label><span>Username</span><input autocomplete="username" bind:value={jellyfinUsername} disabled={jellyfinBusy} onkeydown={(event) => submitOnEnter(event, () => { if (!jellyfinBusy && jellyfinUsername.trim()) void signInJellyfin(); })} /></label>
              <label><span>Password</span><input type="password" autocomplete="current-password" bind:value={jellyfinPassword} disabled={jellyfinBusy} onkeydown={(event) => submitOnEnter(event, () => { if (!jellyfinBusy && jellyfinUsername.trim()) void signInJellyfin(); })} /></label>
              <button class="primary-action" type="button" onclick={signInJellyfin} disabled={jellyfinBusy || !jellyfinUsername.trim()}>{jellyfinAction === "signing-in" ? "Signing In…" : "Sign In"}</button>
            {/if}
            <button class="text-button" type="button" onclick={resetJellyfinForm} disabled={jellyfinBusy}>Cancel</button>
          </div>
        {/if}
        {#if jellyfinDiagnostics.length}
          <details class="jellyfin-diagnostics" open>
            <summary>Jellyfin library contents</summary>
            <ul>{#each jellyfinDiagnostics as diagnostic}<li>{diagnostic}</li>{/each}</ul>
          </details>
        {/if}
        {#if jellyfinServers.length}
          <div class="jellyfin-server-list">
            {#each jellyfinServers as server (server.id)}
              <article class="jellyfin-server-card">
                <div class="settings-subheading"><span><strong>{server.name}</strong><small>{server.baseUrl}</small><small>{server.status === "CONNECTED" ? `Connected as ${server.username}` : server.status}</small></span><div class="inline-actions"><button class="text-button" type="button" onclick={() => refreshJellyfin(server)} disabled={jellyfinBusy}>{jellyfinAction === "refreshing" && refreshingJellyfinServerId === server.id ? "Refreshing…" : "Refresh"}</button><button class="text-button danger" type="button" onclick={() => disconnectJellyfin(server)} disabled={jellyfinBusy}>Disconnect</button></div></div>
                {#if jellyfinServerMessages[server.id]}<p class="inline-message jellyfin-server-message" role="status">{jellyfinServerMessages[server.id]}</p>{/if}
                {#if server.libraries.length}
                  <div class="jellyfin-library-list">
                    {#each server.libraries as jellyfinLibrary (jellyfinLibrary.id)}
                      <label><span><strong>{jellyfinLibrary.name}</strong><small>{jellyfinLibrary.trackCount.toLocaleString()} tracks</small></span><input type="checkbox" checked={jellyfinLibrary.enabled} disabled={jellyfinBusy} onchange={(event) => toggleJellyfinLibrary(server, jellyfinLibrary.id, event.currentTarget.checked)} /></label>
                    {/each}
                  </div>
                {:else}<p class="inline-message">No accessible music libraries were reported by this account.</p>{/if}
              </article>
            {/each}
          </div>
        {:else if !addingJellyfin}<p class="inline-message">Connect an optional Jellyfin server to use your self-hosted library independently of Discovery.</p>{/if}
      </div>

      <div class="settings-subsection">
        <div class="settings-subheading"><h3>Local Artists</h3><span>{artists.length}{artistHasMore ? "+" : ""}</span></div>
        <label class="artist-search"><span class="sr-only">Search local artists</span><input placeholder="Search artists…" bind:value={artistQuery} oninput={artistSearchChanged} onkeydown={submitArtistSearch} /></label>
        <div class="artist-filter-list">
          {#each artists as artist (artist.id)}
            <label><span><strong>{artist.name}</strong><small>{artist.trackCount} {artist.trackCount === 1 ? "track" : "tracks"}</small></span><input type="checkbox" checked={artist.enabled} onchange={() => toggleArtist(artist)} /></label>
          {/each}
        </div>
        {#if artistHasMore}<button class="text-button" type="button" onclick={() => loadArtists()}>Load more artists</button>{/if}
      </div>
      {#if message}<p class="inline-message" role="status">{message}</p>{/if}
    </div>
  </section>

  <section class="settings-group" aria-labelledby="discovery-title">
    <div><h2 id="discovery-title">Discovery</h2><p>Allows SunnySong to contact YouTube Music for online search and recommendations. Local playback and recommendations remain available when off.</p></div>
    <label class="toggle-setting"><span>{library.discoveryEnabled ? "On" : "Off"}</span><input type="checkbox" checked={library.discoveryEnabled} disabled={library.changingDiscovery} onchange={(event) => library.setDiscovery(event.currentTarget.checked)} /></label>
  </section>


  <section class="settings-group data-backup-settings" aria-labelledby="data-backup-title">
    <div><h2 id="data-backup-title">Backup & Restore</h2><p>Exports a consistent local database snapshot containing library indexes, playlists, profiles, and history. Operating-system credentials and YouTube cookies are never included.</p>{#if !filesystemBackupSupported}<p>Filesystem backup and restore are currently available in the desktop app only; Android content URIs are not yet supported for this feature.</p>{/if}</div>
    {#if filesystemBackupSupported}<div class="backup-controls"><div class="inline-actions"><button class="secondary-action" type="button" disabled={backupBusy} onclick={exportDataBackup}>{backupBusy ? "Working…" : "Export backup"}</button><button class="secondary-action" type="button" disabled={backupBusy} onclick={restoreDataBackup}>Restore backup…</button></div>{#if backupMessage}<p class="inline-message" role="status">{backupMessage}</p>{/if}</div>{/if}
  </section>

  <section class="settings-group" aria-labelledby="data-saver-title">
    <div><h2 id="data-saver-title">Data Saver</h2><p>Requests compact 144 × 144 artwork to reduce image bandwidth.</p></div>
    <label class="toggle-setting"><span>{library.dataSaverEnabled ? "On" : "Off"}</span><input type="checkbox" checked={library.dataSaverEnabled} onchange={(event) => library.setDataSaver(event.currentTarget.checked)} /></label>
  </section>

  <section class="settings-group" aria-labelledby="audio-quality-title">
    <div><h2 id="audio-quality-title">Audio quality</h2><p>Low targets up to 64 kbps, Medium targets standard ~128 kbps audio, and High selects the best available audio.</p></div>
    <div class="segmented-control audio-quality-control" aria-label="Audio quality">
      {#each [["low", "Low"], ["medium", "Medium"], ["high", "High"]] as option}
        <button type="button" class:active={library.audioQuality === option[0]} aria-pressed={library.audioQuality === option[0]} disabled={library.changingAudioQuality} onclick={() => changeAudioQuality(option[0] as AudioQuality)}>{option[1]}</button>
      {/each}
    </div>
  </section>

  <section class="settings-group" aria-labelledby="crossfade-title">
    <div><h2 id="crossfade-title">Crossfade</h2><p>Overlap the end of one song with the start of the next.</p></div>
    <div class="segmented-control playback-settings-control" aria-label="Crossfade duration">
      {#each [[0, "Off"], [2, "2 sec"], [5, "5 sec"], [10, "10 sec"]] as option}
        <button type="button" class:active={player.crossfadeDuration === option[0]} aria-pressed={player.crossfadeDuration === option[0]} onclick={() => player.setCrossfadeDuration(option[0] as 0 | 2 | 5 | 10)}>{option[1]}</button>
      {/each}
    </div>
  </section>

  <section class="settings-group" aria-labelledby="normalization-title">
    <div><h2 id="normalization-title">Volume normalization</h2><p>Reduce loudness differences between songs or albums. Normalization activates only where gain metadata exists; it never boosts volume.</p></div>
    <div class="segmented-control playback-settings-control normalization-control" aria-label="Volume normalization mode">
      {#each [["off", "Off"], ["track", "Track"], ["album", "Album"]] as option}
        <button type="button" class:active={player.normalizationMode === option[0]} aria-pressed={player.normalizationMode === option[0]} onclick={() => player.setNormalizationMode(option[0] as "off" | "track" | "album")}>{option[1]}</button>
      {/each}
    </div>
  </section>


  <section class="settings-group" aria-labelledby="motion-title">
    <div><h2 id="motion-title">Interface animations</h2><p>Use subtle movement when opening panels and changing views. Turn this off for an instant, simpler interface.</p></div>
    <label class="toggle-setting"><span>{motion.enabled ? "On" : "Off"}</span><input type="checkbox" checked={motion.enabled} onchange={(event) => motion.set(event.currentTarget.checked)} /></label>
  </section>

  <section class="settings-group desktop-theme-setting" aria-labelledby="theme-title">
    <div><h2 id="theme-title">Theme</h2><p>Use your desktop appearance or choose a fixed mode.</p></div>
    <div class="segmented-control">
      {#each options as option}<button type="button" class:active={theme.preference === option.value} aria-pressed={theme.preference === option.value} onclick={() => theme.set(option.value)}>{option.label}</button>{/each}
    </div>
  </section>
  {#if SHOW_DIAGNOSTICS}<section class="settings-group"><div><h2>Diagnostics</h2><p>Inspect database health, player state, timings, and recommendation scoring.</p></div><a class="primary-action" href="#/diagnostics">Open</a></section>{/if}
  <section class="settings-group"><div><h2>About</h2><p>Version and release history.</p></div><a class="primary-action" href="#/about">View changelog</a></section>
</div>
