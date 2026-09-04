use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use solmusic_application::{
    DatabaseDiagnostics, LibraryAlbum, LibraryFolder, LibraryScanResult, LibrarySource,
    LibraryTrack, ListeningProfile, LocalArtist, LocalPlaybackFile, MusicDirectory,
    MusicRepository, Playlist, PlaylistTrack, RecentSong, ScannedLocalTrack, StorageError,
};
use solmusic_domain::{ArtistRef, ListeningSummary, Song, SongId, TrackProfile};
use tracing::{debug, info};

const LATEST_SCHEMA_VERSION: usize = 8;
const MIGRATIONS: [&str; LATEST_SCHEMA_VERSION] = [
    r#"
    CREATE TABLE songs (
        song_id       TEXT PRIMARY KEY NOT NULL,
        title         TEXT NOT NULL,
        artist_id     TEXT,
        artist_name   TEXT NOT NULL,
        album_id      TEXT,
        album_name    TEXT,
        duration_ms   INTEGER,
        thumbnail_url TEXT
    );

    CREATE TABLE track_stats (
        song_id            TEXT PRIMARY KEY NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        play_count          INTEGER NOT NULL DEFAULT 0 CHECK (play_count >= 0),
        completed_count     INTEGER NOT NULL DEFAULT 0 CHECK (completed_count >= 0),
        early_skip_count    INTEGER NOT NULL DEFAULT 0 CHECK (early_skip_count >= 0),
        completion_ema      REAL NOT NULL DEFAULT 0.0,
        completion_samples  INTEGER NOT NULL DEFAULT 0 CHECK (completion_samples >= 0),
        affinity            REAL NOT NULL DEFAULT 0.0,
        liked               INTEGER NOT NULL DEFAULT 0 CHECK (liked IN (0, 1)),
        disliked            INTEGER NOT NULL DEFAULT 0 CHECK (disliked IN (0, 1)),
        last_played_at_ms    INTEGER
    );

    CREATE TABLE processed_playback_events (
        event_id TEXT PRIMARY KEY NOT NULL
    );

    CREATE TABLE recent_plays (
        sequence_id   INTEGER PRIMARY KEY AUTOINCREMENT,
        event_id      TEXT NOT NULL UNIQUE,
        song_id       TEXT NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        started_at_ms INTEGER NOT NULL,
        listened_ms   INTEGER NOT NULL,
        duration_ms   INTEGER,
        end_reason    TEXT NOT NULL
    );

    CREATE INDEX recent_plays_song_id_idx ON recent_plays(song_id);
    CREATE INDEX recent_plays_started_at_idx ON recent_plays(started_at_ms DESC);
"#,
    r#"
    CREATE TABLE schema_migrations (
        version       INTEGER PRIMARY KEY NOT NULL,
        name          TEXT NOT NULL,
        applied_at_ms INTEGER NOT NULL
    );

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (1, 'initial', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (2, 'hardening', CAST(strftime('%s', 'now') AS INTEGER) * 1000);

    CREATE INDEX track_stats_liked_idx
        ON track_stats(liked, last_played_at_ms DESC) WHERE liked = 1;
    CREATE INDEX songs_artist_id_idx ON songs(artist_id);

    CREATE TABLE player_state (
        singleton_id      INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
        state_json        TEXT NOT NULL,
        updated_at_ms     INTEGER NOT NULL
    );
"#,
    r#"
    CREATE TABLE music_directories (
        directory_id        INTEGER PRIMARY KEY AUTOINCREMENT,
        canonical_path      TEXT NOT NULL UNIQUE,
        added_at_ms         INTEGER NOT NULL,
        last_scanned_at_ms  INTEGER
    );

    CREATE TABLE local_files (
        local_file_id       INTEGER PRIMARY KEY AUTOINCREMENT,
        directory_id        INTEGER NOT NULL REFERENCES music_directories(directory_id) ON DELETE CASCADE,
        canonical_path      TEXT NOT NULL UNIQUE,
        relative_path       TEXT NOT NULL,
        track_id            TEXT NOT NULL REFERENCES songs(song_id),
        mime_type           TEXT NOT NULL,
        file_size_bytes     INTEGER NOT NULL CHECK (file_size_bytes >= 0),
        modified_at_ms      INTEGER NOT NULL,
        first_seen_at_ms    INTEGER NOT NULL,
        missing_since_ms    INTEGER,
        UNIQUE(directory_id, relative_path)
    );

    CREATE TABLE local_artists (
        artist_id        INTEGER PRIMARY KEY AUTOINCREMENT,
        normalized_name  TEXT NOT NULL UNIQUE,
        display_name     TEXT NOT NULL,
        enabled          INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1))
    );

    CREATE TABLE local_track_artists (
        track_id    TEXT NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        artist_id   INTEGER NOT NULL REFERENCES local_artists(artist_id),
        credit_order INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (track_id, artist_id)
    );

    CREATE TABLE app_settings (
        singleton_id       INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
        discovery_enabled  INTEGER NOT NULL DEFAULT 0 CHECK (discovery_enabled IN (0, 1)),
        updated_at_ms      INTEGER NOT NULL
    );
    INSERT INTO app_settings (singleton_id, discovery_enabled, updated_at_ms)
    VALUES (1, 0, CAST(strftime('%s', 'now') AS INTEGER) * 1000);

    CREATE INDEX local_files_directory_idx ON local_files(directory_id, missing_since_ms);
    CREATE INDEX local_files_track_idx ON local_files(track_id, missing_since_ms);
    CREATE INDEX local_artists_name_idx ON local_artists(display_name COLLATE NOCASE);
    CREATE INDEX local_track_artists_artist_idx ON local_track_artists(artist_id, track_id);

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (3, 'local_library_and_discovery', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
"#,
    r#"
    ALTER TABLE music_directories ADD COLUMN last_scan_attempt_at_ms INTEGER;
    ALTER TABLE music_directories ADD COLUMN status TEXT NOT NULL DEFAULT 'READY';
    ALTER TABLE music_directories ADD COLUMN last_scan_error TEXT;
    ALTER TABLE music_directories ADD COLUMN track_count INTEGER NOT NULL DEFAULT 0 CHECK (track_count >= 0);
    ALTER TABLE local_files ADD COLUMN source_identity TEXT;

    CREATE INDEX local_files_source_identity_idx
        ON local_files(source_identity) WHERE source_identity IS NOT NULL;

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (4, 'alpha_filesystem_hardening', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
"#,
    r#"
    CREATE TABLE jellyfin_installation (
        singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
        device_id TEXT NOT NULL
    );
    INSERT INTO jellyfin_installation (singleton_id, device_id)
    VALUES (1, lower(hex(randomblob(16))));

    CREATE TABLE jellyfin_servers (
        server_id INTEGER PRIMARY KEY AUTOINCREMENT,
        server_internal_id TEXT NOT NULL,
        name TEXT NOT NULL,
        base_url TEXT NOT NULL UNIQUE,
        user_id TEXT NOT NULL,
        username TEXT NOT NULL,
        token_reference TEXT NOT NULL UNIQUE,
        device_id TEXT NOT NULL,
        last_connected_at_ms INTEGER,
        last_sync_at_ms INTEGER,
        status TEXT NOT NULL DEFAULT 'CONNECTED',
        last_error TEXT
    );

    CREATE TABLE jellyfin_libraries (
        server_id INTEGER NOT NULL REFERENCES jellyfin_servers(server_id) ON DELETE CASCADE,
        library_id TEXT NOT NULL,
        name TEXT NOT NULL,
        collection_type TEXT,
        enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
        last_sync_at_ms INTEGER,
        track_count INTEGER NOT NULL DEFAULT 0 CHECK (track_count >= 0),
        PRIMARY KEY (server_id, library_id)
    );

    CREATE TABLE jellyfin_track_sources (
        track_id TEXT NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        server_id INTEGER NOT NULL REFERENCES jellyfin_servers(server_id) ON DELETE CASCADE,
        library_id TEXT NOT NULL,
        item_id TEXT NOT NULL,
        container TEXT,
        available INTEGER NOT NULL DEFAULT 1 CHECK (available IN (0, 1)),
        remote_updated_at_ms INTEGER,
        last_seen_at_ms INTEGER NOT NULL,
        PRIMARY KEY (server_id, item_id),
        FOREIGN KEY (server_id, library_id) REFERENCES jellyfin_libraries(server_id, library_id) ON DELETE CASCADE
    );

    CREATE INDEX jellyfin_track_sources_track_idx
        ON jellyfin_track_sources(track_id, available);
    CREATE INDEX jellyfin_libraries_enabled_idx
        ON jellyfin_libraries(server_id, enabled);

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (5, 'jellyfin_library_sources', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
"#,
    r#"
    CREATE TABLE profiles (
        profile_id      TEXT PRIMARY KEY NOT NULL,
        name            TEXT NOT NULL COLLATE NOCASE UNIQUE,
        created_at_ms   INTEGER NOT NULL,
        last_used_at_ms INTEGER NOT NULL
    );

    CREATE TABLE active_profile (
        singleton_id  INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
        profile_id    TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE RESTRICT,
        updated_at_ms INTEGER NOT NULL
    );

    INSERT INTO profiles (profile_id, name, created_at_ms, last_used_at_ms)
    VALUES (
        '00000000-0000-7000-8000-000000000001',
        'Main',
        CAST(strftime('%s', 'now') AS INTEGER) * 1000,
        CAST(strftime('%s', 'now') AS INTEGER) * 1000
    );
    INSERT INTO active_profile (singleton_id, profile_id, updated_at_ms)
    VALUES (
        1,
        '00000000-0000-7000-8000-000000000001',
        CAST(strftime('%s', 'now') AS INTEGER) * 1000
    );

    CREATE TABLE track_stats_profiled (
        profile_id         TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        song_id            TEXT NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        play_count          INTEGER NOT NULL DEFAULT 0 CHECK (play_count >= 0),
        completed_count     INTEGER NOT NULL DEFAULT 0 CHECK (completed_count >= 0),
        early_skip_count    INTEGER NOT NULL DEFAULT 0 CHECK (early_skip_count >= 0),
        completion_ema      REAL NOT NULL DEFAULT 0.0,
        completion_samples  INTEGER NOT NULL DEFAULT 0 CHECK (completion_samples >= 0),
        affinity            REAL NOT NULL DEFAULT 0.0,
        liked               INTEGER NOT NULL DEFAULT 0 CHECK (liked IN (0, 1)),
        disliked            INTEGER NOT NULL DEFAULT 0 CHECK (disliked IN (0, 1)),
        last_played_at_ms    INTEGER,
        PRIMARY KEY (profile_id, song_id)
    );
    INSERT INTO track_stats_profiled
        (profile_id, song_id, play_count, completed_count, early_skip_count,
         completion_ema, completion_samples, affinity, liked, disliked, last_played_at_ms)
    SELECT '00000000-0000-7000-8000-000000000001', song_id, play_count,
           completed_count, early_skip_count, completion_ema, completion_samples,
           affinity, liked, disliked, last_played_at_ms
    FROM track_stats;
    DROP TABLE track_stats;
    ALTER TABLE track_stats_profiled RENAME TO track_stats;

    CREATE TABLE processed_playback_events_profiled (
        profile_id TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        event_id   TEXT NOT NULL,
        PRIMARY KEY (profile_id, event_id)
    );
    INSERT INTO processed_playback_events_profiled (profile_id, event_id)
    SELECT '00000000-0000-7000-8000-000000000001', event_id
    FROM processed_playback_events;
    DROP TABLE processed_playback_events;
    ALTER TABLE processed_playback_events_profiled RENAME TO processed_playback_events;

    CREATE TABLE recent_plays_profiled (
        sequence_id   INTEGER PRIMARY KEY AUTOINCREMENT,
        profile_id    TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        event_id      TEXT NOT NULL,
        song_id       TEXT NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        started_at_ms INTEGER NOT NULL,
        listened_ms   INTEGER NOT NULL,
        duration_ms   INTEGER,
        end_reason    TEXT NOT NULL,
        UNIQUE (profile_id, event_id)
    );
    INSERT INTO recent_plays_profiled
        (sequence_id, profile_id, event_id, song_id, started_at_ms, listened_ms,
         duration_ms, end_reason)
    SELECT sequence_id, '00000000-0000-7000-8000-000000000001', event_id,
           song_id, started_at_ms, listened_ms, duration_ms, end_reason
    FROM recent_plays;
    DROP TABLE recent_plays;
    ALTER TABLE recent_plays_profiled RENAME TO recent_plays;

    CREATE TABLE profile_artist_stats (
        profile_id         TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        artist_key         TEXT NOT NULL,
        artist_name        TEXT NOT NULL,
        play_count         INTEGER NOT NULL DEFAULT 0 CHECK (play_count >= 0),
        completed_count    INTEGER NOT NULL DEFAULT 0 CHECK (completed_count >= 0),
        early_skip_count   INTEGER NOT NULL DEFAULT 0 CHECK (early_skip_count >= 0),
        completion_ema     REAL NOT NULL DEFAULT 0.0,
        completion_samples INTEGER NOT NULL DEFAULT 0 CHECK (completion_samples >= 0),
        affinity           REAL NOT NULL DEFAULT 0.0,
        last_played_at_ms  INTEGER,
        PRIMARY KEY (profile_id, artist_key)
    );
    INSERT INTO profile_artist_stats
        (profile_id, artist_key, artist_name, play_count, completed_count,
         early_skip_count, completion_ema, completion_samples, affinity, last_played_at_ms)
    SELECT t.profile_id,
           COALESCE(s.artist_id, 'name:' || lower(trim(s.artist_name))),
           MAX(s.artist_name), SUM(t.play_count), SUM(t.completed_count),
           SUM(t.early_skip_count),
           CASE WHEN SUM(t.completion_samples) = 0 THEN 0.0
                ELSE SUM(t.completion_ema * t.completion_samples) / SUM(t.completion_samples) END,
           SUM(t.completion_samples), SUM(t.affinity), MAX(t.last_played_at_ms)
    FROM track_stats t JOIN songs s ON s.song_id = t.song_id
    GROUP BY t.profile_id, COALESCE(s.artist_id, 'name:' || lower(trim(s.artist_name)));

    CREATE TABLE profile_recommendation_state (
        profile_id    TEXT PRIMARY KEY NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        state_json    TEXT NOT NULL DEFAULT '{}',
        updated_at_ms INTEGER NOT NULL
    );
    INSERT INTO profile_recommendation_state (profile_id, updated_at_ms)
    SELECT profile_id, created_at_ms FROM profiles;

    CREATE TABLE profile_suppressions (
        profile_id    TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        entity_kind   TEXT NOT NULL,
        entity_id     TEXT NOT NULL,
        reason        TEXT,
        created_at_ms INTEGER NOT NULL,
        expires_at_ms INTEGER,
        PRIMARY KEY (profile_id, entity_kind, entity_id)
    );

    CREATE TABLE profile_release_interactions (
        interaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
        profile_id     TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        provider       TEXT NOT NULL,
        release_id     TEXT NOT NULL,
        artist_id      TEXT,
        interaction    TEXT NOT NULL,
        occurred_at_ms INTEGER NOT NULL
    );

    CREATE INDEX track_stats_profile_affinity_idx
        ON track_stats(profile_id, affinity DESC, last_played_at_ms DESC);
    CREATE INDEX profile_artist_stats_affinity_idx
        ON profile_artist_stats(profile_id, affinity DESC, last_played_at_ms DESC);
    CREATE INDEX track_stats_profile_liked_idx
        ON track_stats(profile_id, liked, last_played_at_ms DESC) WHERE liked = 1;
    CREATE INDEX recent_plays_profile_sequence_idx
        ON recent_plays(profile_id, sequence_id DESC);
    CREATE INDEX recent_plays_profile_track_idx
        ON recent_plays(profile_id, song_id, sequence_id DESC);
    CREATE INDEX recent_plays_profile_started_idx
        ON recent_plays(profile_id, started_at_ms DESC);
    CREATE INDEX profile_suppressions_expiry_idx
        ON profile_suppressions(profile_id, expires_at_ms);
    CREATE INDEX profile_release_interactions_profile_time_idx
        ON profile_release_interactions(profile_id, occurred_at_ms DESC);

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (6, 'local_listening_profiles', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
"#,
    r#"
    CREATE INDEX jellyfin_track_sources_library_available_idx
        ON jellyfin_track_sources(server_id, library_id, available, track_id);

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (7, 'unified_indexed_library', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
"#,
    r#"
    CREATE TABLE playlists (
        playlist_id  TEXT PRIMARY KEY NOT NULL,
        profile_id   TEXT NOT NULL REFERENCES profiles(profile_id) ON DELETE CASCADE,
        name         TEXT NOT NULL COLLATE NOCASE,
        created_at_ms INTEGER NOT NULL,
        updated_at_ms INTEGER NOT NULL,
        UNIQUE (profile_id, name)
    );

    CREATE TABLE playlist_items (
        playlist_id TEXT NOT NULL REFERENCES playlists(playlist_id) ON DELETE CASCADE,
        song_id     TEXT NOT NULL REFERENCES songs(song_id) ON DELETE CASCADE,
        position    INTEGER NOT NULL CHECK (position >= 0),
        added_at_ms INTEGER NOT NULL,
        PRIMARY KEY (playlist_id, position),
        UNIQUE (playlist_id, song_id)
    );

    CREATE INDEX playlists_profile_created_idx
        ON playlists(profile_id, created_at_ms, playlist_id);

    INSERT INTO schema_migrations (version, name, applied_at_ms)
    VALUES (8, 'profile_playlists', CAST(strftime('%s', 'now') AS INTEGER) * 1000);
"#,
];

const COMPLETION_EMA_ALPHA: f64 = 0.2;
const LIKE_AFFINITY: f64 = 3.0;
const DISLIKE_AFFINITY: f64 = -3.0;
const ARTIST_REACTION_WEIGHT: f64 = 0.5;
const MAX_RECENT_PLAYS: i64 = 1_000;

/// SQLite-backed implementation of the application's music repository port.
pub struct SqliteMusicRepository {
    connection: Mutex<Connection>,
}

impl SqliteMusicRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let connection = Connection::open(path).map_err(storage_error)?;
        Self::from_connection(connection)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory().map_err(storage_error)?;
        Self::from_connection(connection)
    }

    fn from_connection(mut connection: Connection) -> Result<Self, StorageError> {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = 5000;
                 PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;",
            )
            .map_err(storage_error)?;
        run_migrations(&mut connection)?;
        repair_active_profile(&mut connection)?;
        validate_database(&connection)?;
        info!(
            category = "DATABASE",
            event = "database_ready",
            schema_version = LATEST_SCHEMA_VERSION
        );
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError("SQLite connection mutex was poisoned".into()))
    }
}

