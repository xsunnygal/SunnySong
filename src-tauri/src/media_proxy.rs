use std::{
    collections::{HashMap, VecDeque},
    net::TcpListener as StdTcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, HeaderMap, Method, Response, StatusCode},
    routing::get,
    Router,
};
use reqwest::Client;
use solmusic_application::PlaybackRequestProfile;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::watch,
};
use tokio_util::io::ReaderStream;
use tracing::{error, warn};
use uuid::Uuid;

use crate::dto::PlaybackPreparationDto;

const WEB_PLAYBACK_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131 Safari/537.36";
const VISIONOS_PLAYBACK_USER_AGENT: &str =
    "com.google.visionos.youtube/1.04(RealityDevice17,1; U; CPU visionOS 26_6_0 like Mac OS X; US)";
const ANDROID_VR_PLAYBACK_USER_AGENT: &str = "com.google.android.apps.youtube.vr.oculus/1.43.32 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1)";
const MAX_REGISTERED_SOURCES: usize = 512;
const CHUNK_LENGTH: u64 = 512 * 1024;
const CHUNK_RETRY_ATTEMPTS: usize = 4;

pub struct MediaProxy {
    client: Client,
    base_url: String,
    sources: Mutex<SourceRegistry>,
    cache_dir: PathBuf,
}

#[derive(Default)]
struct SourceRegistry {
    by_token: HashMap<String, RegisteredSource>,
    order: VecDeque<String>,
    by_song: HashMap<String, Arc<RemoteEntry>>,
}

#[derive(Clone)]
struct RegisteredSource {
    location: SourceLocation,
    mime_type: String,
}

#[derive(Clone)]
enum SourceLocation {
    Remote(Arc<RemoteEntry>),
    DirectRemote(String),
    Local(PathBuf),
}

struct RemoteEntry {
    path: PathBuf,
    status: watch::Receiver<DownloadStatus>,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Debug, Default)]
struct DownloadStatus {
    buffered: u64,
    total: Option<u64>,
    complete: bool,
    error: Option<String>,
}

impl MediaProxy {
    pub fn start(cache_dir: PathBuf) -> Result<Arc<Self>, String> {
        let listener = StdTcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|error| format!("could not bind media proxy: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("could not configure media proxy: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("could not read media proxy address: {error}"))?;
        let mut default_headers = HeaderMap::new();
        default_headers.insert(header::ACCEPT, "*/*".parse().expect("valid accept header"));
        if cache_dir.exists() {
            std::fs::remove_dir_all(&cache_dir)
                .map_err(|error| format!("could not clear media cache: {error}"))?;
        }
        std::fs::create_dir_all(&cache_dir)
            .map_err(|error| format!("could not create media cache: {error}"))?;
        let proxy = Arc::new(Self {
            client: Client::builder()
                .user_agent(WEB_PLAYBACK_USER_AGENT)
                .default_headers(default_headers)
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|error| format!("could not create media client: {error}"))?,
            base_url: format!("http://{address}"),
            sources: Mutex::new(SourceRegistry::default()),
            cache_dir,
        });

