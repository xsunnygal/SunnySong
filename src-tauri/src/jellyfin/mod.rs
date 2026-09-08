use std::{
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use keyring::Entry;
use reqwest::{header, redirect::Policy, Client, StatusCode, Url};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

const CLIENT_NAME: &str = "SunnySong";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const KEYRING_SERVICE: &str = "app.solmusic.jellyfin";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JellyfinServer {
    pub id: i64,
    pub server_internal_id: String,
    pub name: String,
    pub base_url: String,
    pub username: String,
    pub status: String,
    pub last_connected_at_ms: Option<i64>,
    pub last_sync_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub libraries: Vec<JellyfinLibrary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JellyfinLibrary {
    pub id: String,
    pub name: String,
    pub collection_type: Option<String>,
    pub enabled: bool,
    pub last_sync_at_ms: Option<i64>,
    pub track_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicServerInfo {
    pub name: String,
    pub server_id: String,
    pub version: String,
    pub normalized_url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickConnectSession {
    pub code: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JellyfinRefreshResult {
    pub server: JellyfinServer,
    pub diagnostics: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PublicInfoResponse {
    server_name: String,
    id: String,
    version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticationResponse {
    access_token: String,
    server_id: String,
    user: AuthenticatedUser,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AuthenticatedUser {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ItemsResponse<T> {
    #[serde(default)]
    items: Vec<T>,
    #[serde(default)]
    total_record_count: Option<u64>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ViewItem {
    id: String,
    name: String,
    collection_type: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinArtistItem {
    id: String,
    name: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinAlbumItem {
    id: String,
    #[serde(default)]
    image_tags: std::collections::HashMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct JellyfinAudioItem {
    id: String,
    name: String,
    album: Option<String>,
    album_id: Option<String>,
    album_artist: Option<String>,
    #[serde(default)]
    artists: Vec<String>,
    #[serde(default)]
    artist_items: Vec<JellyfinArtistItem>,
    run_time_ticks: Option<u64>,
    container: Option<String>,
    #[serde(default)]
    image_tags: std::collections::HashMap<String, String>,
    primary_image_item_id: Option<String>,
    album_primary_image_tag: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct QuickConnectResponse {
    code: String,
    secret: String,
    #[serde(default)]
    authenticated: bool,
}

pub struct JellyfinService {
    client: Client,
    ipv4_client: Client,
    database: Mutex<Connection>,
    validated_server: Mutex<Option<(PublicServerInfo, Instant)>>,
}

impl JellyfinService {
    pub fn open(database_path: &std::path::Path) -> Result<Self, String> {
        let connection = Connection::open(database_path).map_err(error)?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .map_err(error)?;
        Ok(Self {
            client: jellyfin_http_client(None)?,
            ipv4_client: jellyfin_http_client(Some(std::net::IpAddr::V4(
                std::net::Ipv4Addr::UNSPECIFIED,
            )))?,
            database: Mutex::new(connection),
            validated_server: Mutex::new(None),
        })
    }

    pub async fn validate_server(&self, address: &str) -> Result<PublicServerInfo, String> {
        let candidates = candidate_base_urls(address)?;
        let mut failures = Vec::new();
        for base_url in &candidates {
            match probe_public_server(&self.client, &self.ipv4_client, base_url).await {
                Ok(info) => {
                    if let Ok(mut cached) = self.validated_server.lock() {
                        *cached = Some((info.clone(), Instant::now()));
                    }
                    info!(
                        category = "JELLYFIN",
                        event = "server_discovered",
                        base_url = %base_url,
                        server_id = %info.server_id,
                        version = %info.version
                    );
                    return Ok(info);
                }
                Err(reason) => {
                    warn!(
                        category = "JELLYFIN",
                        event = "server_probe_failed",
                        base_url = %base_url,
                        reason = %reason
                    );
                    failures.push(format!("{base_url}: {reason}"));
                }
            }
        }

        let attempted = candidates.join(", ");
        let detail = summarize_probe_failures(&failures);
        Err(format!(
            "Could not find a Jellyfin API for this address. Tried {attempted}. {detail} Make sure the phone is on the same network, Jellyfin remote connections are enabled, and use the server's LAN address such as http://192.168.1.20:8096."
        ))
    }

    async fn confirmed_server(&self, address: &str) -> Result<PublicServerInfo, String> {
        let normalized = normalize_base_url(address)?;
        if let Ok(cached) = self.validated_server.lock() {
            if let Some((info, confirmed_at)) = cached.as_ref() {
                if confirmed_at.elapsed() <= Duration::from_secs(5 * 60)
                    && info.normalized_url == normalized
                {
                    return Ok(info.clone());
                }
            }
        }
        self.validate_server(address).await
    }

    pub async fn login_password(
        &self,
        address: &str,
        username: &str,
        password: &str,
    ) -> Result<JellyfinServer, String> {
        let public = self.confirmed_server(address).await?;
        let device_id = self.device_id()?;
        let response = self
            .send(
                self.client
                    .post(format!(
                        "{}/Users/AuthenticateByName",
                        public.normalized_url
                    ))
                    .header("Authorization", authorization(&device_id))
                    .json(&serde_json::json!({ "Username": username.trim(), "Pw": password })),
            )
            .await
            .map_err(|e| format!("Jellyfin sign-in failed: {e}"))?;
        let authentication = authentication_response(response).await?;
        self.finish_login(public, device_id, authentication).await
    }

    pub async fn begin_quick_connect(&self, address: &str) -> Result<QuickConnectSession, String> {
        let public = self.confirmed_server(address).await?;
        let device_id = self.device_id()?;
        let response = self
            .send(
                self.client
                    .post(format!("{}/QuickConnect/Initiate", public.normalized_url))
                    .header("Authorization", authorization(&device_id)),
            )
            .await?;
        if response.status() == StatusCode::NOT_FOUND || response.status() == StatusCode::FORBIDDEN
        {
            return Err(
                "Quick Connect is unavailable on this server. Use username and password instead."
                    .into(),
            );
        }
        let quick: QuickConnectResponse =
            checked_json(response, "Quick Connect initiation").await?;
        Ok(QuickConnectSession {
            code: quick.code,
            secret: quick.secret,
        })
    }

    pub async fn finish_quick_connect(
        &self,
        address: &str,
        secret: &str,
    ) -> Result<Option<JellyfinServer>, String> {
        let public = self.confirmed_server(address).await?;
        let device_id = self.device_id()?;
        let status: QuickConnectResponse = checked_json(
            self.send(
                self.client
                    .get(format!("{}/QuickConnect/Connect", public.normalized_url))
                    .query(&[("secret", secret)])
                    .header("Authorization", authorization(&device_id)),
            )
            .await?,
            "Quick Connect status",
        )
        .await?;
        if !status.authenticated {
            return Ok(None);
        }
        let response = self
            .send(
                self.client
                    .post(format!(
                        "{}/Users/AuthenticateWithQuickConnect",
                        public.normalized_url
                    ))
                    .header("Authorization", authorization(&device_id))
                    .json(&serde_json::json!({ "Secret": secret })),
            )
            .await?;
        let authentication = authentication_response(response).await?;
        self.finish_login(public, device_id, authentication)
            .await
            .map(Some)
    }

    pub fn servers(&self) -> Result<Vec<JellyfinServer>, String> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT server_id, server_internal_id, name, base_url, username, status,
                    last_connected_at_ms, last_sync_at_ms, last_error
             FROM jellyfin_servers ORDER BY name COLLATE NOCASE",
            )
            .map_err(error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .map_err(error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(error)?;
        rows.into_iter()
            .map(|row| {
                let libraries = libraries_for(&connection, row.0)?;
                Ok(JellyfinServer {
                    id: row.0,
                    server_internal_id: row.1,
                    name: row.2,
                    base_url: row.3,
                    username: row.4,
                    status: row.5,
                    last_connected_at_ms: row.6,
                    last_sync_at_ms: row.7,
                    last_error: row.8,
                    libraries,
                })
            })
            .collect()
    }

    pub async fn refresh_server_libraries(
        &self,
        server_id: i64,
    ) -> Result<JellyfinRefreshResult, String> {
        info!(
            category = "JELLYFIN",
            event = "library_refresh_started",
            server_id
        );
        let token_reference: String = self
            .connection()?
            .query_row(
                "SELECT token_reference FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
                |row| row.get(0),
            )
            .map_err(error)?;
        let token = Entry::new(KEYRING_SERVICE, &token_reference)
            .map_err(|e| format!("secure credential storage is unavailable: {e}"))?
            .get_password()
            .map_err(|e| {
                warn!(category = "JELLYFIN", event = "saved_token_unavailable", server_id, token_reference = %token_reference, reason = %e);
                "The saved Jellyfin login is unavailable because the previous build did not persist tokens correctly. Disconnect this server and add it again once.".to_owned()
            })?;
        let diagnostics = match self.refresh_libraries(server_id, &token).await {
            Ok(diagnostics) => diagnostics,
            Err(refresh_error) => {
                self.connection()?.execute(
                    "UPDATE jellyfin_servers SET status = 'OFFLINE', last_error = ?2 WHERE server_id = ?1",
                    params![server_id, refresh_error],
                ).map_err(error)?;
                return Err(refresh_error);
            }
        };
        let server = self
            .servers()?
            .into_iter()
            .find(|server| server.id == server_id)
            .ok_or_else(|| "refreshed Jellyfin server was not found".to_owned())?;
        Ok(JellyfinRefreshResult {
            server,
            diagnostics,
        })
    }

    pub async fn set_library_enabled(
        &self,
        server_id: i64,
        library_id: &str,
        enabled: bool,
    ) -> Result<(), String> {
        let changed = self.connection()?.execute(
            "UPDATE jellyfin_libraries SET enabled = ?3 WHERE server_id = ?1 AND library_id = ?2",
            params![server_id, library_id, enabled],
        ).map_err(error)?;
        if changed == 0 {
            return Err("Jellyfin library was not found".into());
        }
        if enabled {
            let token = self.saved_token(server_id)?;
            self.sync_library_tracks(server_id, library_id, &token)
                .await?;
        }
        Ok(())
    }

    pub fn resolve_playback(
        &self,
        song_id: &solmusic_application::domain::SongId,
    ) -> Result<solmusic_application::PlaybackSource, String> {
        let (server_id, base_url, item_id, container): (i64, String, String, Option<String>) = self
            .connection()?
            .query_row(
                "SELECT js.server_id, js.base_url, jts.item_id, jts.container
                 FROM jellyfin_track_sources jts
                 JOIN jellyfin_libraries jl ON jl.server_id = jts.server_id
                      AND jl.library_id = jts.library_id
                 JOIN jellyfin_servers js ON js.server_id = jts.server_id
                 WHERE jts.track_id = ?1 AND jts.available = 1 AND jl.enabled = 1
                 ORDER BY js.last_connected_at_ms DESC LIMIT 1",
                [song_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(error)?;
        let token = self.saved_token(server_id)?;
        let mut url = Url::parse(&format!("{base_url}/Audio/{item_id}/stream"))
            .map_err(|parse_error| format!("invalid Jellyfin playback URL: {parse_error}"))?;
        url.query_pairs_mut()
            .append_pair("Static", "true")
            .append_pair("api_key", &token);
        let mime_type = match container.as_deref() {
            Some("flac") => "audio/flac",
            Some("mp3") => "audio/mpeg",
            Some("ogg") | Some("oga") | Some("opus") => "audio/ogg",
            Some("wav") => "audio/wav",
            Some("m4a") | Some("mp4") | Some("aac") => "audio/mp4",
            _ => "audio/*",
        };
        Ok(solmusic_application::PlaybackSource {
            url: url.to_string(),
            mime_type: mime_type.into(),
            expires_at_ms: None,
            local_path: None,
            request_profile: solmusic_application::PlaybackRequestProfile::Web,
            normalization_gain_metadata: None,
        })
    }

    pub fn remove_server(&self, server_id: i64) -> Result<(), String> {
        let connection = self.connection()?;
        let token_reference: String = connection
            .query_row(
                "SELECT token_reference FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
                |row| row.get(0),
            )
            .map_err(error)?;
        if let Ok(entry) = Entry::new(KEYRING_SERVICE, &token_reference) {
            if let Err(keyring_error) = entry.delete_credential() {
                warn!(category = "JELLYFIN", event = "credential_delete_failed", reason = %keyring_error);
            }
        }
        connection
            .execute(
                "DELETE FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
            )
            .map_err(error)?;
        Ok(())
    }

    async fn finish_login(
        &self,
        public: PublicServerInfo,
        device_id: String,
        authentication: AuthenticationResponse,
    ) -> Result<JellyfinServer, String> {
        let token_reference = format!("{}:{}", authentication.server_id, authentication.user.id);
        Entry::new(KEYRING_SERVICE, &token_reference)
            .map_err(|e| format!("secure credential storage is unavailable: {e}"))?
            .set_password(&authentication.access_token)
            .map_err(|e| format!("could not save Jellyfin token securely: {e}"))?;
        let now = now_ms();
        let server_id = {
            let connection = self.connection()?;
            connection.execute(
                "INSERT INTO jellyfin_servers
                 (server_internal_id, name, base_url, user_id, username, token_reference, device_id, last_connected_at_ms, status, last_error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'CONNECTED', NULL)
                 ON CONFLICT(base_url) DO UPDATE SET server_internal_id = excluded.server_internal_id,
                    name = excluded.name, user_id = excluded.user_id, username = excluded.username,
                    token_reference = excluded.token_reference, device_id = excluded.device_id,
                    last_connected_at_ms = excluded.last_connected_at_ms, status = 'CONNECTED', last_error = NULL",
                params![authentication.server_id, public.name, public.normalized_url, authentication.user.id, authentication.user.name, token_reference, device_id, now],
            ).map_err(error)?;
            connection
                .query_row(
                    "SELECT server_id FROM jellyfin_servers WHERE base_url = ?1",
                    [&public.normalized_url],
                    |row| row.get(0),
                )
                .map_err(error)?
        };
        self.refresh_libraries(server_id, &authentication.access_token)
            .await?;
        info!(category = "JELLYFIN", event = "server_connected", server_id, server_internal_id = %authentication.server_id);
        self.servers()?
            .into_iter()
            .find(|server| server.id == server_id)
            .ok_or_else(|| "connected Jellyfin server was not found".into())
    }

    async fn refresh_libraries(&self, server_id: i64, token: &str) -> Result<Vec<String>, String> {
        let (base_url, user_id, device_id) = self
            .connection()?
            .query_row(
                "SELECT base_url, user_id, device_id FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(error)?;
        let response = self
            .send(
                self.client
                    .get(format!("{base_url}/Users/{user_id}/Views"))
                    .header("Authorization", authorization(&device_id))
                    .header("X-Emby-Token", token),
            )
            .await?;
        let views: ItemsResponse<ViewItem> =
            checked_json(response, "Jellyfin library listing").await?;

        let mut audio_libraries = Vec::new();
        let mut diagnostics = Vec::new();
        for view in views.items {
            let endpoint = format!("{base_url}/Items");
            let common = [
                ("UserId", user_id.as_str()),
                ("ParentId", view.id.as_str()),
                ("Recursive", "true"),
                ("EnableTotalRecordCount", "true"),
            ];
            let sample_response = self
                .send(
                    self.client
                        .get(&endpoint)
                        .header("Authorization", authorization(&device_id))
                        .header("X-Emby-Token", token)
                        .query(&common)
                        .query(&[("Limit", "100")]),
                )
                .await?;
            let sample: ItemsResponse<serde_json::Value> = checked_json(
                sample_response,
                &format!("content scan for Jellyfin library {}", view.name),
            )
            .await?;
            let sample_summary = summarize_jellyfin_items(&sample.items);
            let total_items = sample
                .total_record_count
                .unwrap_or(sample.items.len() as u64);

            let audio_response = self
                .send(
                    self.client
                        .get(&endpoint)
                        .header("Authorization", authorization(&device_id))
                        .header("X-Emby-Token", token)
                        .query(&common)
                        .query(&[("IncludeItemTypes", "Audio"), ("Limit", "1")]),
                )
                .await?;
            let audio: ItemsResponse<serde_json::Value> = checked_json(
                audio_response,
                &format!("audio item scan for Jellyfin library {}", view.name),
            )
            .await?;
            let mut track_count = audio.total_record_count.unwrap_or(audio.items.len() as u64);
            if track_count == 0 {
                let media_response = self
                    .send(
                        self.client
                            .get(&endpoint)
                            .header("Authorization", authorization(&device_id))
                            .header("X-Emby-Token", token)
                            .query(&common)
                            .query(&[("MediaTypes", "Audio"), ("Limit", "1")]),
                    )
                    .await?;
                let media_audio: ItemsResponse<serde_json::Value> = checked_json(
                    media_response,
                    &format!("audio media scan for Jellyfin library {}", view.name),
                )
                .await?;
                track_count = media_audio
                    .total_record_count
                    .unwrap_or(media_audio.items.len() as u64);
            }
            diagnostics.push(format!(
                "{}: {} recursive items, {} audio; {}",
                view.name, total_items, track_count, sample_summary
            ));
            info!(
                category = "JELLYFIN",
                event = "library_content_diagnostic",
                server_id,
                library_id = %view.id,
                library_name = %view.name,
                collection_type = ?view.collection_type,
                recursive_item_count = total_items,
                recursive_audio_count = track_count,
                sample = %sample_summary
            );
            if track_count > 0 {
                audio_libraries.push((view, track_count));
            }
        }

        let now = now_ms();
        {
            let mut connection = self.connection()?;
            let transaction = connection.transaction().map_err(error)?;
            transaction
                .execute(
                    "UPDATE jellyfin_libraries SET track_count = 0 WHERE server_id = ?1",
                    [server_id],
                )
                .map_err(error)?;
            for (view, track_count) in &audio_libraries {
                transaction.execute(
                "INSERT INTO jellyfin_libraries
                 (server_id, library_id, name, collection_type, enabled, last_sync_at_ms, track_count)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6)
                 ON CONFLICT(server_id, library_id) DO UPDATE SET
                    name = excluded.name,
                    collection_type = excluded.collection_type,
                    last_sync_at_ms = excluded.last_sync_at_ms,
                    track_count = excluded.track_count",
                params![server_id, view.id, view.name, view.collection_type, now, track_count],
            ).map_err(error)?;
            }
            transaction
                .execute(
                    "UPDATE jellyfin_servers SET status = 'CONNECTED', last_error = NULL,
                 last_connected_at_ms = ?2 WHERE server_id = ?1",
                    params![server_id, now],
                )
                .map_err(error)?;
            transaction.commit().map_err(error)?;
        }

        let enabled_libraries = {
            let connection = self.connection()?;
            let mut statement = connection
                .prepare(
                    "SELECT library_id, name FROM jellyfin_libraries
                     WHERE server_id = ?1 AND enabled = 1 AND track_count > 0",
                )
                .map_err(error)?;
            let rows = statement
                .query_map([server_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(error)?;
            rows
        };
        for (library_id, library_name) in enabled_libraries {
            let indexed = self
                .sync_library_tracks(server_id, &library_id, token)
                .await?;
            diagnostics.push(format!("{library_name}: indexed {indexed} playable tracks"));
        }

        info!(
            category = "JELLYFIN",
            event = "audio_libraries_refreshed",
            server_id,
            audio_library_count = audio_libraries.len()
        );
        Ok(diagnostics)
    }

    async fn sync_library_tracks(
        &self,
        server_id: i64,
        library_id: &str,
        token: &str,
    ) -> Result<usize, String> {
        let (base_url, user_id, device_id) = self
            .connection()?
            .query_row(
                "SELECT base_url, user_id, device_id FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(error)?;
        let page_size = 200usize;
        let mut album_images = std::collections::HashMap::<String, String>::new();
        let mut album_start_index = 0usize;
        loop {
            let response = self
                .send(
                    self.client
                        .get(format!("{base_url}/Items"))
                        .header("Authorization", authorization(&device_id))
                        .header("X-Emby-Token", token)
                        .query(&[
                            ("UserId", user_id.as_str()),
                            ("ParentId", library_id),
                            ("Recursive", "true"),
                            ("IncludeItemTypes", "MusicAlbum"),
                            ("EnableTotalRecordCount", "true"),
                            ("EnableImages", "true"),
                            ("ImageTypeLimit", "1"),
                            ("EnableImageTypes", "Primary"),
                        ])
                        .query(&[("StartIndex", album_start_index), ("Limit", page_size)]),
                )
                .await?;
            let page: ItemsResponse<JellyfinAlbumItem> =
                checked_json(response, "Jellyfin album artwork synchronization").await?;
            let received = page.items.len();
            for album in page.items {
                if let Some(tag) = album
                    .image_tags
                    .get("Primary")
                    .filter(|tag| !tag.is_empty())
                {
                    album_images.insert(album.id, tag.clone());
                }
            }
            album_start_index += received;
            if received == 0
                || received < page_size
                || page
                    .total_record_count
                    .is_some_and(|total| album_start_index as u64 >= total)
            {
                break;
            }
        }

        let mut items = Vec::new();
        let mut start_index = 0usize;
        loop {
            let response = self
                .send(
                    self.client
                        .get(format!("{base_url}/Items"))
                        .header("Authorization", authorization(&device_id))
                        .header("X-Emby-Token", token)
                        .query(&[
                            ("UserId", user_id.as_str()),
                            ("ParentId", library_id),
                            ("Recursive", "true"),
                            ("IncludeItemTypes", "Audio"),
                            ("EnableTotalRecordCount", "true"),
                            ("EnableImages", "true"),
                            ("ImageTypeLimit", "1"),
                            ("EnableImageTypes", "Primary"),
                            (
                                "Fields",
                                "Album,AlbumId,AlbumArtist,Artists,RunTimeTicks,Container,ImageTags,PrimaryImageItemId,AlbumPrimaryImageTag",
                            ),
                        ])
                        .query(&[("StartIndex", start_index), ("Limit", page_size)]),
                )
                .await?;
            let page: ItemsResponse<JellyfinAudioItem> =
                checked_json(response, "Jellyfin music synchronization").await?;
            let received = page.items.len();
            items.extend(page.items);
            start_index += received;
            if received == 0
                || received < page_size
                || page
                    .total_record_count
                    .is_some_and(|total| start_index as u64 >= total)
            {
                break;
            }
        }

        let now = now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(error)?;
        transaction
            .execute(
                "UPDATE jellyfin_track_sources SET available = 0
                 WHERE server_id = ?1 AND library_id = ?2",
                params![server_id, library_id],
            )
            .map_err(error)?;
        for item in &items {
            if item.id.trim().is_empty() || item.name.trim().is_empty() {
                continue;
            }
            let track_id = format!("jellyfin:{server_id}:{}", item.id);
            let artist_name = item
                .album_artist
                .as_deref()
                .filter(|name| !name.trim().is_empty())
                .or_else(|| item.artists.first().map(String::as_str))
                .or_else(|| item.artist_items.first().map(|artist| artist.name.as_str()))
                .unwrap_or("Unknown Artist");
            let artist_id = item
                .artist_items
                .first()
                .filter(|artist| !artist.id.is_empty())
                .map(|artist| format!("jellyfin-artist:{server_id}:{}", artist.id));
            let album_id = item
                .album_id
                .as_deref()
                .filter(|id| !id.is_empty())
                .map(|id| format!("jellyfin-album:{server_id}:{id}"));
            let duration_ms = item.run_time_ticks.map(|ticks| ticks / 10_000);
            let thumbnail_url = jellyfin_primary_image_url(server_id, item, &album_images);
            transaction
                .execute(
                    "INSERT INTO songs
                     (song_id, title, artist_id, artist_name, album_id, album_name, duration_ms, thumbnail_url)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(song_id) DO UPDATE SET
                        title = excluded.title, artist_id = excluded.artist_id,
                        artist_name = excluded.artist_name, album_id = excluded.album_id,
                        album_name = excluded.album_name, duration_ms = excluded.duration_ms,
                        thumbnail_url = COALESCE(excluded.thumbnail_url, songs.thumbnail_url)",
                    params![
                        track_id,
                        item.name,
                        artist_id,
                        artist_name,
                        album_id,
                        item.album,
                        duration_ms,
                        thumbnail_url
                    ],
                )
                .map_err(error)?;
            transaction
                .execute(
                    "INSERT INTO jellyfin_track_sources
                     (track_id, server_id, library_id, item_id, container, available, last_seen_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
                     ON CONFLICT(server_id, item_id) DO UPDATE SET
                        track_id = excluded.track_id, library_id = excluded.library_id,
                        container = excluded.container, available = 1,
                        last_seen_at_ms = excluded.last_seen_at_ms",
                    params![track_id, server_id, library_id, item.id, item.container, now],
                )
                .map_err(error)?;
        }
        transaction
            .execute(
                "UPDATE songs AS local
                 SET thumbnail_url = (
                    SELECT remote.thumbnail_url
                    FROM songs remote
                    JOIN jellyfin_track_sources source ON source.track_id = remote.song_id
                    WHERE source.available = 1 AND remote.thumbnail_url IS NOT NULL
                      AND remote.album_name = local.album_name COLLATE NOCASE
                      AND remote.artist_name = local.artist_name COLLATE NOCASE
                    ORDER BY source.last_seen_at_ms DESC LIMIT 1
                 )
                 WHERE local.song_id LIKE 'local:%' AND local.thumbnail_url IS NULL
                   AND local.album_name IS NOT NULL
                   AND EXISTS (
                    SELECT 1 FROM songs remote
                    JOIN jellyfin_track_sources source ON source.track_id = remote.song_id
                    WHERE source.available = 1 AND remote.thumbnail_url IS NOT NULL
                      AND remote.album_name = local.album_name COLLATE NOCASE
                      AND remote.artist_name = local.artist_name COLLATE NOCASE
                 )",
                [],
            )
            .map_err(error)?;
        let indexed_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM jellyfin_track_sources
                 WHERE server_id = ?1 AND library_id = ?2 AND available = 1",
                params![server_id, library_id],
                |row| row.get(0),
            )
            .map_err(error)?;
        transaction
            .execute(
                "UPDATE jellyfin_libraries SET track_count = ?3, last_sync_at_ms = ?4
                 WHERE server_id = ?1 AND library_id = ?2",
                params![server_id, library_id, indexed_count, now],
            )
            .map_err(error)?;
        transaction
            .execute(
                "UPDATE jellyfin_servers SET last_sync_at_ms = ?2, status = 'CONNECTED', last_error = NULL
                 WHERE server_id = ?1",
                params![server_id, now],
            )
            .map_err(error)?;
        transaction.commit().map_err(error)?;
        info!(
            category = "JELLYFIN",
            event = "library_tracks_synchronized",
            server_id,
            library_id,
            indexed_count
        );
        Ok(indexed_count as usize)
    }

    pub fn resolve_artwork_url(&self, marker: &str) -> Result<Option<String>, String> {
        let Some(value) = marker.strip_prefix("jellyfin-artwork:") else {
            return Ok(None);
        };
        let mut parts = value.splitn(3, ':');
        let server_id = parts
            .next()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or("invalid Jellyfin artwork server ID")?;
        let item_id = parts
            .next()
            .filter(|value| !value.is_empty())
            .ok_or("invalid Jellyfin artwork item ID")?;
        let tag = parts.next().unwrap_or_default();
        let base_url: String = self
            .connection()?
            .query_row(
                "SELECT base_url FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
                |row| row.get(0),
            )
            .map_err(error)?;
        let token = self.saved_token(server_id)?;
        let mut url = Url::parse(&format!(
            "{}/Items/{item_id}/Images/Primary",
            base_url.trim_end_matches('/')
        ))
        .map_err(error)?;
        url.query_pairs_mut()
            .append_pair("maxWidth", "512")
            .append_pair("quality", "90")
            .append_pair("api_key", &token);
        if !tag.is_empty() {
            url.query_pairs_mut().append_pair("tag", tag);
        }
        Ok(Some(url.to_string()))
    }

    fn saved_token(&self, server_id: i64) -> Result<String, String> {
        let token_reference: String = self
            .connection()?
            .query_row(
                "SELECT token_reference FROM jellyfin_servers WHERE server_id = ?1",
                [server_id],
                |row| row.get(0),
            )
            .map_err(error)?;
        Entry::new(KEYRING_SERVICE, &token_reference)
            .map_err(|e| format!("secure credential storage is unavailable: {e}"))?
            .get_password()
            .map_err(|e| format!("saved Jellyfin login is unavailable: {e}"))
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, String> {
        let request = request
            .build()
            .map_err(|build_error| format!("could not build Jellyfin request: {build_error}"))?;
        execute_resilient(&self.client, &self.ipv4_client, request).await
    }

    fn device_id(&self) -> Result<String, String> {
        self.connection()?
            .query_row(
                "SELECT device_id FROM jellyfin_installation WHERE singleton_id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(error)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, String> {
        self.database
            .lock()
            .map_err(|_| "Jellyfin database mutex was poisoned".into())
    }
}

fn jellyfin_primary_image_url(
    server_id: i64,
    item: &JellyfinAudioItem,
    album_images: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let album_id = item.album_id.as_deref().filter(|id| !id.is_empty());
    let synchronized_album_tag = album_id.and_then(|id| album_images.get(id).map(String::as_str));
    let inherited_album_tag = item
        .album_primary_image_tag
        .as_deref()
        .filter(|tag| !tag.is_empty());
    let (image_item_id, tag) = if let (Some(album_id), Some(tag)) =
        (album_id, synchronized_album_tag.or(inherited_album_tag))
    {
        (album_id, tag)
    } else if let Some(tag) = item.image_tags.get("Primary").filter(|tag| !tag.is_empty()) {
        (item.id.as_str(), tag.as_str())
    } else {
        let image_item_id = item
            .primary_image_item_id
            .as_deref()
            .filter(|id| !id.is_empty())?;
        (image_item_id, "")
    };
    Some(format!(
        "jellyfin-artwork:{server_id}:{image_item_id}:{tag}"
    ))
}

fn libraries_for(connection: &Connection, server_id: i64) -> Result<Vec<JellyfinLibrary>, String> {
    let mut statement = connection
        .prepare(
            "SELECT library_id, name, collection_type, enabled, last_sync_at_ms, track_count
         FROM jellyfin_libraries
         WHERE server_id = ?1 AND track_count > 0
         ORDER BY name COLLATE NOCASE",
        )
        .map_err(error)?;
    let libraries = statement
        .query_map([server_id], |row| {
            Ok(JellyfinLibrary {
                id: row.get(0)?,
                name: row.get(1)?,
                collection_type: row.get(2)?,
                enabled: row.get(3)?,
                last_sync_at_ms: row.get(4)?,
                track_count: row.get(5)?,
            })
        })
        .map_err(error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(error)?;
    Ok(libraries)
}

fn summarize_jellyfin_items(items: &[serde_json::Value]) -> String {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for item in items {
        let item_type = item
            .get("Type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("UnknownType");
        let media_type = item
            .get("MediaType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("UnknownMedia");
        let container = item
            .get("Container")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown-container");
        *counts
            .entry(format!("{item_type}/{media_type}/{container}"))
            .or_default() += 1;
    }
    if counts.is_empty() {
        "no recursive items returned".into()
    } else {
        counts
            .into_iter()
            .map(|(kind, count)| format!("{kind}={count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

async fn authentication_response(
    response: reqwest::Response,
) -> Result<AuthenticationResponse, String> {
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err("Jellyfin rejected the username or password".into());
    }
    checked_json(response, "Jellyfin authentication").await
}

async fn checked_json<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    operation: &str,
) -> Result<T, String> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "{operation} failed with {status}: {}",
            body.chars().take(240).collect::<String>()
        ));
    }
    response
        .json()
        .await
        .map_err(|e| format!("{operation} returned invalid data: {e}"))
}

fn jellyfin_http_client(local_address: Option<std::net::IpAddr>) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(std::time::Duration::from_secs(7))
        .timeout(std::time::Duration::from_secs(15))
        .redirect(Policy::limited(5))
        .user_agent(format!("SunnySong/{CLIENT_VERSION}"))
        .local_address(local_address)
        .build()
        .map_err(error)
}

async fn execute_resilient(
    client: &Client,
    ipv4_client: &Client,
    request: reqwest::Request,
) -> Result<reqwest::Response, String> {
    let ipv4_request = request
        .try_clone()
        .ok_or_else(|| "Jellyfin request body could not be retried".to_owned())?;
    match ipv4_client.execute(ipv4_request).await {
        Ok(response) => Ok(response),
        Err(ipv4_error) => client.execute(request).await.map_err(|normal_error| {
            let ipv4_reason = describe_request_error(&ipv4_error);
            let normal_reason = describe_request_error(&normal_error);
            if ipv4_reason == normal_reason {
                ipv4_reason
            } else {
                format!("{ipv4_reason}; {normal_reason}")
            }
        }),
    }
}

async fn probe_public_server(
    client: &Client,
    ipv4_client: &Client,
    base_url: &str,
) -> Result<PublicServerInfo, String> {
    let endpoint = format!("{base_url}/System/Info/Public");
    let request = client
        .get(&endpoint)
        .header(header::ACCEPT, "application/json")
        .build()
        .map_err(|build_error| format!("could not build probe request: {build_error}"))?;
    let response = execute_resilient(client, ipv4_client, request).await?;
    let status = response.status();
    let effective_url = response.url().clone();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response
        .text()
        .await
        .map_err(|read_error| format!("could not read the response: {read_error}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let info: PublicInfoResponse = serde_json::from_str(&body).map_err(|_| {
        if content_type.contains("text/html") || body.trim_start().starts_with('<') {
            "the address returned a web page instead of the Jellyfin API".to_owned()
        } else {
            "the response was not Jellyfin public server information".to_owned()
        }
    })?;
    if info.id.trim().is_empty() {
        return Err("the server did not provide a stable Jellyfin ID".into());
    }
    Ok(PublicServerInfo {
        name: info.server_name,
        server_id: info.id,
        version: info.version,
        normalized_url: base_url_from_public_endpoint(&effective_url)
            .unwrap_or_else(|| base_url.to_owned()),
    })
}

fn candidate_base_urls(address: &str) -> Result<Vec<String>, String> {
    let entered = address.trim();
    if entered.is_empty() {
        return Err("Enter a Jellyfin server address".into());
    }
    let has_explicit_scheme = entered.contains("://");
    let normalized = normalize_base_url(entered)?;
    let parsed = Url::parse(&normalized).map_err(|_| "Enter a valid Jellyfin server address")?;
    let mut candidates = Vec::new();
    push_unique(&mut candidates, normalized);

    if parsed.port().is_none() {
        if has_explicit_scheme {
            let default_port = match parsed.scheme() {
                "http" => Some(8096),
                "https" => Some(8920),
                _ => None,
            };
            if let Some(port) = default_port {
                let mut with_port = parsed.clone();
                if with_port.set_port(Some(port)).is_ok() {
                    push_unique(
                        &mut candidates,
                        with_port.as_str().trim_end_matches('/').to_owned(),
                    );
                }
            }
        } else {
            for (scheme, port) in [("http", Some(8096)), ("https", Some(8920)), ("https", None)] {
                let mut alternative = parsed.clone();
                alternative
                    .set_scheme(scheme)
                    .map_err(|_| "Enter a valid Jellyfin server address")?;
                alternative
                    .set_port(port)
                    .map_err(|_| "Enter a valid Jellyfin server address")?;
                push_unique(
                    &mut candidates,
                    alternative.as_str().trim_end_matches('/').to_owned(),
                );
            }
        }
    }

    let base_candidates = candidates.clone();
    for candidate in base_candidates {
        let mut url = Url::parse(&candidate).map_err(error)?;
        if url.path() == "/" || url.path().is_empty() {
            url.set_path("/jellyfin");
            push_unique(
                &mut candidates,
                url.as_str().trim_end_matches('/').to_owned(),
            );
        }
    }
    Ok(candidates)
}

fn push_unique(candidates: &mut Vec<String>, candidate: String) {
    if !candidates.iter().any(|existing| existing == &candidate) {
        candidates.push(candidate);
    }
}

fn base_url_from_public_endpoint(endpoint: &Url) -> Option<String> {
    let suffix = "/System/Info/Public";
    let path = endpoint.path();
    if path.len() < suffix.len() || !path[path.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    {
        return None;
    }
    let mut base = endpoint.clone();
    let base_path = &path[..path.len() - suffix.len()];
    base.set_path(if base_path.is_empty() { "/" } else { base_path });
    base.set_query(None);
    base.set_fragment(None);
    Some(base.as_str().trim_end_matches('/').to_owned())
}

fn describe_request_error(request_error: &reqwest::Error) -> String {
    let text = request_error.to_string().to_ascii_lowercase();
    if request_error.is_timeout() {
        "connection timed out".into()
    } else if text.contains("certificate") || text.contains("tls") {
        "TLS certificate validation failed; use HTTP on your LAN or install a trusted certificate"
            .into()
    } else if text.contains("dns") || text.contains("name or service") {
        "the server name could not be resolved; try its LAN IP address".into()
    } else if text.contains("connection refused") {
        "connection was refused; check the Jellyfin port and firewall".into()
    } else if request_error.is_connect() {
        "could not open a network connection".into()
    } else {
        format!("network request failed: {request_error}")
    }
}

fn summarize_probe_failures(failures: &[String]) -> String {
    if failures.is_empty() {
        return "No connection attempt completed.".into();
    }
    let mut unique_reasons = Vec::new();
    for failure in failures {
        let reason = failure
            .split_once(": ")
            .map_or(failure.as_str(), |(_, reason)| reason);
        if !unique_reasons.contains(&reason) {
            unique_reasons.push(reason);
        }
    }
    format!("Results: {}.", unique_reasons.join("; "))
}

fn normalize_base_url(address: &str) -> Result<String, String> {
    let address = address.trim();
    let candidate = if address.contains("://") {
        address.to_owned()
    } else {
        format!("http://{address}")
    };
    let mut url = Url::parse(&candidate).map_err(|_| "Enter a valid Jellyfin server address")?;
    if url.scheme() != "https" && url.scheme() != "http" {
        return Err("Jellyfin URL must use HTTP or HTTPS".into());
    }
    if url.scheme() == "http" && !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
    {
        warn!(category = "JELLYFIN", event = "insecure_server_url", url = %url);
    }
    url.set_query(None);
    url.set_fragment(None);
    let path = url.path().trim_end_matches('/').to_owned();
    let lowercase = path.to_ascii_lowercase();
    let base_path = if lowercase.ends_with("/web/index.html") {
        path[..path.len() - "/web/index.html".len()].to_owned()
    } else if lowercase.ends_with("/web") {
        path[..path.len() - "/web".len()].to_owned()
    } else {
        path
    };
    url.set_path(if base_path.is_empty() {
        "/"
    } else {
        &base_path
    });
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

fn authorization(device_id: &str) -> String {
    format!("MediaBrowser Client=\"{CLIENT_NAME}\", Device=\"SunnySong Device\", DeviceId=\"{device_id}\", Version=\"{CLIENT_VERSION}\"")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn error(value: impl std::fmt::Display) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use axum::{routing::get, Json, Router};

    use super::{
        candidate_base_urls, jellyfin_http_client, normalize_base_url, probe_public_server,
        JellyfinService,
    };

    #[test]
    fn normalizes_server_url_without_paths_being_exposed() {
        assert_eq!(
            normalize_base_url("https://music.example.com/").unwrap(),
            "https://music.example.com"
        );
        assert_eq!(
            normalize_base_url("https://music.example.com/web/index.html#!/home.html").unwrap(),
            "https://music.example.com"
        );
        assert_eq!(
            normalize_base_url("https://example.com/jellyfin/web/").unwrap(),
            "https://example.com/jellyfin"
        );
        assert_eq!(
            normalize_base_url("192.168.1.20:8096").unwrap(),
            "http://192.168.1.20:8096"
        );
    }

    #[test]
    fn infers_standard_jellyfin_ports_and_base_path() {
        let candidates = candidate_base_urls("jellyfin.local").unwrap();
        assert!(candidates.contains(&"http://jellyfin.local:8096".to_owned()));
        assert!(candidates.contains(&"https://jellyfin.local:8920".to_owned()));
        assert!(candidates.contains(&"https://jellyfin.local".to_owned()));
        assert!(candidates.contains(&"http://jellyfin.local:8096/jellyfin".to_owned()));
    }

    #[test]
    fn explicit_https_address_is_never_downgraded_to_http() {
        let candidates = candidate_base_urls("https://music.example.com").unwrap();
        assert!(candidates.contains(&"https://music.example.com".to_owned()));
        assert!(candidates.contains(&"https://music.example.com:8920".to_owned()));
        assert!(candidates
            .iter()
            .all(|candidate| candidate.starts_with("https://")));
    }

    #[test]
    fn probes_and_parses_the_public_system_info_endpoint() {
        tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new().route(
                        "/System/Info/Public",
                        get(|| async {
                            Json(serde_json::json!({
                                "ServerName": "Test Jellyfin",
                                "Id": "server-id",
                                "Version": "10.10.7"
                            }))
                        }),
                    ),
                )
                .await
                .unwrap();
            });
            let base_url = format!("http://{address}");
            let client = reqwest::Client::new();
            let info = probe_public_server(&client, &client, &base_url)
                .await
                .unwrap();
            assert_eq!(info.name, "Test Jellyfin");
            assert_eq!(info.server_id, "server-id");
            assert_eq!(info.normalized_url, base_url);
            server.abort();
        });
    }

    #[test]
    fn confirmed_server_is_reused_for_the_follow_up_login_flow() {
        tauri::async_runtime::block_on(async {
            let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let route_requests = requests.clone();
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new().route(
                        "/System/Info/Public",
                        get(move || {
                            let requests = route_requests.clone();
                            async move {
                                requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                Json(serde_json::json!({
                                    "ServerName": "Cached Jellyfin",
                                    "Id": "cached-server",
                                    "Version": "10.11.11"
                                }))
                            }
                        }),
                    ),
                )
                .await
                .unwrap();
            });
            let service = JellyfinService {
                client: jellyfin_http_client(None).unwrap(),
                ipv4_client: jellyfin_http_client(Some(std::net::IpAddr::V4(
                    std::net::Ipv4Addr::UNSPECIFIED,
                )))
                .unwrap(),
                database: std::sync::Mutex::new(rusqlite::Connection::open_in_memory().unwrap()),
                validated_server: std::sync::Mutex::new(None),
            };
            let base_url = format!("http://{address}");
            service.validate_server(&base_url).await.unwrap();
            server.abort();
            let cached = service.confirmed_server(&base_url).await.unwrap();
            assert_eq!(cached.server_id, "cached-server");
            assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn synchronizes_paged_audio_items_into_the_indexed_library() {
        tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new().route(
                        "/Items",
                        get(
                            |axum::extract::Query(query): axum::extract::Query<
                                std::collections::HashMap<String, String>,
                            >| async move {
                                if query.get("IncludeItemTypes").map(String::as_str)
                                    == Some("MusicAlbum")
                                {
                                    return Json(serde_json::json!({
                                        "Items": [{
                                            "Id": "album-1",
                                            "ImageTags": { "Primary": "cover-tag" }
                                        }],
                                        "TotalRecordCount": 1
                                    }));
                                }
                                Json(serde_json::json!({
                                    "Items": [
                                        {
                                            "Id": "track-1",
                                            "Name": "First Track",
                                            "Album": "Test Album",
                                            "AlbumId": "album-1",
                                            "AlbumArtist": "Test Artist",
                                            "ArtistItems": [{ "Id": "artist-1", "Name": "Test Artist" }],
                                            "RunTimeTicks": 1800000000_u64,
                                            "Container": "flac"
                                        },
                                        {
                                            "Id": "track-2",
                                            "Name": "Second Track",
                                            "Artists": ["Other Artist"],
                                            "RunTimeTicks": 1200000000_u64,
                                            "Container": "mp3"
                                        }
                                    ],
                                    "TotalRecordCount": 2
                                }))
                            },
                        ),
                    ),
                )
                .await
                .unwrap();
            });

            let connection = rusqlite::Connection::open_in_memory().unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE songs (
                    song_id TEXT PRIMARY KEY, title TEXT NOT NULL, artist_id TEXT,
                    artist_name TEXT NOT NULL, album_id TEXT, album_name TEXT,
                    duration_ms INTEGER, thumbnail_url TEXT
                 );
                 CREATE TABLE jellyfin_servers (
                    server_id INTEGER PRIMARY KEY, base_url TEXT NOT NULL, user_id TEXT NOT NULL,
                    device_id TEXT NOT NULL, last_sync_at_ms INTEGER, status TEXT, last_error TEXT
                 );
                 CREATE TABLE jellyfin_libraries (
                    server_id INTEGER NOT NULL, library_id TEXT NOT NULL, track_count INTEGER,
                    last_sync_at_ms INTEGER, enabled INTEGER, PRIMARY KEY(server_id, library_id)
                 );
                 CREATE TABLE jellyfin_track_sources (
                    track_id TEXT NOT NULL, server_id INTEGER NOT NULL, library_id TEXT NOT NULL,
                    item_id TEXT NOT NULL, container TEXT, available INTEGER,
                    remote_updated_at_ms INTEGER, last_seen_at_ms INTEGER,
                    PRIMARY KEY(server_id, item_id)
                 );",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO jellyfin_servers
                 (server_id, base_url, user_id, device_id, status)
                 VALUES (1, ?1, 'user', 'device', 'CONNECTED')",
                    [format!("http://{address}")],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO jellyfin_libraries
                 (server_id, library_id, track_count, enabled) VALUES (1, 'music', 0, 1)",
                    [],
                )
                .unwrap();
            let service = JellyfinService {
                client: jellyfin_http_client(None).unwrap(),
                ipv4_client: jellyfin_http_client(Some(std::net::IpAddr::V4(
                    std::net::Ipv4Addr::UNSPECIFIED,
                )))
                .unwrap(),
                database: std::sync::Mutex::new(connection),
                validated_server: std::sync::Mutex::new(None),
            };

            assert_eq!(
                service
                    .sync_library_tracks(1, "music", "secret")
                    .await
                    .unwrap(),
                2
            );
            let connection = service.connection().unwrap();
            let song_count: i64 = connection
                .query_row("SELECT COUNT(*) FROM songs", [], |row| row.get(0))
                .unwrap();
            let source_count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM jellyfin_track_sources WHERE available = 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let (duration, artwork): (i64, String) = connection
                .query_row(
                    "SELECT duration_ms, thumbnail_url FROM songs WHERE song_id = 'jellyfin:1:track-1'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(song_count, 2);
            assert_eq!(source_count, 2);
            assert_eq!(duration, 180_000);
            assert_eq!(artwork, "jellyfin-artwork:1:album-1:cover-tag");
            server.abort();
        });
    }

    #[test]
    #[ignore = "live Jellyfin integration test; set SUNNYSONG_LIVE_JELLYFIN_URL"]
    fn live_public_jellyfin_endpoint_returns_server_identity() {
        tauri::async_runtime::block_on(async {
            let base_url = std::env::var("SUNNYSONG_LIVE_JELLYFIN_URL")
                .expect("set SUNNYSONG_LIVE_JELLYFIN_URL to a server base URL");
            let client = super::jellyfin_http_client(None).unwrap();
            let ipv4_client = super::jellyfin_http_client(Some(std::net::IpAddr::V4(
                std::net::Ipv4Addr::UNSPECIFIED,
            )))
            .unwrap();
            let info = probe_public_server(&client, &ipv4_client, &base_url)
                .await
                .unwrap();
            assert!(!info.server_id.is_empty());
            assert!(!info.version.is_empty());
        });
    }
}