impl MusicRepository for SqliteMusicRepository {
    fn save_songs(&self, songs: &[Song]) -> Result<(), StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        for song in songs {
            upsert_song(&tx, song)?;
        }
        tx.commit().map_err(storage_error)
    }

    fn record_playback(&self, summary: &ListeningSummary) -> Result<(), StorageError> {
        if summary.event_id.trim().is_empty() {
            return Err(StorageError("playback event_id cannot be blank".into()));
        }

        let listened_ms = sqlite_integer(summary.listened_ms, "listened_ms")?;
        let duration_ms = optional_sqlite_integer(summary.duration_ms, "duration_ms")?;
        let completion = summary.completion();
        let meaningful = summary.is_meaningful();
        let completed = summary.is_completed();
        let early_skip = summary.is_early_skip();

        // Playback affinity rewards intentional listening and completion, while an
        // early skip is a negative signal. A completed meaningful play contributes 2.
        let affinity_delta =
            f64::from(meaningful as u8) + f64::from(completed as u8) - f64::from(early_skip as u8);

        if summary.profile_id.trim().is_empty() {
            return Err(StorageError("playback profile_id cannot be blank".into()));
        }

        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        ensure_profile_exists(&tx, &summary.profile_id)?;
        let is_new_event = tx
            .execute(
                "INSERT OR IGNORE INTO processed_playback_events (profile_id, event_id) VALUES (?1, ?2)",
                params![summary.profile_id, summary.event_id],
            )
            .map_err(storage_error)?;
        if is_new_event == 0 {
            debug!(
                category = "HISTORY",
                event = "duplicate_playback_event_ignored",
                event_id = summary.event_id
            );
            tx.commit().map_err(storage_error)?;
            return Ok(());
        }

        upsert_song(&tx, &summary.song)?;
        tx.execute(
            "INSERT INTO recent_plays
             (profile_id, event_id, song_id, started_at_ms, listened_ms, duration_ms, end_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                summary.profile_id,
                summary.event_id,
                summary.song.id.as_str(),
                summary.started_at_ms,
                listened_ms,
                duration_ms,
                format!("{:?}", summary.reason),
            ],
        )
        .map_err(storage_error)?;

        tx.execute(
            "INSERT INTO track_stats
             (profile_id, song_id, play_count, completed_count, early_skip_count, completion_ema,
              completion_samples, affinity, last_played_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, 0.0), CASE WHEN ?6 IS NULL THEN 0 ELSE 1 END, ?7, ?8)
             ON CONFLICT(profile_id, song_id) DO UPDATE SET
                 play_count = play_count + excluded.play_count,
                 completed_count = completed_count + excluded.completed_count,
                 early_skip_count = early_skip_count + excluded.early_skip_count,
                 completion_ema = CASE
                     WHEN ?6 IS NULL THEN completion_ema
                     WHEN completion_samples = 0 THEN ?6
                     ELSE completion_ema * (1.0 - ?9) + ?6 * ?9
                 END,
                 completion_samples = completion_samples + CASE WHEN ?6 IS NULL THEN 0 ELSE 1 END,
                 affinity = affinity + excluded.affinity,
                 last_played_at_ms = CASE
                     WHEN last_played_at_ms IS NULL OR excluded.last_played_at_ms > last_played_at_ms
                     THEN excluded.last_played_at_ms ELSE last_played_at_ms END",
            params![
                summary.profile_id,
                summary.song.id.as_str(),
                i64::from(meaningful),
                i64::from(completed),
                i64::from(early_skip),
                completion,
                affinity_delta,
                summary.started_at_ms,
                COMPLETION_EMA_ALPHA,
            ],
        )
        .map_err(storage_error)?;

        tx.execute(
            "INSERT INTO profile_artist_stats
             (profile_id, artist_key, artist_name, play_count, completed_count,
              early_skip_count, completion_ema, completion_samples, affinity, last_played_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, COALESCE(?7, 0.0),
                     CASE WHEN ?7 IS NULL THEN 0 ELSE 1 END, ?8, ?9)
             ON CONFLICT(profile_id, artist_key) DO UPDATE SET
                 artist_name = excluded.artist_name,
                 play_count = play_count + excluded.play_count,
                 completed_count = completed_count + excluded.completed_count,
                 early_skip_count = early_skip_count + excluded.early_skip_count,
                 completion_ema = CASE
                     WHEN ?7 IS NULL THEN completion_ema
                     WHEN completion_samples = 0 THEN ?7
                     ELSE completion_ema * (1.0 - ?10) + ?7 * ?10
                 END,
                 completion_samples = completion_samples + CASE WHEN ?7 IS NULL THEN 0 ELSE 1 END,
                 affinity = affinity + excluded.affinity,
                 last_played_at_ms = CASE
                     WHEN last_played_at_ms IS NULL OR excluded.last_played_at_ms > last_played_at_ms
                     THEN excluded.last_played_at_ms ELSE last_played_at_ms END",
            params![
                summary.profile_id,
                artist_key(&summary.song),
                summary.song.artist.name,
                i64::from(meaningful),
                i64::from(completed),
                i64::from(early_skip),
                completion,
                affinity_delta,
                summary.started_at_ms,
                COMPLETION_EMA_ALPHA,
            ],
        )
        .map_err(storage_error)?;

        tx.execute(
            "DELETE FROM recent_plays
             WHERE profile_id = ?1 AND sequence_id <= (
                 SELECT sequence_id FROM recent_plays
                 WHERE profile_id = ?1
                 ORDER BY sequence_id DESC LIMIT 1 OFFSET ?2
             )",
            params![summary.profile_id, MAX_RECENT_PLAYS],
        )
        .map_err(storage_error)?;

        tx.commit().map_err(storage_error)
    }

    fn set_reaction(&self, song: &Song, liked: bool, disliked: bool) -> Result<(), StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        if liked && disliked {
            return Err(StorageError(
                "a track cannot be liked and disliked at the same time".into(),
            ));
        }
        upsert_song(&tx, song)?;
        let profile_id = active_profile_id(&tx)?;

        let old_reaction = tx
            .query_row(
                "SELECT liked, disliked FROM track_stats WHERE profile_id = ?1 AND song_id = ?2",
                params![profile_id, song.id.as_str()],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(storage_error)?
            .unwrap_or((false, false));

        let old_affinity = reaction_affinity(old_reaction.0, old_reaction.1);
        let new_affinity = reaction_affinity(liked, disliked);
        let affinity_delta = new_affinity - old_affinity;

        tx.execute(
            "INSERT INTO track_stats (profile_id, song_id, affinity, liked, disliked)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(profile_id, song_id) DO UPDATE SET
                 affinity = affinity + excluded.affinity,
                 liked = excluded.liked,
                 disliked = excluded.disliked",
            params![
                profile_id,
                song.id.as_str(),
                affinity_delta,
                liked,
                disliked
            ],
        )
        .map_err(storage_error)?;

        tx.execute(
            "INSERT INTO profile_artist_stats
             (profile_id, artist_key, artist_name, affinity)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(profile_id, artist_key) DO UPDATE SET
                 artist_name = excluded.artist_name,
                 affinity = affinity + excluded.affinity",
            params![
                profile_id,
                artist_key(song),
                song.artist.name,
                affinity_delta * ARTIST_REACTION_WEIGHT,
            ],
        )
        .map_err(storage_error)?;

        tx.commit().map_err(storage_error)
    }

    fn recent_songs(
        &self,
        limit: usize,
        before: Option<i64>,
    ) -> Result<Vec<RecentSong>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = sqlite_integer(limit as u64, "limit")?;
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                        s.album_name, s.duration_ms, s.thumbnail_url, recent.last_sequence
                 FROM songs s
                 JOIN (
                     SELECT song_id, MAX(sequence_id) AS last_sequence
                     FROM recent_plays WHERE profile_id = ?1 GROUP BY song_id
                 ) recent ON recent.song_id = s.song_id
                 WHERE ?3 IS NULL OR recent.last_sequence < ?3
                 ORDER BY recent.last_sequence DESC LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![profile_id, limit, before], |row| {
                Ok(RecentSong {
                    song: song_from_row(row)?,
                    cursor: row.get(8)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn track_profiles(&self, limit: usize) -> Result<Vec<TrackProfile>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = sqlite_integer(limit as u64, "limit")?;
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                        s.album_name, s.duration_ms, s.thumbnail_url,
                        t.play_count, t.completed_count, t.early_skip_count,
                        t.completion_ema, t.affinity, t.liked, t.disliked,
                        t.last_played_at_ms
                 FROM track_stats t JOIN songs s ON s.song_id = t.song_id
                 WHERE t.profile_id = ?1
                 ORDER BY t.affinity DESC, t.last_played_at_ms DESC, s.song_id
                 LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![profile_id, limit], track_profile_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn listening_profiles(&self) -> Result<Vec<ListeningProfile>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT profile_id, name, created_at_ms, last_used_at_ms
                 FROM profiles ORDER BY created_at_ms, profile_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], profile_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn active_listening_profile(&self) -> Result<ListeningProfile, StorageError> {
        self.connection()?
            .query_row(
                "SELECT p.profile_id, p.name, p.created_at_ms, p.last_used_at_ms
                 FROM active_profile active
                 JOIN profiles p ON p.profile_id = active.profile_id
                 WHERE active.singleton_id = 1",
                [],
                profile_from_row,
            )
            .map_err(storage_error)
    }

    fn create_listening_profile(
        &self,
        name: &str,
        now_ms: i64,
    ) -> Result<ListeningProfile, StorageError> {
        let name = validated_profile_name(name)?;
        let profile = ListeningProfile {
            id: uuid::Uuid::now_v7().to_string(),
            name,
            created_at_ms: now_ms,
            last_used_at_ms: now_ms,
        };
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        tx.execute(
            "INSERT INTO profiles (profile_id, name, created_at_ms, last_used_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                profile.id,
                profile.name,
                profile.created_at_ms,
                profile.last_used_at_ms
            ],
        )
        .map_err(profile_storage_error)?;
        tx.execute(
            "INSERT INTO profile_recommendation_state (profile_id, updated_at_ms)
             VALUES (?1, ?2)",
            params![profile.id, now_ms],
        )
        .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(profile)
    }

    fn rename_listening_profile(
        &self,
        profile_id: &str,
        name: &str,
    ) -> Result<ListeningProfile, StorageError> {
        let name = validated_profile_name(name)?;
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE profiles SET name = ?1 WHERE profile_id = ?2",
                params![name, profile_id],
            )
            .map_err(profile_storage_error)?;
        if changed == 0 {
            return Err(StorageError("listening profile was not found".into()));
        }
        connection
            .query_row(
                "SELECT profile_id, name, created_at_ms, last_used_at_ms
                 FROM profiles WHERE profile_id = ?1",
                [profile_id],
                profile_from_row,
            )
            .map_err(storage_error)
    }

    fn delete_listening_profile(&self, profile_id: &str) -> Result<(), StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        let profile_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM profiles", [], |row| row.get(0))
            .map_err(storage_error)?;
        if profile_count <= 1 {
            return Err(StorageError(
                "the last listening profile cannot be deleted".into(),
            ));
        }
        ensure_profile_exists(&tx, profile_id)?;
        let active_id = active_profile_id(&tx)?;
        if active_id == profile_id {
            let fallback_id: String = tx
                .query_row(
                    "SELECT profile_id FROM profiles WHERE profile_id <> ?1
                     ORDER BY created_at_ms, profile_id LIMIT 1",
                    [profile_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            tx.execute(
                "UPDATE active_profile SET profile_id = ?1 WHERE singleton_id = 1",
                [fallback_id],
            )
            .map_err(storage_error)?;
        }
        tx.execute("DELETE FROM profiles WHERE profile_id = ?1", [profile_id])
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)
    }

    fn set_active_listening_profile(
        &self,
        profile_id: &str,
        now_ms: i64,
    ) -> Result<ListeningProfile, StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        ensure_profile_exists(&tx, profile_id)?;
        tx.execute(
            "UPDATE profiles SET last_used_at_ms = ?1 WHERE profile_id = ?2",
            params![now_ms, profile_id],
        )
        .map_err(storage_error)?;
        tx.execute(
            "UPDATE active_profile SET profile_id = ?1, updated_at_ms = ?2
             WHERE singleton_id = 1",
            params![profile_id, now_ms],
        )
        .map_err(storage_error)?;
        let profile = tx
            .query_row(
                "SELECT profile_id, name, created_at_ms, last_used_at_ms
                 FROM profiles WHERE profile_id = ?1",
                [profile_id],
                profile_from_row,
            )
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(profile)
    }

    fn playlists(&self) -> Result<Vec<Playlist>, StorageError> {
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT p.playlist_id, p.name, p.created_at_ms, p.updated_at_ms,
                        COUNT(i.song_id)
                 FROM playlists p
                 LEFT JOIN playlist_items i ON i.playlist_id = p.playlist_id
                 WHERE p.profile_id = ?1
                 GROUP BY p.playlist_id
                 ORDER BY p.created_at_ms, p.playlist_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([profile_id], playlist_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn create_playlist(&self, name: &str, now_ms: i64) -> Result<Playlist, StorageError> {
        let name = validated_playlist_name(name)?;
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let playlist = Playlist {
            id: new_uuid_v4(&connection)?,
            name,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            track_count: 0,
        };
        connection
            .execute(
                "INSERT INTO playlists
                 (playlist_id, profile_id, name, created_at_ms, updated_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    playlist.id,
                    profile_id,
                    playlist.name,
                    playlist.created_at_ms,
                    playlist.updated_at_ms
                ],
            )
            .map_err(playlist_storage_error)?;
        Ok(playlist)
    }

    fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<PlaylistTrack>, StorageError> {
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        ensure_playlist_exists(&connection, &profile_id, playlist_id)?;
        let mut statement = connection
            .prepare(
                "SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                        s.album_name, s.duration_ms, s.thumbnail_url,
                        i.position, i.added_at_ms
                 FROM playlist_items i
                 JOIN playlists p ON p.playlist_id = i.playlist_id
                 JOIN songs s ON s.song_id = i.song_id
                 WHERE p.profile_id = ?1 AND p.playlist_id = ?2
                 ORDER BY i.position",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![profile_id, playlist_id], playlist_track_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn add_song_to_playlist(
        &self,
        playlist_id: &str,
        song: &Song,
        now_ms: i64,
    ) -> Result<PlaylistTrack, StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        let profile_id = active_profile_id(&tx)?;
        ensure_playlist_exists(&tx, &profile_id, playlist_id)?;
        upsert_song(&tx, song)?;
        let inserted = tx
            .execute(
                "INSERT INTO playlist_items (playlist_id, song_id, position, added_at_ms)
                 SELECT ?1, ?2, COALESCE(MAX(position) + 1, 0), ?3
                 FROM playlist_items WHERE playlist_id = ?1
                 ON CONFLICT(playlist_id, song_id) DO NOTHING",
                params![playlist_id, song.id.as_str(), now_ms],
            )
            .map_err(storage_error)?;
        if inserted != 0 {
            tx.execute(
                "UPDATE playlists SET updated_at_ms = ?1
                 WHERE playlist_id = ?2 AND profile_id = ?3",
                params![now_ms, playlist_id, profile_id],
            )
            .map_err(storage_error)?;
        }
        let item = tx
            .query_row(
                "SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                        s.album_name, s.duration_ms, s.thumbnail_url,
                        i.position, i.added_at_ms
                 FROM playlist_items i
                 JOIN playlists p ON p.playlist_id = i.playlist_id
                 JOIN songs s ON s.song_id = i.song_id
                 WHERE p.profile_id = ?1 AND p.playlist_id = ?2 AND i.song_id = ?3",
                params![profile_id, playlist_id, song.id.as_str()],
                playlist_track_from_row,
            )
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(item)
    }

    fn liked_song_ids(&self) -> Result<Vec<String>, StorageError> {
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT song_id FROM track_stats
                 WHERE profile_id = ?1 AND liked = 1
                 ORDER BY last_played_at_ms DESC, song_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([profile_id], |row| row.get(0))
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn artist_affinities(&self) -> Result<std::collections::HashMap<String, f64>, StorageError> {
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT artist_key, affinity FROM profile_artist_stats
                 WHERE profile_id = ?1",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([profile_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<std::collections::HashMap<_, _>>>()
            .map_err(storage_error)
    }

    fn discovery_enabled(&self) -> Result<bool, StorageError> {
        self.connection()?
            .query_row(
                "SELECT discovery_enabled FROM app_settings WHERE singleton_id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    fn set_discovery_enabled(&self, enabled: bool, now_ms: i64) -> Result<(), StorageError> {
        self.connection()?
            .execute(
                "UPDATE app_settings SET discovery_enabled = ?1, updated_at_ms = ?2 WHERE singleton_id = 1",
                params![enabled, now_ms],
            )
            .map(|_| ())
            .map_err(storage_error)
    }

    fn music_directories(&self) -> Result<Vec<MusicDirectory>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT directory_id, canonical_path, added_at_ms, last_scanned_at_ms,
                        last_scan_attempt_at_ms, status, last_scan_error, track_count
                 FROM music_directories ORDER BY canonical_path COLLATE NOCASE",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(MusicDirectory {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    added_at_ms: row.get(2)?,
                    last_scanned_at_ms: row.get(3)?,
                    last_scan_attempt_at_ms: row.get(4)?,
                    status: row.get(5)?,
                    last_error: row.get(6)?,
                    track_count: nonnegative_u64(row, 7)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn add_music_directory(&self, path: &str, now_ms: i64) -> Result<MusicDirectory, StorageError> {
        let connection = self.connection()?;
        connection
            .execute(
                "INSERT INTO music_directories (canonical_path, added_at_ms) VALUES (?1, ?2)",
                params![path, now_ms],
            )
            .map_err(storage_error)?;
        let id = connection.last_insert_rowid();
        Ok(MusicDirectory {
            id,
            path: path.into(),
            added_at_ms: now_ms,
            last_scanned_at_ms: None,
            last_scan_attempt_at_ms: None,
            status: "READY".into(),
            last_error: None,
            track_count: 0,
        })
    }

    fn remove_music_directory(&self, directory_id: i64) -> Result<(), StorageError> {
        let changed = self
            .connection()?
            .execute(
                "DELETE FROM music_directories WHERE directory_id = ?1",
                [directory_id],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            return Err(StorageError("music directory was not found".into()));
        }
        Ok(())
    }

    fn replace_directory_scan(
        &self,
        directory_id: i64,
        tracks: &[ScannedLocalTrack],
        scanned_at_ms: i64,
        skipped_files: usize,
        complete: bool,
        duration_ms: u64,
    ) -> Result<LibraryScanResult, StorageError> {
        let mut connection = self.connection()?;
        let tx = connection.transaction().map_err(storage_error)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM music_directories WHERE directory_id = ?1)",
                [directory_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if !exists {
            return Err(StorageError("music directory was not found".into()));
        }
        if complete {
            tx.execute(
                "UPDATE local_files SET missing_since_ms = ?2 WHERE directory_id = ?1",
                params![directory_id, scanned_at_ms],
            )
            .map_err(storage_error)?;
        }

        for track in tracks {
            let path_match = tx
                .query_row(
                    "SELECT local_file_id, track_id, first_seen_at_ms FROM local_files
                     WHERE canonical_path = ?1",
                    [&track.canonical_path],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_error)?;
            let identity_match = if path_match.is_none() {
                track
                    .source_identity
                    .as_deref()
                    .map_or(Ok(None), |identity| {
                        tx.query_row(
                            "SELECT local_file_id, track_id, first_seen_at_ms FROM local_files
                         WHERE source_identity = ?1 ORDER BY local_file_id LIMIT 1",
                            [identity],
                            |row| {
                                Ok((
                                    row.get::<_, i64>(0)?,
                                    row.get::<_, String>(1)?,
                                    row.get::<_, i64>(2)?,
                                ))
                            },
                        )
                        .optional()
                        .map_err(storage_error)
                    })?
            } else {
                None
            };
            let fingerprint_match = if path_match.is_none() && identity_match.is_none() {
                let mut statement = tx
                    .prepare(
                        "SELECT local_file_id, track_id, first_seen_at_ms FROM local_files
                         WHERE track_id = ?1 AND canonical_path <> ?2 AND missing_since_ms IS NOT NULL
                         ORDER BY local_file_id LIMIT 2",
                    )
                    .map_err(storage_error)?;
                let matches = statement
                    .query_map(
                        params![track.song.id.as_str(), track.canonical_path],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .map_err(storage_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(storage_error)?;
                (matches.len() == 1).then(|| matches[0].clone())
            } else {
                None
            };
            let matched = path_match.or(identity_match).or(fingerprint_match);
            let canonical_track_id = matched
                .as_ref()
                .map(|(_, track_id, _)| track_id.as_str())
                .unwrap_or_else(|| track.song.id.as_str());
            let mut canonical_song = track.song.clone();
            canonical_song.id = SongId::new(canonical_track_id.to_owned())
                .expect("stored and generated track IDs are nonblank");
            upsert_song(&tx, &canonical_song)?;

            if let Some((local_file_id, _, first_seen_at_ms)) = matched {
                tx.execute(
                    "UPDATE local_files SET directory_id = ?2, canonical_path = ?3,
                         relative_path = ?4, track_id = ?5, mime_type = ?6,
                         file_size_bytes = ?7, modified_at_ms = ?8, source_identity = ?9,
                         first_seen_at_ms = ?10, missing_since_ms = NULL
                     WHERE local_file_id = ?1",
                    params![
                        local_file_id,
                        directory_id,
                        track.canonical_path,
                        track.relative_path,
                        canonical_track_id,
                        track.mime_type,
                        sqlite_integer(track.file_size_bytes, "local file size")?,
                        track.modified_at_ms,
                        track.source_identity,
                        first_seen_at_ms,
                    ],
                )
                .map_err(storage_error)?;
            } else {
                tx.execute(
                    "INSERT INTO local_files
                     (directory_id, canonical_path, relative_path, track_id, mime_type,
                      file_size_bytes, modified_at_ms, source_identity, first_seen_at_ms, missing_since_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
                    params![
                        directory_id,
                        track.canonical_path,
                        track.relative_path,
                        canonical_track_id,
                        track.mime_type,
                        sqlite_integer(track.file_size_bytes, "local file size")?,
                        track.modified_at_ms,
                        track.source_identity,
                        track.first_seen_at_ms,
                    ],
                )
                .map_err(storage_error)?;
            }

            tx.execute(
                "DELETE FROM local_track_artists WHERE track_id = ?1",
                [canonical_track_id],
            )
            .map_err(storage_error)?;
            for (order, name) in track.artist_names.iter().enumerate() {
                let display_name = normalized_artist_display(name);
                let normalized_name = display_name.to_lowercase();
                tx.execute(
                    "INSERT INTO local_artists (normalized_name, display_name, enabled)
                     VALUES (?1, ?2, 1)
                     ON CONFLICT(normalized_name) DO UPDATE SET display_name = excluded.display_name",
                    params![normalized_name, display_name],
                )
                .map_err(storage_error)?;
                let artist_id: i64 = tx
                    .query_row(
                        "SELECT artist_id FROM local_artists WHERE normalized_name = ?1",
                        [normalized_name],
                        |row| row.get(0),
                    )
                    .map_err(storage_error)?;
                tx.execute(
                    "INSERT OR IGNORE INTO local_track_artists (track_id, artist_id, credit_order)
                     VALUES (?1, ?2, ?3)",
                    params![canonical_track_id, artist_id, order as i64],
                )
                .map_err(storage_error)?;
            }
        }

        let status = if complete { "READY" } else { "ERROR" };
        let scan_error = (!complete).then(|| {
            format!("scan incomplete: {skipped_files} files or entries could not be read")
        });
        let track_count: u64 = tx
            .query_row(
                "SELECT COUNT(*) FROM local_files WHERE directory_id = ?1 AND missing_since_ms IS NULL",
                [directory_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        tx.execute(
            "UPDATE music_directories SET
                 last_scanned_at_ms = CASE WHEN ?2 THEN ?3 ELSE last_scanned_at_ms END,
                 last_scan_attempt_at_ms = ?3, status = ?4, last_scan_error = ?5,
                 track_count = ?6
             WHERE directory_id = ?1",
            params![
                directory_id,
                complete,
                scanned_at_ms,
                status,
                scan_error,
                sqlite_integer(track_count, "directory track count")?
            ],
        )
        .map_err(storage_error)?;
        let unavailable_tracks: usize = tx
            .query_row(
                "SELECT COUNT(*) FROM local_files WHERE directory_id = ?1 AND missing_since_ms IS NOT NULL",
                [directory_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        Ok(LibraryScanResult {
            directory_id,
            status: status.into(),
            indexed_tracks: tracks.len(),
            unavailable_tracks,
            skipped_files,
            duration_ms,
            error: scan_error,
        })
    }

    fn set_music_directory_status(
        &self,
        directory_id: i64,
        status: &str,
        error: Option<&str>,
        attempted_at_ms: i64,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE music_directories SET status = ?2, last_scan_error = ?3,
             last_scan_attempt_at_ms = ?4 WHERE directory_id = ?1",
                params![directory_id, status, error, attempted_at_ms],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            return Err(StorageError("music directory was not found".into()));
        }
        Ok(())
    }

    fn local_artists(
        &self,
        query: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LocalArtist>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT la.artist_id, la.display_name, la.enabled, COUNT(DISTINCT lf.track_id)
                 FROM local_artists la
                 JOIN local_track_artists lta ON lta.artist_id = la.artist_id
                 JOIN local_files lf ON lf.track_id = lta.track_id AND lf.missing_since_ms IS NULL
                 WHERE la.display_name LIKE '%' || ?1 || '%' COLLATE NOCASE
                 GROUP BY la.artist_id
                 ORDER BY la.display_name COLLATE NOCASE
                 LIMIT ?2 OFFSET ?3",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![query, limit as i64, offset as i64], |row| {
                Ok(LocalArtist {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    enabled: row.get(2)?,
                    track_count: row.get(3)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn set_local_artist_enabled(&self, artist_id: i64, enabled: bool) -> Result<(), StorageError> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE local_artists SET enabled = ?2 WHERE artist_id = ?1",
                params![artist_id, enabled],
            )
            .map_err(storage_error)?;
        if changed == 0 {
            return Err(StorageError("local artist was not found".into()));
        }
        Ok(())
    }

    fn search_local(&self, query: &str, limit: usize) -> Result<Vec<Song>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                        s.album_name, s.duration_ms, s.thumbnail_url
                 FROM songs s
                 JOIN local_files lf ON lf.track_id = s.song_id AND lf.missing_since_ms IS NULL
                 WHERE (s.title LIKE '%' || ?1 || '%' COLLATE NOCASE
                    OR s.artist_name LIKE '%' || ?1 || '%' COLLATE NOCASE
                    OR COALESCE(s.album_name, '') LIKE '%' || ?1 || '%' COLLATE NOCASE)
                   AND EXISTS (
                     SELECT 1 FROM local_track_artists lta
                     JOIN local_artists la ON la.artist_id = lta.artist_id
                     WHERE lta.track_id = s.song_id AND la.enabled = 1
                   )
                 ORDER BY s.title COLLATE NOCASE, s.artist_name COLLATE NOCASE
                 LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![query, limit as i64], song_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn library_tracks(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LibraryTrack>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH library_items AS (
                    SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                           s.album_name, s.duration_ms, s.thumbnail_url,
                           'local' AS source, MIN(md.canonical_path) AS source_name
                    FROM songs s
                    JOIN local_files lf ON lf.track_id = s.song_id AND lf.missing_since_ms IS NULL
                    JOIN music_directories md ON md.directory_id = lf.directory_id
                    WHERE EXISTS (
                        SELECT 1 FROM local_track_artists lta
                        JOIN local_artists la ON la.artist_id = lta.artist_id
                        WHERE lta.track_id = s.song_id AND la.enabled = 1
                    )
                    GROUP BY s.song_id
                    UNION ALL
                    SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                           s.album_name, s.duration_ms, s.thumbnail_url,
                           'jellyfin' AS source, MIN(js.name || ' · ' || jl.name) AS source_name
                    FROM songs s
                    JOIN jellyfin_track_sources jts ON jts.track_id = s.song_id AND jts.available = 1
                    JOIN jellyfin_libraries jl ON jl.server_id = jts.server_id
                         AND jl.library_id = jts.library_id AND jl.enabled = 1
                    JOIN jellyfin_servers js ON js.server_id = jts.server_id
                    GROUP BY s.song_id
                 )
                 SELECT * FROM library_items
                 ORDER BY title COLLATE NOCASE, artist_name COLLATE NOCASE, song_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![limit as i64, offset as i64], library_track_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn library_albums(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LibraryAlbum>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH albums AS (
                    SELECT s.album_id, COALESCE(s.album_name, 'Unknown Album') AS title,
                           MIN(s.artist_name) AS artist_name, MAX(s.thumbnail_url) AS thumbnail_url,
                           COUNT(DISTINCT s.song_id) AS track_count, 'local' AS source,
                           MIN(md.canonical_path) AS source_name
                    FROM songs s
                    JOIN local_files lf ON lf.track_id = s.song_id AND lf.missing_since_ms IS NULL
                    JOIN music_directories md ON md.directory_id = lf.directory_id
                    WHERE s.album_id IS NOT NULL AND EXISTS (
                        SELECT 1 FROM local_track_artists lta
                        JOIN local_artists la ON la.artist_id = lta.artist_id
                        WHERE lta.track_id = s.song_id AND la.enabled = 1
                    )
                    GROUP BY s.album_id, s.album_name
                    UNION ALL
                    SELECT s.album_id, COALESCE(s.album_name, 'Unknown Album'),
                           MIN(s.artist_name), MAX(s.thumbnail_url), COUNT(DISTINCT s.song_id),
                           'jellyfin', MIN(js.name || ' · ' || jl.name)
                    FROM songs s
                    JOIN jellyfin_track_sources jts ON jts.track_id = s.song_id AND jts.available = 1
                    JOIN jellyfin_libraries jl ON jl.server_id = jts.server_id
                         AND jl.library_id = jts.library_id AND jl.enabled = 1
                    JOIN jellyfin_servers js ON js.server_id = jts.server_id
                    WHERE s.album_id IS NOT NULL
                    GROUP BY s.album_id, s.album_name
                 )
                 SELECT * FROM albums
                 ORDER BY title COLLATE NOCASE, artist_name COLLATE NOCASE, album_id
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![limit as i64, offset as i64], |row| {
                Ok(LibraryAlbum {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    artist_name: row.get(2)?,
                    thumbnail_url: row.get(3)?,
                    track_count: nonnegative_u64(row, 4)?,
                    source: library_source_from_text(row.get::<_, String>(5)?),
                    source_name: row.get(6)?,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn library_album_tracks(&self, album_id: &str) -> Result<Vec<LibraryTrack>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH library_items AS (
                    SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                           s.album_name, s.duration_ms, s.thumbnail_url,
                           'local' AS source, MIN(md.canonical_path) AS source_name
                    FROM songs s
                    JOIN local_files lf ON lf.track_id = s.song_id AND lf.missing_since_ms IS NULL
                    JOIN music_directories md ON md.directory_id = lf.directory_id
                    WHERE s.album_id = ?1 AND EXISTS (
                        SELECT 1 FROM local_track_artists lta
                        JOIN local_artists la ON la.artist_id = lta.artist_id
                        WHERE lta.track_id = s.song_id AND la.enabled = 1
                    )
                    GROUP BY s.song_id
                    UNION ALL
                    SELECT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                           s.album_name, s.duration_ms, s.thumbnail_url,
                           'jellyfin', MIN(js.name || ' · ' || jl.name)
                    FROM songs s
                    JOIN jellyfin_track_sources jts ON jts.track_id = s.song_id AND jts.available = 1
                    JOIN jellyfin_libraries jl ON jl.server_id = jts.server_id
                         AND jl.library_id = jts.library_id AND jl.enabled = 1
                    JOIN jellyfin_servers js ON js.server_id = jts.server_id
                    WHERE s.album_id = ?1
                    GROUP BY s.song_id
                 )
                 SELECT * FROM library_items
                 ORDER BY title COLLATE NOCASE, song_id",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([album_id], library_track_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn library_folders(&self) -> Result<Vec<LibraryFolder>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT 'local-dir:' || md.directory_id, md.canonical_path, md.canonical_path,
                        COUNT(DISTINCT lf.track_id), 'local'
                 FROM music_directories md
                 LEFT JOIN local_files lf ON lf.directory_id = md.directory_id
                      AND lf.missing_since_ms IS NULL
                 GROUP BY md.directory_id
                 UNION ALL
                 SELECT 'jellyfin-lib:' || jl.server_id || ':' || jl.library_id,
                        jl.name, js.name, COUNT(DISTINCT jts.track_id), 'jellyfin'
                 FROM jellyfin_libraries jl
                 JOIN jellyfin_servers js ON js.server_id = jl.server_id
                 LEFT JOIN jellyfin_track_sources jts ON jts.server_id = jl.server_id
                      AND jts.library_id = jl.library_id AND jts.available = 1
                 WHERE jl.enabled = 1
                 GROUP BY jl.server_id, jl.library_id
                 ORDER BY 2 COLLATE NOCASE",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map([], |row| {
                let path: String = row.get(1)?;
                let source = library_source_from_text(row.get::<_, String>(4)?);
                let name = if source == LibrarySource::Local {
                    std::path::Path::new(&path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(&path)
                        .to_owned()
                } else {
                    path
                };
                Ok(LibraryFolder {
                    id: row.get(0)?,
                    name,
                    detail: row.get(2)?,
                    track_count: nonnegative_u64(row, 3)?,
                    source,
                })
            })
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn has_indexed_source(&self, song_id: &SongId) -> Result<bool, StorageError> {
        self.connection()?
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM local_files
                    WHERE track_id = ?1 AND missing_since_ms IS NULL
                    UNION ALL
                    SELECT 1 FROM jellyfin_track_sources jts
                    JOIN jellyfin_libraries jl ON jl.server_id = jts.server_id
                         AND jl.library_id = jts.library_id
                    WHERE jts.track_id = ?1 AND jts.available = 1 AND jl.enabled = 1
                 )",
                [song_id.as_str()],
                |row| row.get(0),
            )
            .map_err(storage_error)
    }

    fn local_track_profiles(&self, limit: usize) -> Result<Vec<TrackProfile>, StorageError> {
        let connection = self.connection()?;
        let profile_id = active_profile_id(&connection)?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT s.song_id, s.title, s.artist_id, s.artist_name, s.album_id,
                        s.album_name, s.duration_ms, s.thumbnail_url,
                        COALESCE(t.play_count, 0), COALESCE(t.completed_count, 0),
                        COALESCE(t.early_skip_count, 0), COALESCE(t.completion_ema, 0.0),
                        COALESCE(t.affinity, 0.0), COALESCE(t.liked, 0),
                        COALESCE(t.disliked, 0), t.last_played_at_ms
                 FROM songs s
                 LEFT JOIN track_stats t ON t.song_id = s.song_id AND t.profile_id = ?1
                 WHERE (
                   EXISTS (
                     SELECT 1 FROM local_files lf
                     WHERE lf.track_id = s.song_id AND lf.missing_since_ms IS NULL
                   ) AND EXISTS (
                     SELECT 1 FROM local_track_artists lta
                     JOIN local_artists la ON la.artist_id = lta.artist_id
                     WHERE lta.track_id = s.song_id AND la.enabled = 1
                   )
                 ) OR EXISTS (
                   SELECT 1 FROM jellyfin_track_sources jts
                   JOIN jellyfin_libraries jl ON jl.server_id = jts.server_id
                        AND jl.library_id = jts.library_id
                   WHERE jts.track_id = s.song_id AND jts.available = 1 AND jl.enabled = 1
                 )
                 ORDER BY COALESCE(t.affinity, 0.0) DESC,
                          COALESCE(t.last_played_at_ms, 0) DESC, s.song_id
                 LIMIT ?2",
            )
            .map_err(storage_error)?;
        let rows = statement
            .query_map(params![profile_id, limit as i64], track_profile_from_row)
            .map_err(storage_error)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_error)
    }

    fn local_playback_file(
        &self,
        song_id: &SongId,
    ) -> Result<Option<LocalPlaybackFile>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT canonical_path, mime_type FROM local_files
                 WHERE track_id = ?1 AND missing_since_ms IS NULL
                 ORDER BY local_file_id",
            )
            .map_err(storage_error)?;
        let candidates = statement
            .query_map([song_id.as_str()], |row| {
                Ok(LocalPlaybackFile {
                    path: std::path::PathBuf::from(row.get::<_, String>(0)?),
                    mime_type: row.get(1)?,
                })
            })
            .map_err(storage_error)?;
        for candidate in candidates {
            let candidate = candidate.map_err(storage_error)?;
            if std::fs::File::open(&candidate.path).is_ok() {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    fn database_diagnostics(&self) -> Result<DatabaseDiagnostics, StorageError> {
        let started = Instant::now();
        let connection = self.connection()?;
        let schema_version = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, usize>(0))
            .map_err(storage_error)?;
        let page_count = connection
            .query_row("PRAGMA page_count", [], |row| row.get::<_, u64>(0))
            .map_err(storage_error)?;
        let page_size = connection
            .query_row("PRAGMA page_size", [], |row| row.get::<_, u64>(0))
            .map_err(storage_error)?;
        let count = |table: &str| -> Result<u64, StorageError> {
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .map_err(storage_error)
        };
        let artist_count = connection
            .query_row(
                "SELECT COUNT(DISTINCT COALESCE(artist_id, lower(artist_name))) FROM songs",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let profile_id = active_profile_id(&connection)?;
        let liked_song_count = connection
            .query_row(
                "SELECT COUNT(*) FROM track_stats WHERE profile_id = ?1 AND liked = 1",
                [&profile_id],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        let integrity_status = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(storage_error)?;
        Ok(DatabaseDiagnostics {
            schema_version,
            database_size_bytes: page_count.saturating_mul(page_size),
            track_count: count("songs")?,
            artist_count,
            history_event_count: connection
                .query_row(
                    "SELECT COUNT(*) FROM recent_plays WHERE profile_id = ?1",
                    [profile_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?,
            liked_song_count,
            integrity_status,
            query_duration_ms: started.elapsed().as_millis() as u64,
        })
    }
}

fn artist_key(song: &Song) -> String {
    song.artist
        .id
        .clone()
        .unwrap_or_else(|| format!("name:{}", song.artist.name.trim().to_lowercase()))
}

fn active_profile_id(connection: &Connection) -> Result<String, StorageError> {
    connection
        .query_row(
            "SELECT profile_id FROM active_profile WHERE singleton_id = 1",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

fn ensure_profile_exists(connection: &Connection, profile_id: &str) -> Result<(), StorageError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE profile_id = ?1)",
            [profile_id],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if exists {
        Ok(())
    } else {
        Err(StorageError("listening profile was not found".into()))
    }
}

fn validated_profile_name(name: &str) -> Result<String, StorageError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StorageError("profile name cannot be blank".into()));
    }
    if name.chars().count() > 40 {
        return Err(StorageError(
            "profile name cannot be longer than 40 characters".into(),
        ));
    }
    Ok(name.to_owned())
}

fn profile_from_row(row: &Row<'_>) -> rusqlite::Result<ListeningProfile> {
    Ok(ListeningProfile {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at_ms: row.get(2)?,
        last_used_at_ms: row.get(3)?,
    })
}

fn profile_storage_error(error: rusqlite::Error) -> StorageError {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    ) {
        StorageError("a profile with that name already exists".into())
    } else {
        storage_error(error)
    }
}

fn validated_playlist_name(name: &str) -> Result<String, StorageError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(StorageError("playlist name cannot be blank".into()));
    }
    if name.chars().count() > 100 {
        return Err(StorageError(
            "playlist name cannot be longer than 100 characters".into(),
        ));
    }
    Ok(name.to_owned())
}

fn new_uuid_v4(connection: &Connection) -> Result<String, StorageError> {
    connection
        .query_row(
            "SELECT lower(hex(randomblob(4))) || '-' ||
                    lower(hex(randomblob(2))) || '-4' ||
                    substr(lower(hex(randomblob(2))), 2) || '-' ||
                    substr('89ab', abs(random()) % 4 + 1, 1) ||
                    substr(lower(hex(randomblob(2))), 2) || '-' ||
                    lower(hex(randomblob(6)))",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)
}

fn ensure_playlist_exists(
    connection: &Connection,
    profile_id: &str,
    playlist_id: &str,
) -> Result<(), StorageError> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM playlists WHERE profile_id = ?1 AND playlist_id = ?2
             )",
            params![profile_id, playlist_id],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if exists {
        Ok(())
    } else {
        Err(StorageError("playlist was not found".into()))
    }
}

fn playlist_storage_error(error: rusqlite::Error) -> StorageError {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    ) {
        StorageError("a playlist with that name already exists".into())
    } else {
        storage_error(error)
    }
}

fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at_ms: row.get(2)?,
        updated_at_ms: row.get(3)?,
        track_count: nonnegative_u64(row, 4)?,
    })
}

fn playlist_track_from_row(row: &Row<'_>) -> rusqlite::Result<PlaylistTrack> {
    Ok(PlaylistTrack {
        song: song_from_row(row)?,
        position: nonnegative_u64(row, 8)?,
        added_at_ms: row.get(9)?,
    })
}

fn repair_active_profile(connection: &mut Connection) -> Result<(), StorageError> {
    let tx = connection.transaction().map_err(storage_error)?;
    let profile_count: i64 = tx
        .query_row("SELECT COUNT(*) FROM profiles", [], |row| row.get(0))
        .map_err(storage_error)?;
    if profile_count == 0 {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or(0);
        tx.execute(
            "INSERT INTO profiles (profile_id, name, created_at_ms, last_used_at_ms)
             VALUES (?1, 'Main', ?2, ?2)",
            params![uuid::Uuid::now_v7().to_string(), now_ms],
        )
        .map_err(storage_error)?;
    }
    let active_is_valid: bool = tx
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM active_profile active
                 JOIN profiles p ON p.profile_id = active.profile_id
                 WHERE active.singleton_id = 1
             )",
            [],
            |row| row.get(0),
        )
        .map_err(storage_error)?;
    if !active_is_valid {
        let fallback_id: String = tx
            .query_row(
                "SELECT profile_id FROM profiles ORDER BY created_at_ms, profile_id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        tx.execute(
            "INSERT INTO active_profile (singleton_id, profile_id, updated_at_ms)
             VALUES (1, ?1, 0)
             ON CONFLICT(singleton_id) DO UPDATE SET profile_id = excluded.profile_id",
            [fallback_id],
        )
        .map_err(storage_error)?;
    }
    tx.commit().map_err(storage_error)
}

fn validate_database(connection: &Connection) -> Result<(), StorageError> {
    let integrity: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(storage_error)?;
    if integrity != "ok" {
        return Err(StorageError(format!(
            "database integrity check failed: {integrity}"
        )));
    }

    for table in [
        "songs",
        "track_stats",
        "processed_playback_events",
        "recent_plays",
        "schema_migrations",
        "player_state",
        "music_directories",
        "local_files",
        "local_artists",
        "local_track_artists",
        "app_settings",
        "jellyfin_installation",
        "jellyfin_servers",
        "jellyfin_libraries",
        "jellyfin_track_sources",
        "profiles",
        "active_profile",
        "profile_recommendation_state",
        "profile_artist_stats",
        "profile_suppressions",
        "profile_release_interactions",
        "playlists",
        "playlist_items",
    ] {
        let exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )
            .map_err(storage_error)?;
        if !exists {
            return Err(StorageError(format!(
                "database schema is missing required table {table}"
            )));
        }
    }

    let foreign_key_errors: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .map_err(storage_error)?;
    if foreign_key_errors != 0 {
        return Err(StorageError(format!(
            "database contains {foreign_key_errors} foreign key violations"
        )));
    }
    Ok(())
}