        let server_proxy = Arc::clone(&proxy);
        tauri::async_runtime::spawn(async move {
            let listener = match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => listener,
                Err(error) => {
                    error!(category = "PLAYER", event = "media_proxy_listener_failed", reason = %error);
                    return;
                }
            };
            let router = Router::new()
                .route("/media/{token}", get(serve_media).head(serve_media))
                .with_state(server_proxy);
            if let Err(error) = axum::serve(listener, router).await {
                error!(category = "PLAYER", event = "media_proxy_server_failed", reason = %error);
            }
        });
        Ok(proxy)
    }

    pub fn register(
        &self,
        preparation: &mut PlaybackPreparationDto,
        local_path: Option<PathBuf>,
        request_profile: PlaybackRequestProfile,
        force_new_remote: bool,
    ) -> Result<(), String> {
        let token = Uuid::new_v4().simple().to_string();
        let song_id = preparation
            .state
            .current
            .as_ref()
            .map(|song| song.id.clone())
            .unwrap_or_else(|| token.clone());
        let mut registry = self
            .sources
            .lock()
            .map_err(|_| "media proxy registry was poisoned".to_owned())?;
        let location = match local_path {
            Some(path) => SourceLocation::Local(path),
            None => {
                if force_new_remote {
                    if let Some(existing) = registry.by_song.get(&song_id) {
                        existing.cancelled.store(true, Ordering::Relaxed);
                    }
                }
                let reusable = registry.by_song.get(&song_id).filter(|entry| {
                    let status = entry.status.borrow();
                    !force_new_remote
                        && status.error.is_none()
                        && !entry.cancelled.load(Ordering::Relaxed)
                });
                let entry = if let Some(entry) = reusable {
                    Arc::clone(entry)
                } else {
                    for (cached_song, entry) in &registry.by_song {
                        if cached_song != &song_id && !entry.status.borrow().complete {
                            entry.cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                    let url = std::mem::take(&mut preparation.source.url);
                    let path = self.cache_dir.join(format!("{token}.part"));
                    let (status_tx, status_rx) = watch::channel(DownloadStatus::default());
                    let cancelled = Arc::new(AtomicBool::new(false));
                    let entry = Arc::new(RemoteEntry {
                        path: path.clone(),
                        status: status_rx,
                        cancelled: Arc::clone(&cancelled),
                    });
                    let client = self.client.clone();
                    tauri::async_runtime::spawn(async move {
                        download_remote_to_growing_file(
                            client,
                            url,
                            path,
                            request_profile,
                            status_tx,
                            cancelled,
                        )
                        .await;
                    });
                    registry.by_song.insert(song_id.clone(), Arc::clone(&entry));
                    entry
                };
                SourceLocation::Remote(entry)
            }
        };
        let source = RegisteredSource {
            location,
            mime_type: preparation.source.mime_type.clone(),
        };
        registry.by_token.insert(token.clone(), source);
        registry.order.push_back(token.clone());
        while registry.order.len() > MAX_REGISTERED_SOURCES {
            if let Some(expired) = registry.order.pop_front() {
                registry.by_token.remove(&expired);
            }
        }
        preparation.source.url = format!("{}/media/{token}", self.base_url);
        Ok(())
    }

    pub fn register_image(&self, url: String) -> Result<String, String> {
        let token = Uuid::new_v4().simple().to_string();
        let source = RegisteredSource {
            location: SourceLocation::DirectRemote(url),
            mime_type: "image/*".into(),
        };
        let mut registry = self
            .sources
            .lock()
            .map_err(|_| "media proxy registry was poisoned".to_owned())?;
        registry.by_token.insert(token.clone(), source);
        registry.order.push_back(token.clone());
        while registry.order.len() > MAX_REGISTERED_SOURCES {
            if let Some(expired) = registry.order.pop_front() {
                registry.by_token.remove(&expired);
            }
        }
        Ok(format!("{}/media/{token}", self.base_url))
    }

    pub async fn download_url_to_path(
        &self,
        url: &str,
        path: &PathBuf,
        request_profile: PlaybackRequestProfile,
    ) -> Result<u64, String> {
        let first_range = bounded_range(None);
        let response = send_remote_range(&self.client, url, &first_range, request_profile).await?;
        if !response.status().is_success() {
            return Err(format!(
                "media provider returned HTTP {}",
                response.status()
            ));
        }
        let total_length = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(content_range_total)
            .ok_or("media provider did not report the audio size")?;
        let first =
            read_initial_remote_range(response, &self.client, url, &first_range, request_profile)
                .await
                .map_err(|error| error.to_string())?;
        let mut file = tokio::fs::File::create(path)
            .await
            .map_err(|error| format!("could not create temporary audio file: {error}"))?;
        let mut written = first.len() as u64;
        file.write_all(&first)
            .await
            .map_err(|error| format!("could not write audio file: {error}"))?;
        while written < total_length {
            let end = written
                .saturating_add(CHUNK_LENGTH - 1)
                .min(total_length - 1);
            let range = format!("bytes={written}-{end}");
            let mut bytes = read_remote_range(&self.client, url, &range, request_profile)
                .await
                .map_err(|error| error.to_string())?;
            let remaining = total_length - written;
            if bytes.len() as u64 > remaining {
                bytes.truncate(remaining as usize);
            }
            file.write_all(&bytes)
                .await
                .map_err(|error| format!("could not write audio file: {error}"))?;
            written += bytes.len() as u64;
        }
        file.flush()
            .await
            .map_err(|error| format!("could not finalize audio file: {error}"))?;
        Ok(written)
    }

    fn source(&self, token: &str) -> Result<RegisteredSource, StatusCode> {
        self.sources
            .lock()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .by_token
            .get(token)
            .cloned()
            .ok_or(StatusCode::NOT_FOUND)
    }
}

async fn serve_media(
    State(proxy): State<Arc<MediaProxy>>,
    Path(token): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    match try_serve_media(&proxy, &token, method, &headers).await {
        Ok(response) => response,
        Err((status, message)) => {
            warn!(category = "PLAYER", event = "media_proxy_request_failed", status = %status, reason = %message);
            Response::builder()
                .status(status)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(Body::from(message))
                .expect("valid media proxy error response")
        }
    }
}

async fn try_serve_media(
    proxy: &MediaProxy,
    token: &str,
    method: Method,
    headers: &HeaderMap,
) -> Result<Response<Body>, (StatusCode, String)> {
    let source = proxy
        .source(token)
        .map_err(|status| proxy_error(status, "unknown media source"))?;
    let requested_range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    if let SourceLocation::Local(path) = &source.location {
        return serve_local_file(path, &source.mime_type, method, requested_range).await;
    }
    if let SourceLocation::DirectRemote(url) = &source.location {
        return serve_direct_remote(&proxy.client, url, method).await;
    }
    let SourceLocation::Remote(entry) = &source.location else {
        unreachable!();
    };
    serve_cached_remote(entry, &source.mime_type, method, requested_range).await
}

async fn serve_direct_remote(
    client: &Client,
    url: &str,
    method: Method,
) -> Result<Response<Body>, (StatusCode, String)> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| proxy_error(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(proxy_error(
            StatusCode::BAD_GATEWAY,
            format!("artwork provider returned {status}"),
        ));
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_owned();
    let content_length = response.content_length();
    let bytes = if method == Method::HEAD {
        Bytes::new()
    } else {
        response
            .bytes()
            .await
            .map_err(|error| proxy_error(StatusCode::BAD_GATEWAY, error.to_string()))?
    };
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "private, max-age=3600")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    if let Some(length) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, length);
    }
    builder
        .body(Body::from(bytes))
        .map_err(|error| proxy_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))
}

