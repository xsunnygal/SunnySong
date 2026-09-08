# SunnySong roadmap

SunnySong has moved beyond the original foundation milestones. This document separates functionality present in the current working tree from release work that still requires implementation or hands-on platform validation.

## Implemented in the current working tree

### Core architecture and playback

- Rust workspace boundaries, typed Tauri commands, SQLite migrations, and provider/playback ports are established.
- Local files, Jellyfin libraries, and optional YouTube Music search and playback share normalized song and collection models.
- Queue persistence, play next, queue editing, shuffle, repeat-one, seek, volume, buffered playback recovery, lyrics, and sleep timer controls are implemented.
- Playback and listening events feed profile-scoped history, likes, affinity, skips, completions, Recently Played, Next Up, and Quick Picks.

### Discovery and library

- Search includes local and online songs, artists, albums, playlists, suggestions, artist pages, and collection playback, with partial-result handling.
- Home provides bounded, paged Quick Picks and paged Recently Played results with configurable diversity, new-song, and rediscovery inputs.
- Discover provides profile-aware recommendation sections and continues to support local recommendations when online Discovery is disabled.
- Library browsing covers songs, albums, folders, Jellyfin sources, playlists, bulk song actions, and explicit user-driven pagination for large lists.
- Dedicated Liked Songs, History, Downloads, and listening Recap pages are implemented with profile-aware refreshes and bounded loading.
- Local playlists support create, rename, delete, reorder, bulk additions/removals, and desktop M3U/M3U8 import/export.
- Downloads track attempts and failures, support retry/removal, write metadata, and use Android Storage Access Framework destinations where applicable.
- Desktop backup export and validated, restart-gated restore staging are implemented; credentials and YouTube cookies remain outside backups.

### Platform integration present in the tree

- Android has a foreground playback service, MediaSession controls, notification metadata/artwork, and persisted browse snapshots.
- Linux has an MPRIS implementation and AppImage configuration for bundling the required GStreamer runtime.
- Responsive layouts, reduced-motion support, keyboard/accessibility labels, live loading/error announcements, explicit retries, and empty/partial-failure states have received an initial pass.

## Remaining platform and release work

### Platform completion

- Validate Android foreground/background playback, media controls, process recreation, audio focus, storage permissions, and downloaded-file playback on representative physical devices and supported Android versions.
- Validate packaged AppImage playback on clean Linux distributions, including bundled GStreamer codecs, local/Jellyfin/YouTube sources, MPRIS controls, and update/install behavior.
- Implement and validate Windows SMTC and macOS Now Playing integrations before describing those platforms as supported.
- Add Android-compatible backup import/export through content URIs if backup parity is required on mobile.
- Continue accessibility testing with keyboard-only navigation, screen readers, text scaling, contrast modes, and mobile touch targets.

### Reliability and performance

- Expand failure-injection coverage for offline transitions, provider timeouts, stale requests, interrupted downloads, database restore failures, and queue recovery.
- Profile large libraries and long histories on lower-end Android hardware; tune query, artwork, and list rendering costs where measurements justify it.
- Continue recommendation tuning from real listening data while keeping score inputs and exclusion reasons explainable.
- Define retention and cleanup behavior for download records, cached provider metadata, artwork, and diagnostic data.

### Release engineering

- Add reproducible CI builds for Linux, Windows, macOS, APK, and AAB, with version checks and artifact retention.
- Add signing, notarization, Android keystore handling, release-channel configuration, checksums, and rollback documentation.
- Run a manual release matrix for install, upgrade, migration, backup/restore, offline startup, playback, background controls, and uninstall/reinstall behavior.
- Document supported operating-system versions, codec limitations, Jellyfin compatibility, YouTube authentication caveats, and privacy expectations.

Automated Rust tests cover core application and persistence behavior, and frontend static checks are available through `npm run check`. There are currently no automated Android-device or packaged-AppImage playback tests; those scenarios remain explicit manual release gates.

## Optional sync — future

- Build an Axum service separately from the client and YouTube request path.
- Sync UUIDv7 listening summaries and explicit actions idempotently.
- Rebuild aggregates locally; never synchronize stream URLs, credentials, cookies, or proxied media.