fn run_migrations(connection: &mut Connection) -> Result<(), StorageError> {
    let current_version: usize = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(storage_error)?;
    if current_version > LATEST_SCHEMA_VERSION {
        return Err(StorageError(format!(
            "database schema version {current_version} is newer than supported version {LATEST_SCHEMA_VERSION}"
        )));
    }

    for (index, migration) in MIGRATIONS.iter().enumerate().skip(current_version) {
        let version = index + 1;
        info!(category = "DATABASE", event = "migration_started", version);
        let tx = connection.transaction().map_err(storage_error)?;
        tx.execute_batch(migration).map_err(storage_error)?;
        tx.pragma_update(None, "user_version", version)
            .map_err(storage_error)?;
        tx.commit().map_err(storage_error)?;
        info!(
            category = "DATABASE",
            event = "migration_completed",
            version
        );
    }
    Ok(())
}

fn upsert_song(tx: &Transaction<'_>, song: &Song) -> Result<(), StorageError> {
    let duration_ms = optional_sqlite_integer(song.duration_ms, "song duration_ms")?;
    tx.execute(
        "INSERT INTO songs
         (song_id, title, artist_id, artist_name, album_id, album_name, duration_ms, thumbnail_url)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(song_id) DO UPDATE SET
             title = CASE WHEN excluded.title = '' THEN songs.title ELSE excluded.title END,
             artist_id = COALESCE(excluded.artist_id, songs.artist_id),
             artist_name = CASE WHEN excluded.artist_name = '' THEN songs.artist_name ELSE excluded.artist_name END,
             album_id = COALESCE(excluded.album_id, songs.album_id),
             album_name = COALESCE(excluded.album_name, songs.album_name),
             duration_ms = COALESCE(excluded.duration_ms, songs.duration_ms),
             thumbnail_url = COALESCE(excluded.thumbnail_url, songs.thumbnail_url)",
        params![
            song.id.as_str(),
            song.title,
            song.artist.id,
            song.artist.name,
            song.album_id,
            song.album_name,
            duration_ms,
            song.thumbnail_url,
        ],
    )
    .map_err(storage_error)?;
    Ok(())
}