async fn serve_cached_remote(
    entry: &Arc<RemoteEntry>,
    mime_type: &str,
    method: Method,
    requested_range: Option<&str>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let mut status_rx = entry.status.clone();
    let total_length = loop {
        let status = status_rx.borrow().clone();
        if let Some(total) = status.total {
            break total;
        }
        if let Some(error) = status.error {
            return Err(proxy_error(StatusCode::BAD_GATEWAY, error));
        }
        status_rx.changed().await.map_err(|_| {
            proxy_error(
                StatusCode::BAD_GATEWAY,
                "media cache downloader stopped unexpectedly",
            )
        })?;
    };
    if total_length == 0 {
        return Err(proxy_error(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "remote media file is empty",
        ));
    }

    let (start, end, response_status) = match requested_range {
        Some(value) => {
            let (start, requested_end) = parse_local_range(value).ok_or_else(|| {
                proxy_error(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "unsupported media byte range",
                )
            })?;
            if start >= total_length {
                return Err(proxy_error(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "requested range is outside the media file",
                ));
            }
            (
                start,
                requested_end
                    .unwrap_or(total_length - 1)
                    .min(total_length - 1),
                StatusCode::PARTIAL_CONTENT,
            )
        }
        None => (0, total_length - 1, StatusCode::OK),
    };
    let response_length = end - start + 1;
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        let path = entry.path.clone();
        let stream = async_stream::stream! {
            let mut position = start;
            while position <= end {
                let status = status_rx.borrow().clone();
                if status.buffered > position {
                    let available_end = (status.buffered - 1).min(end);
                    let requested = (available_end - position + 1).min(128 * 1024) as usize;
                    let mut file = match tokio::fs::File::open(&path).await {
                        Ok(file) => file,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            continue;
                        }
                        Err(error) => {
                            yield Err::<Bytes, std::io::Error>(error);
                            return;
                        }
                    };
                    if let Err(error) = file.seek(std::io::SeekFrom::Start(position)).await {
                        yield Err::<Bytes, std::io::Error>(error);
                        return;
                    }
                    let mut bytes = vec![0; requested];
                    match file.read(&mut bytes).await {
                        Ok(0) => {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        Ok(count) => {
                            bytes.truncate(count);
                            position += count as u64;
                            yield Ok::<Bytes, std::io::Error>(Bytes::from(bytes));
                        }
                        Err(error) => {
                            yield Err::<Bytes, std::io::Error>(error);
                            return;
                        }
                    }
                    continue;
                }
                if let Some(error) = status.error {
                    yield Err::<Bytes, std::io::Error>(std::io::Error::other(error));
                    return;
                }
                if status.complete {
                    yield Err::<Bytes, std::io::Error>(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "cached media ended before the requested range",
                    ));
                    return;
                }
                if status_rx.changed().await.is_err() {
                    yield Err::<Bytes, std::io::Error>(std::io::Error::other(
                        "media cache downloader stopped unexpectedly",
                    ));
                    return;
                }
            }
        };
        Body::from_stream(stream)
    };

    let mut response = Response::builder()
        .status(response_status)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_LENGTH, response_length)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCEPT_RANGES, "bytes");
    if response_status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total_length}"),
        );
    }
    response.body(body).map_err(|error| {
        proxy_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not build cached media response: {error}"),
        )
    })
}

