# SunnySong

SunnySong is a local-first music player for Android and Linux. It combines music stored on the device or Jellyfin with optional YouTube Music discovery, while keeping listening history, profiles, likes, playlists, and recommendation signals local.

> SunnySong is an independent, unofficial client and is not affiliated with YouTube, Google, or Jellyfin.

## Features

- Local folders and Jellyfin libraries work without enabling online Discovery.
- Optional YouTube Music search, recommendations, lyrics, and playback.
- Profile-isolated history, likes, affinity, Recently Played, and Quick Picks.
- Persistent queue, repeat-one, resilient buffered playback, and Android media controls.
- Local playlists, downloads with metadata, album/artist browsing, and configurable recommendation diversity.
- Svelte 5 interface with a Rust/Tauri 2 backend and SQLite persistence.

## Development

Requirements:

- Node.js and npm
- Rust toolchain
- Tauri 2 system dependencies
- Java 17 plus Android SDK/NDK for Android builds
- WebKitGTK and GStreamer codecs for Linux

```bash
npm install
npm run check
cargo test --workspace
npm run tauri dev
```

Initialize the Android host once, then run or package it:

```bash
npm run tauri android init
npm run tauri android dev
npm run tauri android build
```

## Architecture

The frontend calls typed Tauri commands. Application behavior and recommendation logic live in platform-independent Rust crates; SQLite stores shared library metadata and profile-scoped listening state. Provider and playback implementations sit behind ports so local files, Jellyfin, and YouTube Music remain isolated.

See:

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- [`docs/ROADMAP.md`](docs/ROADMAP.md)

## Privacy

SunnySong stores normal app data locally. Imported YouTube cookies and Jellyfin credentials are placed in operating-system credential storage. Do not commit cookie exports, database files, logs, or generated artifacts.

## License

MIT