fn song_from_row(row: &Row<'_>) -> rusqlite::Result<Song> {
    let raw_id: String = row.get(0)?;
    let id = SongId::new(raw_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            "blank song id in database".into(),
        )
    })?;
    let duration_ms = optional_u64(row, 6)?;
    Ok(Song {
        id,
        title: row.get(1)?,
        artist: ArtistRef {
            id: row.get(2)?,
            name: row.get(3)?,
        },
        album_id: row.get(4)?,
        album_name: row.get(5)?,
        duration_ms,
        thumbnail_url: row.get(7)?,
    })
}

fn library_source_from_text(value: String) -> LibrarySource {
    if value == "jellyfin" {
        LibrarySource::Jellyfin
    } else {
        LibrarySource::Local
    }
}

fn library_track_from_row(row: &Row<'_>) -> rusqlite::Result<LibraryTrack> {
    Ok(LibraryTrack {
        song: song_from_row(row)?,
        source: library_source_from_text(row.get(8)?),
        source_name: row.get(9)?,
    })
}

fn track_profile_from_row(row: &Row<'_>) -> rusqlite::Result<TrackProfile> {
    let song = song_from_row(row)?;
    Ok(TrackProfile {
        song,
        play_count: nonnegative_u64(row, 8)?,
        completed_count: nonnegative_u64(row, 9)?,
        early_skip_count: nonnegative_u64(row, 10)?,
        completion_ema: row.get(11)?,
        affinity: row.get(12)?,
        liked: row.get(13)?,
        disliked: row.get(14)?,
        last_played_at_ms: row.get(15)?,
    })
}