async fn download_remote_to_growing_file(
    client: Client,
    url: String,
    path: PathBuf,
    request_profile: PlaybackRequestProfile,
    status_tx: watch::Sender<DownloadStatus>,
    cancelled: Arc<AtomicBool>,
) {
    let first_range = bounded_range(None);
    let mut consecutive_failures = 0_u32;
    let (total_length, first) = loop {
        if cancelled.load(Ordering::Relaxed) {
            set_download_error(&status_tx, "media prefetch was cancelled");
            return;
        }
        let result = async {
            let response = send_remote_range(&client, &url, &first_range, request_profile).await?;
            if !response.status().is_success() {
                return Err(format!(
                    "media provider returned HTTP {}",
                    response.status()
                ));
            }
            let total = response
                .headers()
                .get(header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .and_then(content_range_total)
                .ok_or_else(|| "media provider did not report the audio size".to_owned())?;
            let bytes =
                read_initial_remote_range(response, &client, &url, &first_range, request_profile)
                    .await
                    .map_err(|error| error.to_string())?;
            Ok::<_, String>((total, bytes))
        }
        .await;
        match result {
            Ok(value) => break value,
            Err(error) => {
                consecutive_failures += 1;
                if consecutive_failures >= 6 {
                    set_download_error(&status_tx, error);
                    return;
                }
                tokio::time::sleep(retry_delay(consecutive_failures)).await;
            }
        }
    };

    let mut file = match tokio::fs::File::create(&path).await {
        Ok(file) => file,
        Err(error) => {
            set_download_error(&status_tx, format!("could not create media cache: {error}"));
            return;
        }
    };
    if let Err(error) = file.write_all(&first).await {
        set_download_error(&status_tx, format!("could not write media cache: {error}"));
        return;
    }
    if let Err(error) = file.flush().await {
        set_download_error(&status_tx, format!("could not flush media cache: {error}"));
        return;
    }
    let mut written = first.len() as u64;
    status_tx.send_modify(|status| {
        status.total = Some(total_length);
        status.buffered = written.min(total_length);
    });

    while written < total_length {
        if cancelled.load(Ordering::Relaxed) {
            set_download_error(&status_tx, "media prefetch was cancelled");
            return;
        }
        let end = written
            .saturating_add(CHUNK_LENGTH - 1)
            .min(total_length - 1);
        let range = format!("bytes={written}-{end}");
        let mut failures = 0_u32;
        let mut bytes = loop {
            match read_remote_range(&client, &url, &range, request_profile).await {
                Ok(bytes) => break bytes,
                Err(error) => {
                    failures += 1;
                    if failures >= 6 {
                        set_download_error(&status_tx, error.to_string());
                        return;
                    }
                    if cancelled.load(Ordering::Relaxed) {
                        set_download_error(&status_tx, "media prefetch was cancelled");
                        return;
                    }
                    tokio::time::sleep(retry_delay(failures)).await;
                }
            }
        };
        let remaining = total_length - written;
        if bytes.len() as u64 > remaining {
            bytes.truncate(remaining as usize);
        }
        if let Err(error) = file.write_all(&bytes).await {
            set_download_error(&status_tx, format!("could not write media cache: {error}"));
            return;
        }
        if let Err(error) = file.flush().await {
            set_download_error(&status_tx, format!("could not flush media cache: {error}"));
            return;
        }
        written += bytes.len() as u64;
        status_tx.send_modify(|status| status.buffered = written);
    }
    status_tx.send_modify(|status| {
        status.buffered = total_length;
        status.complete = true;
    });
}

fn retry_delay(failures: u32) -> Duration {
    Duration::from_millis((500_u64 * (1_u64 << failures.min(4))).min(8_000))
}

fn set_download_error(status_tx: &watch::Sender<DownloadStatus>, error: impl Into<String>) {
    let error = error.into();
    status_tx.send_modify(|status| status.error = Some(error));
}

async fn send_remote_range(
    client: &Client,
    url: &str,
    range: &str,
    request_profile: PlaybackRequestProfile,
) -> Result<reqwest::Response, String> {
    let mut last_error = "media request failed".to_owned();
    for attempt in 0..CHUNK_RETRY_ATTEMPTS {
        match client
            .get(url)
            .header(header::USER_AGENT, playback_user_agent(request_profile))
            .header(header::RANGE, range)
            .send()
            .await
        {
            Ok(response)
                if response.status().is_success()
                    || (!response.status().is_server_error()
                        && response.status() != StatusCode::REQUEST_TIMEOUT
                        && response.status() != StatusCode::TOO_MANY_REQUESTS) =>
            {
                return Ok(response);
            }
            Ok(response) => {
                last_error = format!(
                    "media provider returned HTTP {} for {range}",
                    response.status()
                );
            }
            Err(error) => {
                last_error = format!("media request failed for {range}: {error}");
            }
        }
        if attempt + 1 < CHUNK_RETRY_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(250 * (1_u64 << attempt))).await;
        }
    }
    Err(last_error)
}

