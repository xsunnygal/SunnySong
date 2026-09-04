# SunnySong roadmap

This is the working sequence for the MVP. Each phase should leave the app runnable and keep unstable integrations behind ports.

## 0. Foundation — current

- Establish the Rust workspace and dependency boundaries.
- Establish feature-oriented Svelte modules and one typed Tauri API boundary.
- Validate desktop and Android builds in CI-friendly form.
- Record architectural decisions before adding difficult integrations.

## 1. Search and provider

- Define canonical song, artist, album, search, and playback-source models.
- Define the `MusicProvider` port in the application layer.
- Implement YouTube Music search and metadata behind a dedicated adapter.
- Add caching, timeouts, structured errors, and provider diagnostics.

Milestone: search for a song and display normalized results.

## 2. Playback foundation

- Define player commands/events and a single authoritative playback state.
- Implement a desktop audio adapter.
- Implement Android Media3 playback through a narrow Kotlin/Tauri bridge.
- Add play, pause, seek, volume, next/previous, audio focus, and media metadata.

Milestone: search → select → reliable foreground/background playback.

## 3. Persistence and listening signals

- Add versioned SQLite migrations and repository adapters.
- Store song cache, settings, track/artist aggregates, and bounded recent plays.
- Track sessions in memory and commit one summary when playback ends/interruption occurs.
- Add likes, dislikes, completion, early-skip, replay, and crash-safe checkpoints.

Milestone: behavior produces explainable, tested local aggregates.

## 4. Next Up

- Retrieve provider-related tracks when playback starts.
- Rank primarily by current-song relationship.
- Deduplicate, enforce session/artist diversity, and refill near queue end.
- Expose queue reasons in developer diagnostics.

Milestone: continuous listening with a coherent 10–20 item queue.

## 5. Quick Picks

- Generate a bounded pool from favorites, recent interests, related tracks, rediscovery, and exploration.
- Score track affinity, artist affinity, recency, completion, discovery, repetition, and skips.
- Save stable recommendation snapshots and score explanations.
- Tune from real listening without introducing opaque ML.

Milestone: recommendations become noticeably personal after roughly 20 meaningful plays.

## 6. Platform polish and releases

- Linux MPRIS, Windows SMTC, macOS Now Playing, and Android MediaSession/notification integration.
- Responsive desktop/mobile UX, accessibility, offline/error states, and performance passes.
- CI builds for Linux, Windows, macOS, APK, and AAB.

## 7. Optional sync

- Build an Axum service separately from the client and YouTube request path.
- Sync UUIDv7 listening summaries and explicit actions idempotently.
- Rebuild aggregates locally; never synchronize stream URLs or proxy media.