fn optional_u64(row: &Row<'_>, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<i64>>(index)?
        .map(|value| {
            u64::try_from(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

fn nonnegative_u64(row: &Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn sqlite_integer(value: u64, field: &str) -> Result<i64, StorageError> {
    i64::try_from(value)
        .map_err(|_| StorageError(format!("{field} exceeds SQLite's signed integer range")))
}

fn optional_sqlite_integer(value: Option<u64>, field: &str) -> Result<Option<i64>, StorageError> {
    value.map(|value| sqlite_integer(value, field)).transpose()
}

fn reaction_affinity(liked: bool, disliked: bool) -> f64 {
    (if liked { LIKE_AFFINITY } else { 0.0 }) + if disliked { DISLIKE_AFFINITY } else { 0.0 }
}

fn normalized_artist_display(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "Unknown Artist".into()
    } else {
        value.into()
    }
}

fn storage_error(error: impl std::fmt::Display) -> StorageError {
    StorageError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{SqliteMusicRepository, LATEST_SCHEMA_VERSION, MIGRATIONS};
    use rusqlite::Connection;
    use solmusic_application::{LibrarySource, MusicRepository, ScannedLocalTrack};
    use solmusic_domain::{ArtistRef, ListeningSummary, PlaybackEndReason, Song, SongId};

    fn song(id: &str) -> Song {
        Song {
            id: SongId::new(id).unwrap(),
            title: format!("Song {id}"),
            artist: ArtistRef {
                id: Some("artist-1".into()),
                name: "Artist".into(),
            },
            album_id: Some("album-1".into()),
            album_name: Some("Album".into()),
            duration_ms: Some(100_000),
            thumbnail_url: Some("https://example.test/cover.jpg".into()),
        }
    }

    fn scanned(track: Song, path: &str, source_identity: &str, seen_at: i64) -> ScannedLocalTrack {
        ScannedLocalTrack {
            song: track,
            canonical_path: path.into(),
            relative_path: path.rsplit('/').next().unwrap_or(path).into(),
            mime_type: "audio/flac".into(),
            file_size_bytes: 42,
            modified_at_ms: seen_at,
            source_identity: Some(source_identity.into()),
            artist_names: vec!["Artist".into()],
            first_seen_at_ms: seen_at,
        }
    }

    fn summary(
        event_id: &str,
        song: Song,
        listened_ms: u64,
        reason: PlaybackEndReason,
        started_at_ms: i64,
    ) -> ListeningSummary {
        ListeningSummary {
            event_id: event_id.into(),
            profile_id: "00000000-0000-7000-8000-000000000001".into(),
            song,
            started_at_ms,
            listened_ms,
            duration_ms: Some(100_000),
            reason,
        }
    }

    #[test]
    fn migrates_v1_database_and_validates_required_schema() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATIONS[0]).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();

        let repository = SqliteMusicRepository::from_connection(connection).unwrap();
        let version: usize = repository
            .connection()
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, LATEST_SCHEMA_VERSION);
        let migrations: i64 = repository
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(migrations, LATEST_SCHEMA_VERSION as i64);
    }

    #[test]
    fn rejects_future_schema_versions_without_resetting() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "user_version", LATEST_SCHEMA_VERSION + 1)
            .unwrap();
        let error = SqliteMusicRepository::from_connection(connection)
            .err()
            .expect("future schema must be rejected");
        assert!(error.0.contains("newer than supported"));
    }

    #[test]
    fn playback_event_is_idempotent() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let event = summary(
            "event-1",
            song("song-1"),
            90_000,
            PlaybackEndReason::Completed,
            10,
        );

        repository.record_playback(&event).unwrap();
        repository.record_playback(&event).unwrap();

        let profiles = repository.track_profiles(10).unwrap();
        assert_eq!(profiles.len(), 1);
        let profile = &profiles[0];
        assert_eq!(profile.play_count, 1);
        assert_eq!(profile.completed_count, 1);
        assert_eq!(profile.early_skip_count, 0);
        assert!((profile.completion_ema - 0.9).abs() < f64::EPSILON);
        assert_eq!(profile.affinity, 2.0);
        assert_eq!(
            repository
                .recent_songs(10, None)
                .unwrap()
                .into_iter()
                .map(|item| item.song)
                .collect::<Vec<_>>(),
            vec![event.song]
        );
    }

    #[test]
    fn playback_signals_aggregate_and_completion_uses_ema() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let track = song("song-1");

        repository
            .record_playback(&summary(
                "completed",
                track.clone(),
                100_000,
                PlaybackEndReason::Completed,
                10,
            ))
            .unwrap();
        repository
            .record_playback(&summary(
                "early-skip",
                track.clone(),
                10_000,
                PlaybackEndReason::Next,
                20,
            ))
            .unwrap();
        repository
            .record_playback(&summary(
                "meaningful",
                track.clone(),
                50_000,
                PlaybackEndReason::Stopped,
                30,
            ))
            .unwrap();

        let profile = repository.track_profiles(1).unwrap().pop().unwrap();
        assert_eq!(profile.song, track);
        assert_eq!(profile.play_count, 2);
        assert_eq!(profile.completed_count, 1);
        assert_eq!(profile.early_skip_count, 1);
        // EMA: 1.0 -> 0.82 after 0.1 -> 0.756 after 0.5.
        assert!((profile.completion_ema - 0.756).abs() < 1e-12);
        assert_eq!(profile.affinity, 2.0);
        assert_eq!(profile.last_played_at_ms, Some(30));
    }

    #[test]
    fn partial_metadata_does_not_erase_complete_metadata() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let complete = song("song-1");
        repository
            .save_songs(std::slice::from_ref(&complete))
            .unwrap();
        let mut partial = complete.clone();
        partial.album_id = None;
        partial.album_name = None;
        partial.duration_ms = None;
        partial.thumbnail_url = None;
        repository.save_songs(&[partial]).unwrap();

        let stored = repository
            .connection()
            .unwrap()
            .query_row(
                "SELECT song_id, title, artist_id, artist_name, album_id, album_name, duration_ms, thumbnail_url FROM songs WHERE song_id = 'song-1'",
                [],
                super::song_from_row,
            )
            .unwrap();
        assert_eq!(stored, complete);
    }

    #[test]
    fn invalid_combined_reaction_is_rejected() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let error = repository
            .set_reaction(&song("song-1"), true, true)
            .unwrap_err();
        assert!(error.0.contains("cannot be liked and disliked"));
        assert!(repository.track_profiles(10).unwrap().is_empty());
    }

    #[test]
    fn repeated_reaction_state_does_not_stack_affinity() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let track = song("song-1");

        repository.set_reaction(&track, true, false).unwrap();
        repository.set_reaction(&track, true, false).unwrap();
        let liked = repository.track_profiles(1).unwrap().pop().unwrap();
        assert!(liked.liked);
        assert!(!liked.disliked);
        assert_eq!(liked.affinity, 3.0);

        repository.set_reaction(&track, false, true).unwrap();
        repository.set_reaction(&track, false, true).unwrap();
        let disliked = repository.track_profiles(1).unwrap().pop().unwrap();
        assert!(!disliked.liked);
        assert!(disliked.disliked);
        assert_eq!(disliked.affinity, -3.0);
    }

    #[test]
    fn recent_songs_are_distinct_and_ordered_by_latest_play() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let first = song("first");
        let second = song("second");
        repository
            .record_playback(&summary(
                "one",
                first.clone(),
                40_000,
                PlaybackEndReason::Stopped,
                10,
            ))
            .unwrap();
        repository
            .record_playback(&summary(
                "two",
                second.clone(),
                40_000,
                PlaybackEndReason::Stopped,
                20,
            ))
            .unwrap();
        repository
            .record_playback(&summary(
                "three",
                first.clone(),
                40_000,
                PlaybackEndReason::Stopped,
                30,
            ))
            .unwrap();

        assert_eq!(
            repository
                .recent_songs(10, None)
                .unwrap()
                .into_iter()
                .map(|item| item.song)
                .collect::<Vec<_>>(),
            vec![first.clone(), second.clone()]
        );

        let first_page = repository.recent_songs(1, None).unwrap();
        assert_eq!(first_page[0].song, first);
        let second_page = repository
            .recent_songs(1, Some(first_page[0].cursor))
            .unwrap();
        assert_eq!(second_page[0].song, second);
    }

    #[test]
    fn local_artist_visibility_is_relational_and_persistent() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        assert!(!repository.discovery_enabled().unwrap());
        repository.set_discovery_enabled(true, 1).unwrap();
        assert!(repository.discovery_enabled().unwrap());

        let directory = repository.add_music_directory("/music", 1).unwrap();
        let mut track = song("local:track");
        track.artist.name = "Daft Punk feat. Pharrell Williams".into();
        repository
            .replace_directory_scan(
                directory.id,
                &[ScannedLocalTrack {
                    song: track.clone(),
                    canonical_path: "/music/track.flac".into(),
                    relative_path: "track.flac".into(),
                    mime_type: "audio/flac".into(),
                    file_size_bytes: 42,
                    modified_at_ms: 2,
                    source_identity: Some("unix:1:1".into()),
                    artist_names: vec!["Daft Punk".into(), "Pharrell Williams".into()],
                    first_seen_at_ms: 2,
                }],
                2,
                0,
                true,
                1,
            )
            .unwrap();

        assert_eq!(
            repository.search_local("track", 10).unwrap(),
            vec![track.clone()]
        );
        let artists = repository.local_artists("", 10, 0).unwrap();
        assert_eq!(artists.len(), 2);
        repository
            .set_local_artist_enabled(artists[0].id, false)
            .unwrap();
        assert_eq!(
            repository.search_local("track", 10).unwrap(),
            vec![track.clone()]
        );
        repository
            .set_local_artist_enabled(artists[1].id, false)
            .unwrap();
        assert!(repository.search_local("track", 10).unwrap().is_empty());

        repository
            .replace_directory_scan(
                directory.id,
                &[ScannedLocalTrack {
                    song: track.clone(),
                    canonical_path: "/music/track.flac".into(),
                    relative_path: "track.flac".into(),
                    mime_type: "audio/flac".into(),
                    file_size_bytes: 42,
                    modified_at_ms: 3,
                    source_identity: Some("unix:1:1".into()),
                    artist_names: vec!["Daft Punk".into(), "Pharrell Williams".into()],
                    first_seen_at_ms: 3,
                }],
                3,
                0,
                true,
                1,
            )
            .unwrap();
        assert!(repository
            .local_artists("", 10, 0)
            .unwrap()
            .iter()
            .all(|artist| !artist.enabled));

        repository.set_reaction(&track, true, false).unwrap();
        repository.remove_music_directory(directory.id).unwrap();
        assert!(repository.search_local("track", 10).unwrap().is_empty());
        let preserved = repository.track_profiles(10).unwrap();
        assert_eq!(preserved.len(), 1);
        assert!(preserved[0].liked);
    }

    #[test]
    fn metadata_edit_same_path_preserves_track_identity_and_reaction() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let directory = repository.add_music_directory("/music", 1).unwrap();
        let original = song("local:original");
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(
                    original.clone(),
                    "/music/track.flac",
                    "unix:1:1",
                    2,
                )],
                2,
                0,
                true,
                1,
            )
            .unwrap();
        repository.set_reaction(&original, true, false).unwrap();

        let mut edited = song("local:changed-fingerprint");
        edited.title = "Edited Metadata".into();
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(edited, "/music/track.flac", "unix:1:1", 3)],
                3,
                0,
                true,
                1,
            )
            .unwrap();

        let stored_track_id: String = repository
            .connection()
            .unwrap()
            .query_row(
                "SELECT track_id FROM local_files WHERE canonical_path = '/music/track.flac'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored_track_id, original.id.as_str());
        let profile = repository
            .track_profiles(10)
            .unwrap()
            .into_iter()
            .find(|item| item.song.id == original.id)
            .unwrap();
        assert!(profile.liked);
        assert_eq!(profile.song.title, "Edited Metadata");
    }

    #[test]
    fn rename_reuses_source_row_and_canonical_track() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let directory = repository.add_music_directory("/music", 1).unwrap();
        let original = song("local:original");
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(original.clone(), "/music/old.flac", "unix:1:9", 2)],
                2,
                0,
                true,
                1,
            )
            .unwrap();
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(
                    song("local:new-fingerprint"),
                    "/music/new.flac",
                    "unix:1:9",
                    3,
                )],
                3,
                0,
                true,
                1,
            )
            .unwrap();

        let connection = repository.connection().unwrap();
        let source: (i64, String, String) = connection
            .query_row(
                "SELECT COUNT(*), canonical_path, track_id FROM local_files",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            source,
            (1, "/music/new.flac".into(), original.id.as_str().into())
        );
    }

    #[test]
    fn partial_scan_does_not_mark_unseen_sources_missing() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let directory = repository.add_music_directory("/music", 1).unwrap();
        let first = song("local:first");
        let second = song("local:second");
        repository
            .replace_directory_scan(
                directory.id,
                &[
                    scanned(first.clone(), "/music/first.flac", "unix:1:1", 2),
                    scanned(second.clone(), "/music/second.flac", "unix:1:2", 2),
                ],
                2,
                0,
                true,
                1,
            )
            .unwrap();
        let report = repository
            .replace_directory_scan(
                directory.id,
                &[scanned(first, "/music/first.flac", "unix:1:1", 3)],
                3,
                1,
                false,
                1,
            )
            .unwrap();

        assert_eq!(report.status, "ERROR");
        let missing: Option<i64> = repository
            .connection()
            .unwrap()
            .query_row(
                "SELECT missing_since_ms FROM local_files WHERE track_id = ?1",
                [second.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, None);
    }

    #[test]
    fn complete_scan_marks_unseen_source_missing() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let directory = repository.add_music_directory("/music", 1).unwrap();
        let track = song("local:removed");
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(track.clone(), "/music/removed.flac", "unix:1:3", 2)],
                2,
                0,
                true,
                1,
            )
            .unwrap();
        let report = repository
            .replace_directory_scan(directory.id, &[], 3, 0, true, 1)
            .unwrap();

        assert_eq!(report.unavailable_tracks, 1);
        assert!(repository.search_local("removed", 10).unwrap().is_empty());
        let retained: bool = repository
            .connection()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM songs WHERE song_id = ?1)",
                [track.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert!(retained);
    }

    #[test]
    fn unavailable_directory_status_preserves_indexed_sources() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let directory = repository.add_music_directory("/music", 1).unwrap();
        let track = song("local:offline-drive");
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(track.clone(), "/music/offline.flac", "unix:1:4", 2)],
                2,
                0,
                true,
                1,
            )
            .unwrap();
        repository
            .set_music_directory_status(directory.id, "UNAVAILABLE", Some("drive disconnected"), 3)
            .unwrap();

        let stored = repository.music_directories().unwrap().pop().unwrap();
        assert_eq!(stored.status, "UNAVAILABLE");
        assert_eq!(stored.track_count, 1);
        let missing: Option<i64> = repository
            .connection()
            .unwrap()
            .query_row(
                "SELECT missing_since_ms FROM local_files WHERE track_id = ?1",
                [track.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, None);
    }

    #[test]
    fn duplicate_playback_source_falls_through_to_readable_copy() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let root =
            std::env::temp_dir().join(format!("solmusic-duplicate-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let missing = root.join("missing.flac");
        let available = root.join("available.flac");
        std::fs::write(&available, b"audio").unwrap();
        let first_directory = repository.add_music_directory("/first", 1).unwrap();
        let second_directory = repository.add_music_directory("/second", 1).unwrap();
        let track = song("local:duplicate");
        repository
            .replace_directory_scan(
                first_directory.id,
                &[scanned(
                    track.clone(),
                    missing.to_str().unwrap(),
                    "unix:1:1",
                    2,
                )],
                2,
                0,
                true,
                1,
            )
            .unwrap();
        repository
            .replace_directory_scan(
                second_directory.id,
                &[scanned(
                    track.clone(),
                    available.to_str().unwrap(),
                    "unix:2:2",
                    2,
                )],
                2,
                0,
                true,
                1,
            )
            .unwrap();

        let selected = repository.local_playback_file(&track.id).unwrap().unwrap();
        assert_eq!(selected.path, available);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unified_library_includes_local_and_enabled_jellyfin_without_discovery() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let directory = repository.add_music_directory("/music", 1).unwrap();
        let local = song("local:library-track");
        repository
            .replace_directory_scan(
                directory.id,
                &[scanned(local.clone(), "/music/local.flac", "unix:1:20", 2)],
                2,
                0,
                true,
                1,
            )
            .unwrap();

        let mut remote = song("jellyfin:1:remote-item");
        remote.album_id = Some("jellyfin-album:1:album".into());
        remote.album_name = Some("Server Album".into());
        repository
            .save_songs(std::slice::from_ref(&remote))
            .unwrap();
        {
            let connection = repository.connection().unwrap();
            connection.execute(
                "INSERT INTO jellyfin_servers
                 (server_internal_id, name, base_url, user_id, username, token_reference, device_id, status)
                 VALUES ('server', 'Home Server', 'https://media.test', 'user', 'listener', 'token', 'device', 'CONNECTED')",
                [],
            ).unwrap();
            connection
                .execute(
                    "INSERT INTO jellyfin_libraries
                 (server_id, library_id, name, collection_type, enabled, track_count)
                 VALUES (1, 'music', 'Music', 'music', 1, 1)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO jellyfin_track_sources
                 (track_id, server_id, library_id, item_id, container, available, last_seen_at_ms)
                 VALUES (?1, 1, 'music', 'remote-item', 'flac', 1, 3)",
                    [remote.id.as_str()],
                )
                .unwrap();
        }

        assert!(!repository.discovery_enabled().unwrap());
        let tracks = repository.library_tracks(10, 0).unwrap();
        assert_eq!(tracks.len(), 2);
        assert!(tracks
            .iter()
            .any(|item| item.source == LibrarySource::Local));
        assert!(tracks
            .iter()
            .any(|item| item.source == LibrarySource::Jellyfin));
        assert_eq!(
            repository.search_local("Server Album", 10).unwrap().len(),
            0
        );
        assert!(repository.has_indexed_source(&remote.id).unwrap());
        assert_eq!(repository.library_albums(10, 0).unwrap().len(), 2);
        assert_eq!(
            repository
                .library_album_tracks("jellyfin-album:1:album")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(repository.library_folders().unwrap().len(), 2);
        assert_eq!(repository.local_track_profiles(10).unwrap().len(), 2);

        repository
            .connection()
            .unwrap()
            .execute("UPDATE jellyfin_libraries SET enabled = 0", [])
            .unwrap();
        assert_eq!(repository.library_tracks(10, 0).unwrap().len(), 1);
        assert!(!repository.has_indexed_source(&remote.id).unwrap());
    }

    #[test]
    fn profile_crud_persists_identity_and_protects_the_last_profile() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let main = repository.active_listening_profile().unwrap();
        assert_eq!(main.name, "Main");

        let created = repository.create_listening_profile("Phonk", 100).unwrap();
        assert_eq!(
            uuid::Uuid::parse_str(&created.id)
                .unwrap()
                .get_version_num(),
            7
        );
        let renamed = repository
            .rename_listening_profile(&created.id, "Late Night")
            .unwrap();
        assert_eq!(renamed.id, created.id);
        assert_eq!(renamed.name, "Late Night");

        repository
            .set_active_listening_profile(&created.id, 200)
            .unwrap();
        assert_eq!(
            repository.active_listening_profile().unwrap().id,
            created.id
        );
        repository.delete_listening_profile(&created.id).unwrap();
        assert_eq!(repository.active_listening_profile().unwrap().id, main.id);
        assert!(repository.delete_listening_profile(&main.id).is_err());
    }

    #[test]
    fn playlist_crud_basics_and_duplicate_add_are_idempotent() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        assert!(repository.playlists().unwrap().is_empty());

        let playlist = repository.create_playlist("  Favorites  ", 100).unwrap();
        assert_eq!(playlist.name, "Favorites");
        assert_eq!(playlist.track_count, 0);
        assert_eq!(
            uuid::Uuid::parse_str(&playlist.id)
                .unwrap()
                .get_version_num(),
            4
        );

        let track = song("song-1");
        let first = repository
            .add_song_to_playlist(&playlist.id, &track, 200)
            .unwrap();
        let duplicate = repository
            .add_song_to_playlist(&playlist.id, &track, 300)
            .unwrap();
        assert_eq!(first, duplicate);
        assert_eq!(
            repository.playlist_tracks(&playlist.id).unwrap(),
            vec![first]
        );

        let listed = repository.playlists().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].track_count, 1);
        assert_eq!(listed[0].updated_at_ms, 200);
        assert!(repository.create_playlist("favorites", 400).is_err());
    }

    #[test]
    fn playlist_tracks_preserve_insertion_order() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let playlist = repository.create_playlist("Road Trip", 100).unwrap();
        for (index, id) in ["third", "first", "second"].into_iter().enumerate() {
            repository
                .add_song_to_playlist(&playlist.id, &song(id), 200 + index as i64)
                .unwrap();
        }

        let tracks = repository.playlist_tracks(&playlist.id).unwrap();
        assert_eq!(
            tracks
                .iter()
                .map(|item| item.song.id.as_str())
                .collect::<Vec<_>>(),
            vec!["third", "first", "second"]
        );
        assert_eq!(
            tracks.iter().map(|item| item.position).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn playlists_are_isolated_by_active_profile() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let main = repository.active_listening_profile().unwrap();
        let main_playlist = repository.create_playlist("Private", 100).unwrap();
        repository
            .add_song_to_playlist(&main_playlist.id, &song("main-song"), 200)
            .unwrap();

        let other = repository.create_listening_profile("Other", 300).unwrap();
        repository
            .set_active_listening_profile(&other.id, 400)
            .unwrap();
        assert!(repository.playlists().unwrap().is_empty());
        assert!(repository.playlist_tracks(&main_playlist.id).is_err());
        assert!(repository
            .add_song_to_playlist(&main_playlist.id, &song("other-song"), 500)
            .is_err());
        let other_playlist = repository.create_playlist("Private", 600).unwrap();
        assert_ne!(other_playlist.id, main_playlist.id);

        repository
            .set_active_listening_profile(&main.id, 700)
            .unwrap();
        assert_eq!(repository.playlists().unwrap()[0].id, main_playlist.id);
        assert_eq!(
            repository.playlist_tracks(&main_playlist.id).unwrap()[0]
                .song
                .id
                .as_str(),
            "main-song"
        );
        assert!(repository.playlist_tracks(&other_playlist.id).is_err());
    }

    #[test]
    fn active_profile_survives_repository_restart() {
        let path = std::env::temp_dir().join(format!(
            "solmusic-profile-restart-{}.sqlite3",
            uuid::Uuid::now_v7()
        ));
        let profile_id = {
            let repository = SqliteMusicRepository::open(&path).unwrap();
            let profile = repository.create_listening_profile("Country", 100).unwrap();
            repository
                .set_active_listening_profile(&profile.id, 200)
                .unwrap();
            profile.id
        };
        let reopened = SqliteMusicRepository::open(&path).unwrap();
        assert_eq!(reopened.active_listening_profile().unwrap().id, profile_id);
        drop(reopened);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite3-shm"));
    }

    #[test]
    fn history_likes_and_affinity_are_isolated_by_profile() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let main = repository.active_listening_profile().unwrap();
        let phonk = repository.create_listening_profile("Phonk", 100).unwrap();
        let track = song("song-shared");

        let mut main_event = summary(
            "main-event",
            track.clone(),
            90_000,
            PlaybackEndReason::Completed,
            10,
        );
        main_event.profile_id = main.id.clone();
        repository.record_playback(&main_event).unwrap();
        repository.set_reaction(&track, true, false).unwrap();
        assert_eq!(
            repository.liked_song_ids().unwrap(),
            vec![track.id.as_str()]
        );
        assert_eq!(repository.track_profiles(10).unwrap()[0].affinity, 5.0);
        assert_eq!(repository.artist_affinities().unwrap()["artist-1"], 3.5);

        repository
            .set_active_listening_profile(&phonk.id, 200)
            .unwrap();
        assert!(repository.recent_songs(10, None).unwrap().is_empty());
        assert!(repository.liked_song_ids().unwrap().is_empty());
        assert!(repository.track_profiles(10).unwrap().is_empty());
        assert!(repository.artist_affinities().unwrap().is_empty());

        let mut phonk_event = summary(
            "phonk-event",
            track.clone(),
            40_000,
            PlaybackEndReason::Stopped,
            20,
        );
        phonk_event.profile_id = phonk.id.clone();
        repository.record_playback(&phonk_event).unwrap();
        let phonk_profile = repository.track_profiles(10).unwrap().pop().unwrap();
        assert_eq!(phonk_profile.affinity, 1.0);
        assert!(!phonk_profile.liked);
        assert_eq!(repository.artist_affinities().unwrap()["artist-1"], 1.0);

        repository
            .set_active_listening_profile(&main.id, 300)
            .unwrap();
        let main_profile = repository.track_profiles(10).unwrap().pop().unwrap();
        assert_eq!(main_profile.affinity, 5.0);
        assert!(main_profile.liked);
        assert_eq!(
            repository.recent_songs(10, None).unwrap()[0].song.id,
            track.id
        );
    }

    #[test]
    fn finalized_event_keeps_its_captured_profile_after_active_profile_changes() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let main = repository.active_listening_profile().unwrap();
        let country = repository.create_listening_profile("Country", 100).unwrap();
        let mut event = summary(
            "split-before-switch",
            song("song-a"),
            90_000,
            PlaybackEndReason::Stopped,
            10,
        );
        event.profile_id = main.id.clone();

        repository
            .set_active_listening_profile(&country.id, 200)
            .unwrap();
        repository.record_playback(&event).unwrap();
        assert!(repository.recent_songs(10, None).unwrap().is_empty());

        repository
            .set_active_listening_profile(&main.id, 300)
            .unwrap();
        assert_eq!(repository.recent_songs(10, None).unwrap().len(), 1);
    }

    #[test]
    fn v5_taste_data_migrates_losslessly_into_main_profile() {
        let connection = Connection::open_in_memory().unwrap();
        for migration in MIGRATIONS.iter().take(5) {
            connection.execute_batch(migration).unwrap();
        }
        connection.pragma_update(None, "user_version", 5).unwrap();
        connection
            .execute(
                "INSERT INTO songs
                 (song_id, title, artist_name, duration_ms) VALUES ('legacy', 'Legacy', 'Artist', 100000)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO track_stats
                 (song_id, play_count, completed_count, completion_ema, completion_samples,
                  affinity, liked, last_played_at_ms)
                 VALUES ('legacy', 7, 5, 0.8, 7, 9.0, 1, 123)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO processed_playback_events (event_id) VALUES ('legacy-event')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO recent_plays
                 (event_id, song_id, started_at_ms, listened_ms, duration_ms, end_reason)
                 VALUES ('legacy-event', 'legacy', 123, 80000, 100000, 'Completed')",
                [],
            )
            .unwrap();

        let repository = SqliteMusicRepository::from_connection(connection).unwrap();
        assert_eq!(repository.active_listening_profile().unwrap().name, "Main");
        let profile = repository.track_profiles(10).unwrap().pop().unwrap();
        assert_eq!(profile.play_count, 7);
        assert_eq!(profile.completed_count, 5);
        assert_eq!(profile.affinity, 9.0);
        assert!(profile.liked);
        assert_eq!(repository.artist_affinities().unwrap()["name:artist"], 9.0);
        assert_eq!(repository.recent_songs(10, None).unwrap().len(), 1);
    }

    #[test]
    fn recent_history_is_bounded_without_weakening_idempotency() {
        let repository = SqliteMusicRepository::open_in_memory().unwrap();
        let track = song("song-1");
        for index in 0..=1_000 {
            repository
                .record_playback(&summary(
                    &format!("event-{index}"),
                    track.clone(),
                    40_000,
                    PlaybackEndReason::Stopped,
                    index,
                ))
                .unwrap();
        }

        let recent_count: i64 = repository
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM recent_plays", [], |row| row.get(0))
            .unwrap();
        assert_eq!(recent_count, 1_000);

        repository
            .record_playback(&summary(
                "event-0",
                track,
                40_000,
                PlaybackEndReason::Stopped,
                2_000,
            ))
            .unwrap();
        let profile = repository.track_profiles(1).unwrap().pop().unwrap();
        assert_eq!(profile.play_count, 1_001);
    }
}