async fn read_remote_range(
    client: &Client,
    url: &str,
    range: &str,
    request_profile: PlaybackRequestProfile,
) -> Result<Bytes, std::io::Error> {
    let mut last_error = "media chunk could not be read".to_owned();
    for attempt in 0..CHUNK_RETRY_ATTEMPTS {
        match send_remote_range(client, url, range, request_profile).await {
            Ok(response) if response.status().is_success() => match response.bytes().await {
                Ok(bytes) if !bytes.is_empty() => return Ok(bytes),
                Ok(_) => last_error = format!("media provider returned an empty chunk for {range}"),
                Err(error) => {
                    last_error = format!("could not read media bytes for {range}: {error}")
                }
            },
            Ok(response) => {
                return Err(std::io::Error::other(format!(
                    "media provider returned HTTP {} for {range}",
                    response.status()
                )));
            }
            Err(error) => last_error = error,
        }
        if attempt + 1 < CHUNK_RETRY_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(250 * (1_u64 << attempt))).await;
        }
    }
    Err(std::io::Error::other(last_error))
}

async fn read_initial_remote_range(
    response: reqwest::Response,
    client: &Client,
    url: &str,
    range: &str,
    request_profile: PlaybackRequestProfile,
) -> Result<Bytes, std::io::Error> {
    match response.bytes().await {
        Ok(bytes) if !bytes.is_empty() => Ok(bytes),
        _ => read_remote_range(client, url, range, request_profile).await,
    }
}

async fn serve_local_file(
    path: &PathBuf,
    mime_type: &str,
    method: Method,
    requested_range: Option<&str>,
) -> Result<Response<Body>, (StatusCode, String)> {
    let mut file = tokio::fs::File::open(path).await.map_err(|error| {
        proxy_error(
            StatusCode::NOT_FOUND,
            format!("local media file is unavailable: {error}"),
        )
    })?;
    let total_length = file
        .metadata()
        .await
        .map_err(|error| {
            proxy_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not read local media size: {error}"),
            )
        })?
        .len();
    if total_length == 0 {
        return Err(proxy_error(
            StatusCode::RANGE_NOT_SATISFIABLE,
            "local media file is empty",
        ));
    }

    let range = requested_range.and_then(parse_local_range);
    let (start, end, status) = match range {
        Some((start, requested_end)) if start < total_length => (
            start,
            requested_end
                .unwrap_or(total_length - 1)
                .min(total_length - 1),
            StatusCode::PARTIAL_CONTENT,
        ),
        Some(_) => {
            return Err(proxy_error(
                StatusCode::RANGE_NOT_SATISFIABLE,
                "requested range is outside the local media file",
            ))
        }
        None => (0, total_length - 1, StatusCode::OK),
    };
    let length = end - start + 1;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(|error| {
            proxy_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("could not seek local media file: {error}"),
            )
        })?;

    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(ReaderStream::new(file.take(length)))
    };
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_LENGTH, length)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::ACCEPT_RANGES, "bytes");
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total_length}"),
        );
    }
    response.body(body).map_err(|error| {
        proxy_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("could not build local media response: {error}"),
        )
    })
}

fn parse_local_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start = start.parse().ok()?;
    let end = if end.is_empty() {
        None
    } else {
        end.parse().ok()
    };
    Some((start, end))
}

fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.parse().ok()
}

fn bounded_range(requested: Option<&str>) -> String {
    let parsed = requested
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.split_once('-'));
    let start = parsed
        .and_then(|(start, _)| start.parse::<u64>().ok())
        .unwrap_or(0);
    let maximum_end = start.saturating_add(CHUNK_LENGTH - 1);
    let requested_end = parsed.and_then(|(_, end)| end.parse::<u64>().ok());
    let end = requested_end.map_or(maximum_end, |end| end.min(maximum_end));
    format!("bytes={start}-{end}")
}

fn playback_user_agent(request_profile: PlaybackRequestProfile) -> &'static str {
    match request_profile {
        PlaybackRequestProfile::Web => WEB_PLAYBACK_USER_AGENT,
        PlaybackRequestProfile::VisionOs => VISIONOS_PLAYBACK_USER_AGENT,
        PlaybackRequestProfile::AndroidVr => ANDROID_VR_PLAYBACK_USER_AGENT,
    }
}

fn proxy_error(status: StatusCode, message: impl Into<String>) -> (StatusCode, String) {
    (status, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_total_length_from_content_range() {
        assert_eq!(
            content_range_total("bytes 0-524287/4879322"),
            Some(4_879_322)
        );
        assert_eq!(content_range_total("invalid"), None);
    }

    #[test]
    fn bounded_range_defaults_to_first_chunk() {
        assert_eq!(bounded_range(None), "bytes=0-524287");
    }

    #[test]
    fn media_requests_use_the_explicit_playback_profile() {
        assert_eq!(
            playback_user_agent(PlaybackRequestProfile::VisionOs),
            VISIONOS_PLAYBACK_USER_AGENT
        );
        assert_eq!(
            playback_user_agent(PlaybackRequestProfile::AndroidVr),
            ANDROID_VR_PLAYBACK_USER_AGENT
        );
        assert_eq!(
            playback_user_agent(PlaybackRequestProfile::Web),
            WEB_PLAYBACK_USER_AGENT
        );
    }

    #[test]
    fn bounded_range_keeps_requested_start_and_caps_length() {
        assert_eq!(
            bounded_range(Some("bytes=1048576-")),
            "bytes=1048576-1572863"
        );
        assert_eq!(bounded_range(Some("bytes=42-99")), "bytes=42-99");
    }

    async fn serve_flaky_remote_range(
        State(attempts): State<Arc<std::sync::atomic::AtomicUsize>>,
    ) -> Response<Body> {
        let attempt = attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if attempt < 2 {
            return Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .body(Body::empty())
                .unwrap();
        }
        Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .body(Body::from("recovered"))
            .unwrap()
    }

    #[test]
    fn retries_transient_remote_chunk_failures() {
        tauri::async_runtime::block_on(async {
            let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server_attempts = Arc::clone(&attempts);
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new()
                        .route("/audio", get(serve_flaky_remote_range))
                        .with_state(server_attempts),
                )
                .await
                .unwrap();
            });
            let bytes = read_remote_range(
                &Client::new(),
                &format!("http://{address}/audio"),
                "bytes=0-8",
                PlaybackRequestProfile::Web,
            )
            .await
            .unwrap();
            assert_eq!(&bytes[..], b"recovered");
            assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 3);
            server.abort();
        });
    }

    async fn serve_test_remote_range(headers: HeaderMap) -> Response<Body> {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let total_length = CHUNK_LENGTH * 2 + 17;
        let requested = headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_local_range)
            .expect("test proxy always requests a byte range");
        let start = requested.0;
        let end = requested
            .1
            .unwrap_or(total_length - 1)
            .min(total_length - 1);
        let bytes = (start..=end)
            .map(|position| (position % 251) as u8)
            .collect::<Vec<_>>();
        Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_LENGTH, bytes.len())
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total_length}"),
            )
            .body(Body::from(bytes))
            .unwrap()
    }

    #[test]
    fn open_ended_remote_range_streams_all_upstream_chunks() {
        tauri::async_runtime::block_on(async {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                axum::serve(
                    listener,
                    Router::new().route("/audio", get(serve_test_remote_range)),
                )
                .await
                .unwrap();
            });

            let token = "remote-test".to_owned();
            let path = std::env::temp_dir().join(format!(
                "solmusic-growing-cache-{}",
                Uuid::new_v4().simple()
            ));
            let (status_tx, status_rx) = watch::channel(DownloadStatus::default());
            let mut progress_rx = status_rx.clone();
            let progress = tokio::spawn(async move {
                let mut samples = vec![progress_rx.borrow().clone()];
                while !progress_rx.borrow().complete && progress_rx.borrow().error.is_none() {
                    progress_rx.changed().await.unwrap();
                    samples.push(progress_rx.borrow().clone());
                }
                samples
            });
            let cancelled = Arc::new(AtomicBool::new(false));
            let entry = Arc::new(RemoteEntry {
                path: path.clone(),
                status: status_rx,
                cancelled: Arc::clone(&cancelled),
            });
            let downloader = tokio::spawn(download_remote_to_growing_file(
                Client::new(),
                format!("http://{address}/audio"),
                path.clone(),
                PlaybackRequestProfile::Web,
                status_tx,
                cancelled,
            ));
            let mut registry = SourceRegistry::default();
            registry.by_token.insert(
                token.clone(),
                RegisteredSource {
                    location: SourceLocation::Remote(entry),
                    mime_type: "audio/mp4".to_owned(),
                },
            );
            let proxy = MediaProxy {
                client: Client::new(),
                base_url: String::new(),
                sources: Mutex::new(registry),
                cache_dir: std::env::temp_dir(),
            };
            let mut headers = HeaderMap::new();
            headers.insert(header::RANGE, "bytes=0-".parse().unwrap());
            let response = try_serve_media(&proxy, &token, Method::GET, &headers)
                .await
                .unwrap();
            let expected_length = CHUNK_LENGTH * 2 + 17;
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_RANGE)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                format!("bytes 0-{}/{expected_length}", expected_length - 1)
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_LENGTH)
                    .unwrap()
                    .to_str()
                    .unwrap(),
                expected_length.to_string()
            );
            let body = axum::body::to_bytes(response.into_body(), expected_length as usize)
                .await
                .unwrap();
            assert_eq!(body.len(), expected_length as usize);
            assert_eq!(body[CHUNK_LENGTH as usize], (CHUNK_LENGTH % 251) as u8);
            assert_eq!(
                body[(CHUNK_LENGTH * 2) as usize],
                ((CHUNK_LENGTH * 2) % 251) as u8
            );
            downloader.await.unwrap();
            let progress = progress.await.unwrap();
            assert_eq!(progress.first().unwrap().buffered, 0);
            assert!(progress
                .windows(2)
                .all(|samples| samples[0].buffered <= samples[1].buffered));
            assert!(progress
                .iter()
                .any(|sample| sample.buffered > 0 && sample.buffered < expected_length));
            let completed = progress.last().unwrap();
            assert_eq!(completed.buffered, expected_length);
            assert_eq!(completed.total, Some(expected_length));
            assert!(completed.complete);
            assert!(completed.error.is_none());
            assert_eq!(std::fs::metadata(&path).unwrap().len(), expected_length);
            std::fs::remove_file(path).unwrap();
            server.abort();
        });
    }

    #[test]
    fn serves_local_file_byte_ranges() {
        tauri::async_runtime::block_on(async {
            let path =
                std::env::temp_dir().join(format!("solmusic-local-range-{}", std::process::id()));
            std::fs::write(&path, b"0123456789").unwrap();
            let response = serve_local_file(&path, "audio/test", Method::GET, Some("bytes=2-5"))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(
                response.headers().get(header::CONTENT_RANGE).unwrap(),
                "bytes 2-5/10"
            );
            let body = axum::body::to_bytes(response.into_body(), 16)
                .await
                .unwrap();
            assert_eq!(&body[..], b"2345");
            std::fs::remove_file(path).unwrap();
        });
    }
}
