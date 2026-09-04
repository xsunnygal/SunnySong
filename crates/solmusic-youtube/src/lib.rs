//! YouTube Music implementation of SunnySong's provider port.

use async_trait::async_trait;
use std::{
    collections::HashMap,
    future::{poll_fn, Future},
    pin::pin,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex, RwLock,
    },
    task::Poll,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{header, Client, StatusCode};
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use solmusic_application::{
    domain::{ArtistRef, Song, SongId},
    ArtistPage, AudioQuality, CatalogArtist, CatalogCollection, CatalogFilter,
    CatalogSearchResults, Lyrics, MusicProvider, PlaybackRequestProfile, PlaybackSource,
    ProviderError,
};
use uuid::Uuid;

const MUSIC_API_BASE: &str = "https://music.youtube.com/youtubei/v1";
const PLAYER_API_BASE: &str = "https://www.youtube.com/youtubei/v1";
const MOBILE_PLAYER_API_BASE: &str = "https://youtubei.googleapis.com/youtubei/v1";
const WEB_CLIENT_VERSION: &str = "1.20240819.01.00";
const VISIONOS_CLIENT_VERSION: &str = "1.04";
const ANDROID_VR_CLIENT_VERSION: &str = "1.43.32";
const WEB_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131 Safari/537.36";
const VISIONOS_USER_AGENT: &str =
    "com.google.visionos.youtube/1.04(RealityDevice17,1; U; CPU visionOS 26_6_0 like Mac OS X; US)";
const ANDROID_VR_USER_AGENT: &str = "com.google.android.apps.youtube.vr.oculus/1.43.32 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1)";
const SEARCH_PARAMS: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D";
const ARTIST_SEARCH_PARAMS: &str = "EgWKAQIgAWoKEAkQChAFEAMQBA%3D%3D";
const ALBUM_SEARCH_PARAMS: &str = "EgWKAQIYAWoKEAkQChAFEAMQBA%3D%3D";
const PLAYLIST_SEARCH_PARAMS: &str = "EgWKAQIoAWoKEAkQChAFEAMQBA%3D%3D";
const MUSIC_ORIGIN: &str = "https://music.youtube.com";

/// Parsed YouTube browser credentials. Secret values are deliberately redacted from Debug output.
#[derive(Clone)]
pub struct YouTubeAuth {
    netscape_cookies: Arc<str>,
    cookie_header: Arc<str>,
    sapisid: Arc<str>,
}

impl std::fmt::Debug for YouTubeAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("YouTubeAuth")
            .field("configured", &true)
            .finish()
    }
}

impl YouTubeAuth {
    pub fn from_netscape(input: &str) -> Result<Self, String> {
        let mut normalized = String::from("# Netscape HTTP Cookie File\n");
        let mut cookies = Vec::new();
        let mut sapisid = None;

        for raw_line in input.lines() {
            let line = raw_line.trim_end_matches('\r');
            let cookie_line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if cookie_line.is_empty() || (cookie_line.starts_with('#') && cookie_line == line) {
                continue;
            }
            let fields = cookie_line.split('\t').collect::<Vec<_>>();
            if fields.len() != 7 {
                continue;
            }
            let domain = fields[0]
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if domain != "youtube.com" && !domain.ends_with(".youtube.com") {
                continue;
            }
            let name = fields[5].trim();
            let value = fields[6].trim();
            if name.is_empty()
                || value.is_empty()
                || name.bytes().any(|byte| byte.is_ascii_control())
                || value.bytes().any(|byte| byte.is_ascii_control())
            {
                continue;
            }
            if matches!(name, "SAPISID" | "__Secure-3PAPISID") {
                sapisid = Some(value.to_owned());
            }
            cookies.push((name.to_owned(), value.to_owned()));
            normalized.push_str(line);
            normalized.push('\n');
        }

        let sapisid = sapisid.ok_or_else(|| {
            "cookies.txt does not contain a YouTube SAPISID session cookie".to_owned()
        })?;
        if cookies.is_empty() {
            return Err("cookies.txt contains no youtube.com cookies".into());
        }
        let cookie_header = cookies
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Self {
            netscape_cookies: normalized.into(),
            cookie_header: cookie_header.into(),
            sapisid: sapisid.into(),
        })
    }

    pub fn netscape_cookies(&self) -> &str {
        &self.netscape_cookies
    }

    fn cookie_header(&self) -> &str {
        &self.cookie_header
    }

    fn authorization_at(&self, origin: &str, timestamp: u64) -> String {
        let payload = format!("{timestamp} {} {origin}", self.sapisid);
        let digest = Sha1::digest(payload.as_bytes());
        format!("SAPISIDHASH {timestamp}_{digest:x}")
    }
}

#[derive(Clone, Default)]
pub struct YouTubeAuthState {
    inner: Arc<RwLock<Option<YouTubeAuth>>>,
}

impl std::fmt::Debug for YouTubeAuthState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("YouTubeAuthState")
            .field("configured", &self.is_configured())
            .finish()
    }
}

impl YouTubeAuthState {
    pub fn snapshot(&self) -> Option<YouTubeAuth> {
        self.inner
            .read()
            .expect("YouTube auth state poisoned")
            .clone()
    }

    pub fn replace(&self, auth: YouTubeAuth) {
        *self.inner.write().expect("YouTube auth state poisoned") = Some(auth);
    }

    pub fn clear(&self) {
        *self.inner.write().expect("YouTube auth state poisoned") = None;
    }

    pub fn is_configured(&self) -> bool {
        self.inner
            .read()
            .expect("YouTube auth state poisoned")
            .is_some()
    }
}

/// A YouTube Music provider backed by YouTube's Innertube endpoints.
#[derive(Clone, Debug)]
pub struct YouTubeMusicProvider {
    client: Client,
    audio_quality: Arc<AtomicU8>,
    playback_clients: Arc<Mutex<HashMap<String, PlaybackClient>>>,
    visionos_visitor_data: Arc<Mutex<Option<String>>>,
    auth: YouTubeAuthState,
}

impl YouTubeMusicProvider {
    pub fn new() -> Result<Self, ProviderError> {
        Self::with_auth(YouTubeAuthState::default())
    }

    pub fn with_auth(auth: YouTubeAuthState) -> Result<Self, ProviderError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(network_error)?;
        Ok(Self {
            client,
            audio_quality: Arc::new(AtomicU8::new(audio_quality_value(AudioQuality::High))),
            playback_clients: Arc::new(Mutex::new(HashMap::new())),
            visionos_visitor_data: Arc::new(Mutex::new(None)),
            auth,
        })
    }

    async fn search_category(&self, query: &str, params: &str) -> Result<Value, ProviderError> {
        self.post(
            "search",
            json!({
                "context": web_context(),
                "query": query,
                "params": params,
            }),
            InnertubeClient::WebRemix,
        )
        .await
    }

    async fn post(
        &self,
        endpoint: &str,
        body: Value,
        client: InnertubeClient,
    ) -> Result<Value, ProviderError> {
        let mut request = self
            .client
            .post(format!("{}/{endpoint}", client.api_base()))
            .header(header::USER_AGENT, client.user_agent())
            .header("x-youtube-client-name", client.numeric_name())
            .header("x-youtube-client-version", client.version());
        if matches!(client, InnertubeClient::WebRemix) {
            request = request
                .header(header::ORIGIN, "https://music.youtube.com")
                .header(header::REFERER, "https://music.youtube.com/");
        }
        let response = request.json(&body).send().await.map_err(network_error)?;

        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(ProviderError::Unavailable);
        }
        if !status.is_success() {
            return Err(ProviderError::Network(format!(
                "YouTube Music returned HTTP {status}"
            )));
        }

        response
            .json()
            .await
            .map_err(|error| ProviderError::Incompatible(format!("invalid JSON response: {error}")))
    }

    async fn visionos_visitor_data(&self) -> Result<String, ProviderError> {
        if let Some(visitor_data) = self
            .visionos_visitor_data
            .lock()
            .expect("VisionOS visitor data cache poisoned")
            .clone()
        {
            return Ok(visitor_data);
        }

        let response = self
            .client
            .post(format!("{PLAYER_API_BASE}/visitor_id?prettyPrint=false"))
            .header(header::USER_AGENT, VISIONOS_USER_AGENT)
            .header("x-goog-api-format-version", "2")
            .json(&json!({ "context": playback_context(PlaybackClient::VisionOs, None) }))
            .send()
            .await
            .map_err(network_error)?;
        if !response.status().is_success() {
            return Err(ProviderError::Network(format!(
                "YouTube visitor endpoint returned HTTP {}",
                response.status()
            )));
        }
        let root: Value = response.json().await.map_err(|error| {
            ProviderError::Incompatible(format!("invalid visitor response: {error}"))
        })?;
        let visitor_data = root
            .pointer("/responseContext/visitorData")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| incompatible("visitor response has no visitor data"))?
            .to_owned();
        *self
            .visionos_visitor_data
            .lock()
            .expect("VisionOS visitor data cache poisoned") = Some(visitor_data.clone());
        Ok(visitor_data)
    }

    async fn resolve_with_client(
        &self,
        song_id: &SongId,
        client: PlaybackClient,
    ) -> Result<PlaybackSource, ProviderError> {
        let cpn = random_playback_token(16);
        let visitor_data = if client == PlaybackClient::VisionOs {
            Some(self.visionos_visitor_data().await?)
        } else {
            None
        };
        let request_url = format!(
            "{}/player?prettyPrint=false&t={}&id={}",
            client.api_base(),
            random_playback_token(12),
            song_id.as_str()
        );
        let mut request = self
            .client
            .post(request_url)
            .header(header::USER_AGENT, client.user_agent())
            .header("x-goog-api-format-version", "2")
            .header("x-youtube-client-name", client.numeric_name())
            .header("x-youtube-client-version", client.version());
        if client == PlaybackClient::WebMusicAuthenticated {
            let auth = self.auth.snapshot().ok_or_else(|| {
                ProviderError::VerificationRequired(
                    "connect YouTube Music in Settings, then retry this song".into(),
                )
            })?;
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default();
            request = request
                .header(header::COOKIE, auth.cookie_header())
                .header(
                    header::AUTHORIZATION,
                    auth.authorization_at(MUSIC_ORIGIN, timestamp),
                )
                .header(header::ORIGIN, MUSIC_ORIGIN)
                .header("x-origin", MUSIC_ORIGIN)
                .header("x-goog-authuser", "0");
        }
        let response = request
            .json(&json!({
                "context": playback_context(client, visitor_data.as_deref()),
                "videoId": song_id.as_str(),
                "cpn": cpn,
                "contentCheckOk": true,
                "racyCheckOk": true,
            }))
            .send()
            .await
            .map_err(network_error)?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            return Err(ProviderError::Unavailable);
        }
        if status == StatusCode::FORBIDDEN {
            let detail = if client == PlaybackClient::WebMusicAuthenticated {
                "the saved YouTube session was rejected or this stream requires a proof token"
            } else {
                "YouTube rejected anonymous playback; connect YouTube Music in Settings"
            };
            return Err(ProviderError::VerificationRequired(detail.into()));
        }
        if !status.is_success() {
            return Err(ProviderError::Network(format!(
                "YouTube player returned HTTP {status} for {}",
                client.label()
            )));
        }
        let root: Value = response.json().await.map_err(|error| {
            ProviderError::Incompatible(format!("invalid player response: {error}"))
        })?;
        parse_playback(
            &root,
            audio_quality_from_value(self.audio_quality.load(Ordering::Relaxed)),
            client.request_profile(),
            Some(&cpn),
        )
    }

    async fn resolve_from_clients(
        &self,
        song_id: &SongId,
        clients: [PlaybackClient; 2],
    ) -> Result<PlaybackSource, ProviderError> {
        let mut last_error = ProviderError::Unavailable;
        let mut verification_error = None;
        for client in clients {
            match self.resolve_with_client(song_id, client).await {
                Ok(source) => {
                    self.remember_playback_client(song_id, client);
                    return Ok(source);
                }
                Err(error) => {
                    if is_verification_required(&error) {
                        verification_error = Some(error);
                    } else {
                        last_error = error;
                    }
                }
            }
        }
        Err(verification_error.unwrap_or(last_error))
    }

    async fn resolve_with_auth_fallback(
        &self,
        song_id: &SongId,
        clients: [PlaybackClient; 2],
    ) -> Result<PlaybackSource, ProviderError> {
        match self.resolve_from_clients(song_id, clients).await {
            Err(error) if is_verification_required(&error) && self.auth.is_configured() => {
                let source = self
                    .resolve_with_client(song_id, PlaybackClient::WebMusicAuthenticated)
                    .await?;
                self.remember_playback_client(song_id, PlaybackClient::WebMusicAuthenticated);
                Ok(source)
            }
            result => result,
        }
    }

    fn remember_playback_client(&self, song_id: &SongId, client: PlaybackClient) {
        self.playback_clients
            .lock()
            .expect("playback client state poisoned")
            .insert(song_id.as_str().to_owned(), client);
    }
}

#[async_trait]
impl MusicProvider for YouTubeMusicProvider {
    fn set_audio_quality(&self, quality: AudioQuality) {
        self.audio_quality
            .store(audio_quality_value(quality), Ordering::Relaxed);
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<Song>, ProviderError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let response = self
            .post(
                "search",
                json!({
                    "context": web_context(),
                    "query": query,
                    "params": SEARCH_PARAMS,
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        parse_search(&response, limit)
    }

    async fn search_catalog(
        &self,
        query: &str,
        filter: CatalogFilter,
        limit: usize,
    ) -> Result<CatalogSearchResults, ProviderError> {
        if limit == 0 {
            return Ok(CatalogSearchResults::default());
        }

        match filter {
            CatalogFilter::All => {
                let (songs, artists, albums, playlists) = join4(
                    self.search_category(query, SEARCH_PARAMS),
                    self.search_category(query, ARTIST_SEARCH_PARAMS),
                    self.search_category(query, ALBUM_SEARCH_PARAMS),
                    self.search_category(query, PLAYLIST_SEARCH_PARAMS),
                )
                .await;
                let songs = songs?;
                let artists = artists?;
                let albums = albums?;
                let playlists = playlists?;
                Ok(CatalogSearchResults {
                    songs: parse_search(&songs, limit)?,
                    artists: parse_catalog_artists(&artists, limit)?,
                    albums: parse_catalog_collections(&albums, &["MPRE", "OLAK"], "album", limit)?,
                    playlists: parse_catalog_collections(
                        &playlists,
                        &["VL", "PL"],
                        "playlist",
                        limit,
                    )?,
                })
            }
            CatalogFilter::Songs => Ok(CatalogSearchResults {
                songs: self.search(query, limit).await?,
                ..CatalogSearchResults::default()
            }),
            CatalogFilter::Artists => {
                let response = self.search_category(query, ARTIST_SEARCH_PARAMS).await?;
                Ok(CatalogSearchResults {
                    artists: parse_catalog_artists(&response, limit)?,
                    ..CatalogSearchResults::default()
                })
            }
            CatalogFilter::Albums => {
                let response = self.search_category(query, ALBUM_SEARCH_PARAMS).await?;
                Ok(CatalogSearchResults {
                    albums: parse_catalog_collections(
                        &response,
                        &["MPRE", "OLAK"],
                        "album",
                        limit,
                    )?,
                    ..CatalogSearchResults::default()
                })
            }
            CatalogFilter::Playlists => {
                let response = self.search_category(query, PLAYLIST_SEARCH_PARAMS).await?;
                Ok(CatalogSearchResults {
                    playlists: parse_catalog_collections(
                        &response,
                        &["VL", "PL"],
                        "playlist",
                        limit,
                    )?,
                    ..CatalogSearchResults::default()
                })
            }
        }
    }

    async fn artist_page(&self, artist_id: &str) -> Result<ArtistPage, ProviderError> {
        let artist_id = artist_id.trim();
        if !artist_id.starts_with("UC") {
            return Err(incompatible("artist browse id must start with UC"));
        }
        let response = self
            .post(
                "browse",
                json!({
                    "context": web_context(),
                    "browseId": artist_id,
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        parse_artist_page(&response, artist_id)
    }

    async fn collection_songs(&self, collection_id: &str) -> Result<Vec<Song>, ProviderError> {
        let collection_id = collection_id.trim();
        if !["MPRE", "OLAK", "VL", "PL"]
            .iter()
            .any(|prefix| collection_id.starts_with(prefix))
        {
            return Err(incompatible("unsupported collection browse id"));
        }
        let response = self
            .post(
                "browse",
                json!({
                    "context": web_context(),
                    "browseId": collection_id,
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        parse_collection_songs(&response)
    }

    async fn related(&self, song_id: &SongId, limit: usize) -> Result<Vec<Song>, ProviderError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let preview = self
            .post(
                "next",
                json!({
                    "context": web_context(),
                    "videoId": song_id.as_str(),
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        let automix = parse_automix_endpoint(&preview)?;
        let response = self
            .post(
                "next",
                json!({
                    "context": web_context(),
                    "videoId": song_id.as_str(),
                    "playlistId": automix.playlist_id,
                    "params": automix.params,
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        parse_related(&response, limit)
    }

    async fn lyrics(&self, song_id: &SongId) -> Result<Option<Lyrics>, ProviderError> {
        let next = self
            .post(
                "next",
                json!({
                    "context": web_context(),
                    "videoId": song_id.as_str(),
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        let Some(browse_id) = parse_lyrics_browse_id(&next) else {
            return Ok(None);
        };
        let browse = self
            .post(
                "browse",
                json!({
                    "context": web_context(),
                    "browseId": browse_id,
                }),
                InnertubeClient::WebRemix,
            )
            .await?;
        Ok(parse_lyrics(&browse))
    }

    async fn resolve_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        self.resolve_with_auth_fallback(
            song_id,
            [PlaybackClient::VisionOs, PlaybackClient::AndroidVr143],
        )
        .await
    }

    async fn refresh_playback(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        let previous = self
            .playback_clients
            .lock()
            .expect("playback client state poisoned")
            .get(song_id.as_str())
            .copied()
            .unwrap_or(PlaybackClient::VisionOs);
        let clients = if previous == PlaybackClient::WebMusicAuthenticated {
            [PlaybackClient::VisionOs, PlaybackClient::AndroidVr143]
        } else {
            [previous.next(), previous]
        };
        self.resolve_with_auth_fallback(song_id, clients).await
    }
}

fn audio_quality_value(quality: AudioQuality) -> u8 {
    match quality {
        AudioQuality::Low => 0,
        AudioQuality::Medium => 1,
        AudioQuality::High => 2,
    }
}

fn audio_quality_from_value(value: u8) -> AudioQuality {
    match value {
        0 => AudioQuality::Low,
        1 => AudioQuality::Medium,
        _ => AudioQuality::High,
    }
}

async fn join4<A, B, C, D>(
    first: A,
    second: B,
    third: C,
    fourth: D,
) -> (A::Output, B::Output, C::Output, D::Output)
where
    A: Future,
    B: Future,
    C: Future,
    D: Future,
{
    let mut first = pin!(first);
    let mut second = pin!(second);
    let mut third = pin!(third);
    let mut fourth = pin!(fourth);
    let mut first_output = None;
    let mut second_output = None;
    let mut third_output = None;
    let mut fourth_output = None;

    poll_fn(|context| {
        if first_output.is_none() {
            if let Poll::Ready(output) = first.as_mut().poll(context) {
                first_output = Some(output);
            }
        }
        if second_output.is_none() {
            if let Poll::Ready(output) = second.as_mut().poll(context) {
                second_output = Some(output);
            }
        }
        if third_output.is_none() {
            if let Poll::Ready(output) = third.as_mut().poll(context) {
                third_output = Some(output);
            }
        }
        if fourth_output.is_none() {
            if let Poll::Ready(output) = fourth.as_mut().poll(context) {
                fourth_output = Some(output);
            }
        }

        if first_output.is_some()
            && second_output.is_some()
            && third_output.is_some()
            && fourth_output.is_some()
        {
            Poll::Ready((
                first_output.take().unwrap(),
                second_output.take().unwrap(),
                third_output.take().unwrap(),
                fourth_output.take().unwrap(),
            ))
        } else {
            Poll::Pending
        }
    })
    .await
}

#[derive(Clone, Copy)]
enum InnertubeClient {
    WebRemix,
}

impl InnertubeClient {
    fn api_base(self) -> &'static str {
        match self {
            Self::WebRemix => MUSIC_API_BASE,
        }
    }

    fn version(self) -> &'static str {
        match self {
            Self::WebRemix => WEB_CLIENT_VERSION,
        }
    }

    fn numeric_name(self) -> &'static str {
        match self {
            Self::WebRemix => "67",
        }
    }

    fn user_agent(self) -> &'static str {
        match self {
            Self::WebRemix => WEB_USER_AGENT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlaybackClient {
    VisionOs,
    AndroidVr143,
    WebMusicAuthenticated,
}

impl PlaybackClient {
    fn next(self) -> Self {
        match self {
            Self::VisionOs => Self::AndroidVr143,
            Self::AndroidVr143 | Self::WebMusicAuthenticated => Self::VisionOs,
        }
    }

    fn version(self) -> &'static str {
        match self {
            Self::VisionOs => VISIONOS_CLIENT_VERSION,
            Self::AndroidVr143 => ANDROID_VR_CLIENT_VERSION,
            Self::WebMusicAuthenticated => WEB_CLIENT_VERSION,
        }
    }

    fn numeric_name(self) -> &'static str {
        match self {
            Self::VisionOs => "101",
            Self::AndroidVr143 => "28",
            Self::WebMusicAuthenticated => "67",
        }
    }

    fn user_agent(self) -> &'static str {
        match self {
            Self::VisionOs => VISIONOS_USER_AGENT,
            Self::AndroidVr143 => ANDROID_VR_USER_AGENT,
            Self::WebMusicAuthenticated => WEB_USER_AGENT,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::VisionOs => "VisionOS",
            Self::AndroidVr143 => "Android VR 1.43",
            Self::WebMusicAuthenticated => "authenticated YouTube Music Web",
        }
    }

    fn request_profile(self) -> PlaybackRequestProfile {
        match self {
            Self::VisionOs => PlaybackRequestProfile::VisionOs,
            Self::AndroidVr143 => PlaybackRequestProfile::AndroidVr,
            Self::WebMusicAuthenticated => PlaybackRequestProfile::Web,
        }
    }

    fn api_base(self) -> &'static str {
        match self {
            Self::VisionOs | Self::AndroidVr143 => MOBILE_PLAYER_API_BASE,
            Self::WebMusicAuthenticated => MUSIC_API_BASE,
        }
    }
}

fn playback_context(client: PlaybackClient, visitor_data: Option<&str>) -> Value {
    let mut context = match client {
        PlaybackClient::VisionOs => json!({
            "clientName": "VISIONOS",
            "clientVersion": VISIONOS_CLIENT_VERSION,
            "clientScreen": "WATCH",
            "platform": "MOBILE",
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "osName": "visionOS",
            "osVersion": "26.6.0.23O770",
            "hl": "en",
            "gl": "US",
            "utcOffsetMinutes": 0,
        }),
        PlaybackClient::AndroidVr143 => json!({
            "clientName": "ANDROID_VR",
            "clientVersion": ANDROID_VR_CLIENT_VERSION,
            "clientScreen": "WATCH",
            "platform": "MOBILE",
            "deviceMake": "Oculus",
            "deviceModel": "Quest 3",
            "osName": "Android",
            "osVersion": "12",
            "androidSdkVersion": 32,
            "hl": "en",
            "gl": "US",
            "utcOffsetMinutes": 0,
        }),
        PlaybackClient::WebMusicAuthenticated => json!({
            "clientName": "WEB_REMIX",
            "clientVersion": WEB_CLIENT_VERSION,
            "clientScreen": "WATCH",
            "platform": "DESKTOP",
            "hl": "en",
            "gl": "US",
            "utcOffsetMinutes": 0,
        }),
    };
    if let Some(visitor_data) = visitor_data {
        context["visitorData"] = Value::String(visitor_data.to_owned());
    }
    json!({
        "client": context,
        "request": {
            "internalExperimentFlags": [],
            "useSsl": true,
        },
        "user": {
            "lockedSafetyMode": false,
        },
    })
}

fn random_playback_token(length: usize) -> String {
    Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(length)
        .collect()
}

fn web_context() -> Value {
    json!({
        "client": {
            "clientName": "WEB_REMIX",
            "clientVersion": WEB_CLIENT_VERSION,
            "hl": "en",
        }
    })
}

fn network_error(error: reqwest::Error) -> ProviderError {
    if error.is_timeout() || error.is_connect() {
        ProviderError::Unavailable
    } else {
        ProviderError::Network(error.to_string())
    }
}

fn is_verification_required(error: &ProviderError) -> bool {
    matches!(error, ProviderError::VerificationRequired(_))
}

fn reason_requires_verification(status: &str, reason: &str) -> bool {
    if status.eq_ignore_ascii_case("LOGIN_REQUIRED") {
        return true;
    }
    let normalized = reason.to_ascii_lowercase();
    normalized.contains("sign in to confirm")
        || normalized.contains("confirm you're not a bot")
        || normalized.contains("confirm you’re not a bot")
        || normalized.contains("login required")
}

fn parse_lyrics_browse_id(root: &Value) -> Option<String> {
    match root {
        Value::Object(object) => {
            if let Some(endpoint) = object.get("browseEndpoint") {
                let page_type = endpoint
                    .pointer("/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
                    .and_then(Value::as_str);
                if page_type == Some("MUSIC_PAGE_TYPE_TRACK_LYRICS") {
                    return endpoint
                        .get("browseId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            if let Some(tab) = object.get("tabRenderer") {
                let title = tab.get("title").and_then(|title| {
                    title
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| text_object(title))
                });
                if title.is_some_and(|title| title.eq_ignore_ascii_case("lyrics")) {
                    if let Some(id) = tab
                        .pointer("/endpoint/browseEndpoint/browseId")
                        .and_then(Value::as_str)
                    {
                        return Some(id.to_owned());
                    }
                }
            }
            object.values().find_map(parse_lyrics_browse_id)
        }
        Value::Array(values) => values.iter().find_map(parse_lyrics_browse_id),
        _ => None,
    }
}

fn parse_lyrics(root: &Value) -> Option<Lyrics> {
    let shelf = find_renderers(root, "musicDescriptionShelfRenderer")
        .into_iter()
        .next()?;
    let text = shelf.get("description").and_then(text_object)?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let attribution = shelf
        .get("footer")
        .and_then(text_object)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    Some(Lyrics {
        text: text.to_owned(),
        attribution,
    })
}

fn parse_search(root: &Value, limit: usize) -> Result<Vec<Song>, ProviderError> {
    if root.get("contents").is_none() {
        return Err(incompatible("search response has no contents"));
    }
    Ok(find_renderers(root, "musicResponsiveListItemRenderer")
        .into_iter()
        .filter_map(parse_search_song)
        .take(limit)
        .collect())
}

fn parse_search_song(renderer: &Value) -> Option<Song> {
    parse_responsive_song(renderer, None)
}

fn parse_responsive_song(renderer: &Value, fallback_artist: Option<&ArtistRef>) -> Option<Song> {
    let columns = renderer.get("flexColumns")?.as_array()?;
    let title_runs = columns
        .first()?
        .pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs")?
        .as_array()?;
    let title = first_text(title_runs)?;
    let id = renderer
        .pointer("/playlistItemData/videoId")
        .and_then(Value::as_str)
        .or_else(|| first_video_id(title_runs))?;
    let metadata_runs = columns
        .get(1)
        .and_then(|column| column.pointer("/musicResponsiveListItemFlexColumnRenderer/text/runs"))
        .and_then(Value::as_array);
    let fixed_runs = renderer
        .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs")
        .and_then(Value::as_array);

    let artist = metadata_runs
        .and_then(|runs| find_stable_artist(runs))
        .or_else(|| fallback_artist.cloned())
        .or_else(|| metadata_runs.and_then(|runs| find_artist(runs)))?;
    let album = metadata_runs.and_then(|runs| find_album(runs));
    let duration = fixed_runs
        .and_then(|runs| last_text(runs))
        .or_else(|| metadata_runs.and_then(|runs| last_duration_text(runs)))
        .and_then(parse_duration_ms);

    Some(Song {
        id: SongId::new(id)?,
        title,
        artist,
        album_id: album.as_ref().and_then(|item| item.0.clone()),
        album_name: album.map(|item| item.1),
        duration_ms: duration,
        thumbnail_url: thumbnail_url(renderer),
    })
}

fn parse_catalog_artists(root: &Value, limit: usize) -> Result<Vec<CatalogArtist>, ProviderError> {
    if root.get("contents").is_none() {
        return Err(incompatible("artist search response has no contents"));
    }
    let mut artists = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for renderer in find_renderers(root, "musicResponsiveListItemRenderer")
        .into_iter()
        .chain(find_renderers(root, "musicTwoRowItemRenderer"))
    {
        if let Some(artist) = parse_catalog_artist(renderer) {
            if ids.insert(artist.id.clone()) {
                artists.push(artist);
                if artists.len() == limit {
                    break;
                }
            }
        }
    }
    Ok(artists)
}

fn parse_catalog_artist(renderer: &Value) -> Option<CatalogArtist> {
    let id = first_browse_id_with_prefixes(renderer, &["UC"])?;
    Some(CatalogArtist {
        id: id.to_owned(),
        name: renderer_primary_text(renderer)?,
        thumbnail_url: thumbnail_url(renderer),
        subtitle: renderer_secondary_text(renderer),
        source: "youtube".into(),
    })
}

fn parse_catalog_collections(
    root: &Value,
    prefixes: &[&str],
    default_kind: &str,
    limit: usize,
) -> Result<Vec<CatalogCollection>, ProviderError> {
    if root.get("contents").is_none() {
        return Err(incompatible("collection search response has no contents"));
    }
    Ok(collect_catalog_collections(
        root,
        prefixes,
        default_kind,
        limit,
    ))
}

fn collect_catalog_collections(
    root: &Value,
    prefixes: &[&str],
    default_kind: &str,
    limit: usize,
) -> Vec<CatalogCollection> {
    let mut collections = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for renderer in find_renderers(root, "musicResponsiveListItemRenderer")
        .into_iter()
        .chain(find_renderers(root, "musicTwoRowItemRenderer"))
    {
        if let Some(collection) = parse_catalog_collection(renderer, prefixes, default_kind) {
            if ids.insert(collection.id.clone()) {
                collections.push(collection);
                if collections.len() == limit {
                    break;
                }
            }
        }
    }
    collections
}

fn parse_catalog_collection(
    renderer: &Value,
    prefixes: &[&str],
    default_kind: &str,
) -> Option<CatalogCollection> {
    let id = first_browse_id_with_prefixes(renderer, prefixes)?;
    let subtitle = renderer_secondary_text(renderer);
    Some(CatalogCollection {
        id: id.to_owned(),
        title: renderer_primary_text(renderer)?,
        kind: collection_kind(subtitle.as_deref(), default_kind),
        subtitle,
        thumbnail_url: thumbnail_url(renderer),
    })
}

fn parse_artist_page(root: &Value, requested_artist_id: &str) -> Result<ArtistPage, ProviderError> {
    if root.get("contents").is_none() {
        return Err(incompatible("artist response has no contents"));
    }
    let header = find_renderers(root, "musicImmersiveHeaderRenderer")
        .into_iter()
        .next()
        .or_else(|| {
            find_renderers(root, "musicVisualHeaderRenderer")
                .into_iter()
                .next()
        })
        .or_else(|| {
            find_renderers(root, "musicArtistHeaderRenderer")
                .into_iter()
                .next()
        })
        .ok_or_else(|| incompatible("artist response has no supported header"))?;
    let artist = CatalogArtist {
        id: requested_artist_id.to_owned(),
        name: renderer_primary_text(header)
            .ok_or_else(|| incompatible("artist header has no title"))?,
        thumbnail_url: thumbnail_url(header),
        subtitle: renderer_secondary_text(header),
        source: "youtube".into(),
    };
    let fallback_artist = ArtistRef {
        id: Some(artist.id.clone()),
        name: artist.name.clone(),
    };
    let mut page = ArtistPage {
        artist,
        top_songs: Vec::new(),
        songs: Vec::new(),
        latest_releases: Vec::new(),
        albums: Vec::new(),
        singles: Vec::new(),
        playlists: Vec::new(),
    };

    for shelf in find_renderers(root, "musicShelfRenderer")
        .into_iter()
        .chain(find_renderers(root, "musicCarouselShelfRenderer"))
    {
        let Some(title) = shelf_title(shelf) else {
            continue;
        };
        let title = title.trim().to_ascii_lowercase();
        match title.as_str() {
            "top songs" | "popular" => page
                .top_songs
                .extend(parse_shelf_songs(shelf, &fallback_artist)),
            "songs" => page
                .songs
                .extend(parse_shelf_songs(shelf, &fallback_artist)),
            "latest release" | "latest releases" => page.latest_releases.extend(
                collect_catalog_collections(shelf, &["MPRE", "OLAK"], "release", usize::MAX),
            ),
            "albums" => page.albums.extend(collect_catalog_collections(
                shelf,
                &["MPRE", "OLAK"],
                "album",
                usize::MAX,
            )),
            "singles" | "singles & eps" | "singles and eps" | "singles & ep's" => {
                page.singles.extend(collect_catalog_collections(
                    shelf,
                    &["MPRE", "OLAK"],
                    "single",
                    usize::MAX,
                ))
            }
            title if title == "featured on" || title.contains("playlist") => page.playlists.extend(
                collect_catalog_collections(shelf, &["VL", "PL"], "playlist", usize::MAX),
            ),
            _ => {}
        }
    }

    Ok(page)
}

fn parse_collection_songs(root: &Value) -> Result<Vec<Song>, ProviderError> {
    if root.get("contents").is_none() {
        return Err(incompatible("collection response has no contents"));
    }
    let mut songs = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for renderer in find_renderers(root, "musicResponsiveListItemRenderer") {
        if let Some(song) = parse_responsive_song(renderer, None) {
            if ids.insert(song.id.clone()) {
                songs.push(song);
            }
        }
    }
    Ok(songs)
}

fn parse_shelf_songs(shelf: &Value, fallback_artist: &ArtistRef) -> Vec<Song> {
    find_renderers(shelf, "musicResponsiveListItemRenderer")
        .into_iter()
        .filter_map(|renderer| parse_responsive_song(renderer, Some(fallback_artist)))
        .collect()
}

fn shelf_title(renderer: &Value) -> Option<String> {
    [
        "/title",
        "/header/musicCarouselShelfBasicHeaderRenderer/title",
        "/header/musicShelfBasicHeaderRenderer/title",
    ]
    .into_iter()
    .find_map(|pointer| text_object(renderer.pointer(pointer)?))
}

fn renderer_primary_text(renderer: &Value) -> Option<String> {
    if let Some(columns) = renderer.get("flexColumns").and_then(Value::as_array) {
        if let Some(text) = columns
            .first()
            .and_then(|column| column.pointer("/musicResponsiveListItemFlexColumnRenderer/text"))
        {
            if let Some(text) = text_object(text) {
                return Some(text);
            }
        }
    }
    renderer.get("title").and_then(text_object)
}

fn renderer_secondary_text(renderer: &Value) -> Option<String> {
    if let Some(columns) = renderer.get("flexColumns").and_then(Value::as_array) {
        if let Some(text) = columns
            .get(1)
            .and_then(|column| column.pointer("/musicResponsiveListItemFlexColumnRenderer/text"))
        {
            if let Some(text) = text_object(text) {
                return Some(text);
            }
        }
    }
    renderer
        .get("subtitle")
        .and_then(text_object)
        .or_else(|| renderer.get("straplineTextOne").and_then(text_object))
}

fn text_object(value: &Value) -> Option<String> {
    value
        .get("simpleText")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            value
                .get("runs")
                .and_then(Value::as_array)
                .and_then(|runs| join_text(runs))
        })
}

fn join_text(runs: &[Value]) -> Option<String> {
    let text = runs
        .iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .collect::<String>();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn first_browse_id_with_prefixes<'a>(root: &'a Value, prefixes: &[&str]) -> Option<&'a str> {
    fn visit<'a>(value: &'a Value, prefixes: &[&str]) -> Option<&'a str> {
        match value {
            Value::Object(object) => {
                if let Some(id) = object.get("browseId").and_then(Value::as_str) {
                    if prefixes.iter().any(|prefix| id.starts_with(prefix)) {
                        return Some(id);
                    }
                }
                object.values().find_map(|child| visit(child, prefixes))
            }
            Value::Array(array) => array.iter().find_map(|child| visit(child, prefixes)),
            _ => None,
        }
    }
    visit(root, prefixes)
}

fn collection_kind(subtitle: Option<&str>, default_kind: &str) -> String {
    let subtitle = subtitle.unwrap_or_default().to_ascii_lowercase();
    if subtitle
        .split([' ', '•', '·'])
        .any(|part| part.trim() == "ep")
    {
        "ep".into()
    } else if subtitle.contains("single") {
        "single".into()
    } else if subtitle.contains("album") {
        "album".into()
    } else if subtitle.contains("playlist") {
        "playlist".into()
    } else {
        default_kind.into()
    }
}

fn parse_related(root: &Value, limit: usize) -> Result<Vec<Song>, ProviderError> {
    if root.get("contents").is_none() {
        return Err(incompatible("related response has no contents"));
    }
    Ok(find_renderers(root, "playlistPanelVideoRenderer")
        .into_iter()
        .filter_map(parse_panel_song)
        .take(limit)
        .collect())
}

fn parse_panel_song(renderer: &Value) -> Option<Song> {
    let id = renderer.get("videoId")?.as_str()?;
    let title_runs = renderer.pointer("/title/runs")?.as_array()?;
    let byline_runs = renderer.pointer("/longBylineText/runs")?.as_array()?;
    let artist = find_artist(byline_runs)?;
    let album = find_album(byline_runs);
    let duration = renderer
        .pointer("/lengthText/simpleText")
        .and_then(Value::as_str)
        .or_else(|| {
            renderer
                .pointer("/lengthText/runs/0/text")
                .and_then(Value::as_str)
        })
        .and_then(parse_duration_ms);

    Some(Song {
        id: SongId::new(id)?,
        title: first_text(title_runs)?,
        artist,
        album_id: album.as_ref().and_then(|item| item.0.clone()),
        album_name: album.map(|item| item.1),
        duration_ms: duration,
        thumbnail_url: thumbnail_url(renderer),
    })
}

struct AutomixEndpoint {
    playlist_id: String,
    params: String,
}

fn parse_automix_endpoint(root: &Value) -> Result<AutomixEndpoint, ProviderError> {
    let renderer = find_renderers(root, "automixPreviewVideoRenderer")
        .into_iter()
        .next()
        .ok_or_else(|| incompatible("next response has no automix preview"))?;
    let endpoint = renderer
        .pointer("/content/automixPlaylistVideoRenderer/navigationEndpoint/watchPlaylistEndpoint")
        .or_else(|| renderer.pointer("/navigationEndpoint/watchPlaylistEndpoint"))
        .ok_or_else(|| incompatible("automix preview has no watch playlist endpoint"))?;
    let playlist_id = endpoint
        .get("playlistId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| incompatible("automix endpoint has no playlist id"))?;
    let params = endpoint
        .get("params")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| incompatible("automix endpoint has no params"))?;

    Ok(AutomixEndpoint {
        playlist_id: playlist_id.to_owned(),
        params: params.to_owned(),
    })
}

fn parse_playback(
    root: &Value,
    audio_quality: AudioQuality,
    request_profile: PlaybackRequestProfile,
    content_playback_nonce: Option<&str>,
) -> Result<PlaybackSource, ProviderError> {
    let status = root
        .pointer("/playabilityStatus/status")
        .and_then(Value::as_str)
        .unwrap_or("UNKNOWN");
    if status != "OK" {
        let reason = root
            .pointer("/playabilityStatus/reason")
            .and_then(Value::as_str)
            .unwrap_or(status);
        if reason_requires_verification(status, reason) {
            return Err(ProviderError::VerificationRequired(reason.to_owned()));
        }
        return Err(ProviderError::Unplayable(reason.to_owned()));
    }

    let formats = root
        .pointer("/streamingData/adaptiveFormats")
        .and_then(Value::as_array)
        .ok_or_else(|| incompatible("player response has no adaptive formats"))?;
    let audio_formats = formats
        .iter()
        .filter(|format| {
            format.get("url").and_then(Value::as_str).is_some()
                && format
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .is_some_and(|mime| mime.starts_with("audio/"))
        })
        .collect::<Vec<_>>();
    let format_key = |format: &&Value| {
        let mime = format
            .get("mimeType")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let bitrate = format
            .get("bitrate")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        (u8::from(mime.starts_with("audio/mp4")), bitrate)
    };
    let best = match audio_quality {
        AudioQuality::Low => audio_formats.iter().min_by_key(|format| {
            let (is_mp4, bitrate) = format_key(format);
            (u8::from(is_mp4 == 0), bitrate)
        }),
        AudioQuality::Medium => audio_formats
            .iter()
            .filter(|format| format_key(format).1 <= 160_000)
            .max_by_key(|format| format_key(format))
            .or_else(|| {
                audio_formats
                    .iter()
                    .min_by_key(|format| format_key(format).1)
            }),
        AudioQuality::High => audio_formats.iter().max_by_key(|format| format_key(format)),
    }
    .copied()
    .ok_or_else(|| ProviderError::Unplayable("no direct audio stream is available".into()))?;
    let mut url = best.get("url").and_then(Value::as_str).unwrap().to_owned();
    if let Some(cpn) = content_playback_nonce {
        let mut parsed = reqwest::Url::parse(&url).map_err(|error| {
            ProviderError::Incompatible(format!("invalid playback URL: {error}"))
        })?;
        if !parsed.query_pairs().any(|(key, _)| key == "cpn") {
            parsed.query_pairs_mut().append_pair("cpn", cpn);
        }
        url = parsed.to_string();
    }
    let mime_type = best
        .get("mimeType")
        .and_then(Value::as_str)
        .unwrap()
        .to_owned();

    Ok(PlaybackSource {
        expires_at_ms: playback_expiry_ms(&url),
        url,
        mime_type,
        local_path: None,
        request_profile,
    })
}

fn playback_expiry_ms(url: &str) -> Option<i64> {
    reqwest::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(key, _)| key == "expire")?
        .1
        .parse::<i64>()
        .ok()?
        .checked_mul(1_000)
}

fn find_renderers<'a>(root: &'a Value, name: &str) -> Vec<&'a Value> {
    fn visit<'a>(value: &'a Value, name: &str, found: &mut Vec<&'a Value>) {
        match value {
            Value::Object(object) => {
                if let Some(renderer) = object.get(name) {
                    found.push(renderer);
                }
                for child in object.values() {
                    visit(child, name, found);
                }
            }
            Value::Array(array) => {
                for child in array {
                    visit(child, name, found);
                }
            }
            _ => {}
        }
    }

    let mut found = Vec::new();
    visit(root, name, &mut found);
    found
}

fn first_text(runs: &[Value]) -> Option<String> {
    runs.iter()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
        .map(str::to_owned)
}

fn last_text(runs: &[Value]) -> Option<&str> {
    runs.iter()
        .rev()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .find(|text| !text.trim().is_empty())
}

fn first_video_id(runs: &[Value]) -> Option<&str> {
    runs.iter().find_map(|run| {
        run.pointer("/navigationEndpoint/watchEndpoint/videoId")
            .and_then(Value::as_str)
    })
}

fn browse_id(run: &Value) -> Option<&str> {
    run.pointer("/navigationEndpoint/browseEndpoint/browseId")
        .and_then(Value::as_str)
}

fn find_stable_artist(runs: &[Value]) -> Option<ArtistRef> {
    let run = runs
        .iter()
        .find(|run| browse_id(run).is_some_and(|id| id.starts_with("UC")))?;
    Some(ArtistRef {
        id: browse_id(run).map(str::to_owned),
        name: run.get("text")?.as_str()?.to_owned(),
    })
}

fn find_artist(runs: &[Value]) -> Option<ArtistRef> {
    if let Some(artist) = find_stable_artist(runs) {
        return Some(artist);
    }
    let run = runs.iter().find(|run| {
        run.get("text")
            .and_then(Value::as_str)
            .is_some_and(|text| text != " • " && parse_duration_ms(text).is_none())
    })?;
    Some(ArtistRef {
        id: browse_id(run).map(str::to_owned),
        name: run.get("text")?.as_str()?.to_owned(),
    })
}

fn find_album(runs: &[Value]) -> Option<(Option<String>, String)> {
    let run = runs.iter().find(|run| {
        browse_id(run).is_some_and(|id| id.starts_with("MPRE") || id.starts_with("OLAK"))
    })?;
    Some((
        browse_id(run).map(str::to_owned),
        run.get("text")?.as_str()?.to_owned(),
    ))
}

fn last_duration_text(runs: &[Value]) -> Option<&str> {
    runs.iter()
        .rev()
        .filter_map(|run| run.get("text").and_then(Value::as_str))
        .find(|text| parse_duration_ms(text).is_some())
}

fn parse_duration_ms(text: &str) -> Option<u64> {
    let parts = text.trim().split(':');
    let mut seconds = 0_u64;
    let mut count = 0;
    for part in parts {
        seconds = seconds
            .checked_mul(60)?
            .checked_add(part.parse::<u64>().ok()?)?;
        count += 1;
    }
    (2..=3)
        .contains(&count)
        .then(|| seconds.saturating_mul(1_000))
}

fn thumbnail_url(renderer: &Value) -> Option<String> {
    let thumbnails = renderer
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        .or_else(|| {
            renderer.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")
        })
        .or_else(|| {
            renderer
                .pointer("/thumbnailRenderer/playlistVideoThumbnailRenderer/thumbnail/thumbnails")
        })
        .or_else(|| {
            renderer.pointer("/thumbnail/croppedSquareThumbnailRenderer/thumbnail/thumbnails")
        })
        .or_else(|| {
            renderer.pointer("/foregroundThumbnail/musicThumbnailRenderer/thumbnail/thumbnails")
        })
        .or_else(|| renderer.pointer("/background/musicThumbnailRenderer/thumbnail/thumbnails"))
        .or_else(|| renderer.pointer("/thumbnail/thumbnails"))?
        .as_array()?;
    thumbnails
        .iter()
        .filter_map(|thumbnail| thumbnail.get("url").and_then(Value::as_str))
        .next_back()
        .map(str::to_owned)
}

fn incompatible(message: &str) -> ProviderError {
    ProviderError::Incompatible(message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lyrics_tab_and_description_shelf() {
        let next = json!({
            "contents": {"tabRenderer": {
                "title": "Lyrics",
                "endpoint": {"browseEndpoint": {"browseId": "MPLYt_test"}}
            }}
        });
        assert_eq!(parse_lyrics_browse_id(&next).as_deref(), Some("MPLYt_test"));

        let browse = json!({
            "contents": {"musicDescriptionShelfRenderer": {
                "description": {"runs": [{"text": "Line one\n"}, {"text": "Line two"}]},
                "footer": {"simpleText": "Provided by Musixmatch"}
            }}
        });
        let lyrics = parse_lyrics(&browse).unwrap();
        assert_eq!(lyrics.text, "Line one\nLine two");
        assert_eq!(
            lyrics.attribution.as_deref(),
            Some("Provided by Musixmatch")
        );
    }

    #[test]
    fn identifies_typed_lyrics_endpoint_without_tab_title() {
        let next = json!({
            "endpoint": {"browseEndpoint": {
                "browseId": "MPLYt_typed",
                "browseEndpointContextSupportedConfigs": {
                    "browseEndpointContextMusicConfig": {
                        "pageType": "MUSIC_PAGE_TYPE_TRACK_LYRICS"
                    }
                }
            }}
        });
        assert_eq!(
            parse_lyrics_browse_id(&next).as_deref(),
            Some("MPLYt_typed")
        );
    }

    #[test]
    fn parses_search_song_renderer() {
        let fixture = json!({
            "contents": {"section": {"musicResponsiveListItemRenderer": {
                "playlistItemData": {"videoId": "video-1"},
                "flexColumns": [
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                        {"text": "Test Song", "navigationEndpoint": {"watchEndpoint": {"videoId": "video-1"}}}
                    ]}}},
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                        {"text": "Test Artist", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCartist"}}},
                        {"text": " • "},
                        {"text": "Test Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREalbum"}}}
                    ]}}}
                ],
                "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": {"text": {"runs": [{"text": "3:07"}]}}}],
                "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [
                    {"url": "https://img/small"}, {"url": "https://img/large"}
                ]}}}
            }}}
        });

        let songs = parse_search(&fixture, 10).unwrap();
        assert_eq!(songs.len(), 1);
        let song = &songs[0];
        assert_eq!(song.id.as_str(), "video-1");
        assert_eq!(song.title, "Test Song");
        assert_eq!(
            song.artist,
            ArtistRef {
                id: Some("UCartist".into()),
                name: "Test Artist".into()
            }
        );
        assert_eq!(song.album_id.as_deref(), Some("MPREalbum"));
        assert_eq!(song.album_name.as_deref(), Some("Test Album"));
        assert_eq!(song.duration_ms, Some(187_000));
        assert_eq!(song.thumbnail_url.as_deref(), Some("https://img/large"));
    }

    #[test]
    fn parses_automix_endpoint() {
        let fixture = json!({
            "contents": {"automixPreviewVideoRenderer": {
                "content": {"automixPlaylistVideoRenderer": {
                    "navigationEndpoint": {"watchPlaylistEndpoint": {
                        "playlistId": "RDAMVMvideo-1", "params": "wAEB"
                    }}
                }}
            }}
        });

        let endpoint = parse_automix_endpoint(&fixture).unwrap();
        assert_eq!(endpoint.playlist_id, "RDAMVMvideo-1");
        assert_eq!(endpoint.params, "wAEB");
    }

    #[test]
    fn parses_related_playlist_panel_song() {
        let fixture = json!({
            "contents": {"playlistPanelVideoRenderer": {
                "videoId": "related-1",
                "title": {"runs": [{"text": "Related Song"}]},
                "longBylineText": {"runs": [
                    {"text": "Related Artist", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCrelated"}}},
                    {"text": " • "},
                    {"text": "Related Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPRErelated"}}}
                ]},
                "lengthText": {"simpleText": "1:02:03"},
                "thumbnail": {"thumbnails": [{"url": "https://img/related"}]}
            }}
        });

        let songs = parse_related(&fixture, 1).unwrap();
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id.as_str(), "related-1");
        assert_eq!(songs[0].duration_ms, Some(3_723_000));
        assert_eq!(songs[0].album_name.as_deref(), Some("Related Album"));
    }

    #[test]
    fn parses_artist_search_from_responsive_and_two_row_renderers() {
        let fixture = json!({
            "contents": {"section": {"contents": [
                {"musicResponsiveListItemRenderer": {
                    "flexColumns": [
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                            {"text": "Responsive Artist", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCresponsive"}}}
                        ]}}},
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Artist"}]}}}
                    ],
                    "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/responsive"}]}}}
                }},
                {"musicTwoRowItemRenderer": {
                    "title": {"runs": [{"text": "Two Row Artist"}]},
                    "subtitle": {"runs": [{"text": "1.2M subscribers"}]},
                    "navigationEndpoint": {"browseEndpoint": {"browseId": "UCtworow"}},
                    "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/two-row"}]}}}
                }}
            ]}}
        });

        let artists = parse_catalog_artists(&fixture, 10).unwrap();
        assert_eq!(artists.len(), 2);
        assert_eq!(artists[0].id, "UCresponsive");
        assert_eq!(artists[0].name, "Responsive Artist");
        assert_eq!(artists[1].id, "UCtworow");
        assert_eq!(artists[1].subtitle.as_deref(), Some("1.2M subscribers"));
    }

    #[test]
    fn parses_album_and_playlist_collection_cards() {
        let fixture = json!({
            "contents": {"section": {"contents": [
                {"musicTwoRowItemRenderer": {
                    "title": {"runs": [{"text": "Test Album"}]},
                    "subtitle": {"runs": [{"text": "Album • 2025"}]},
                    "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREalbum"}},
                    "thumbnailRenderer": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/album-small"}, {"url": "https://img/album"}]}}}
                }},
                {"musicResponsiveListItemRenderer": {
                    "flexColumns": [
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Test Playlist"}]}}},
                        {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Playlist • 20 songs"}]}}}
                    ],
                    "navigationEndpoint": {"browseEndpoint": {"browseId": "VLPLtest"}},
                    "thumbnailRenderer": {"playlistVideoThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/playlist"}]}}}
                }}
            ]}}
        });

        let albums = parse_catalog_collections(&fixture, &["MPRE", "OLAK"], "album", 10).unwrap();
        let playlists = parse_catalog_collections(&fixture, &["VL", "PL"], "playlist", 10).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].id, "MPREalbum");
        assert_eq!(albums[0].kind, "album");
        assert_eq!(
            albums[0].thumbnail_url.as_deref(),
            Some("https://img/album")
        );
        assert_eq!(playlists.len(), 1);
        assert_eq!(playlists[0].id, "VLPLtest");
        assert_eq!(playlists[0].title, "Test Playlist");
        assert_eq!(
            playlists[0].thumbnail_url.as_deref(),
            Some("https://img/playlist")
        );
    }

    #[test]
    fn parses_collection_tracks() {
        let fixture = json!({
            "contents": {"musicResponsiveListItemRenderer": {
                "playlistItemData": {"videoId": "collection-video"},
                "flexColumns": [
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Collection Song"}]}}},
                    {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                        {"text": "Collection Artist", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCcollection"}}},
                        {"text": " • "},
                        {"text": "Collection Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREcollection"}}}
                    ]}}}
                ]
            }}
        });

        let songs = parse_collection_songs(&fixture).unwrap();
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id.as_str(), "collection-video");
        assert_eq!(songs[0].artist.name, "Collection Artist");
    }

    #[test]
    fn parses_artist_page_top_songs_and_releases() {
        let fixture = json!({
            "contents": {"artist": {
                "musicImmersiveHeaderRenderer": {
                    "title": {"runs": [{"text": "Page Artist"}]},
                    "subtitle": {"runs": [{"text": "2M subscribers"}]},
                    "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://img/artist"}]}}}
                },
                "sections": [
                    {"musicShelfRenderer": {
                        "title": {"runs": [{"text": "Top songs"}]},
                        "contents": [{"musicResponsiveListItemRenderer": {
                            "playlistItemData": {"videoId": "top-video"},
                            "flexColumns": [
                                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Top Song"}]}}},
                                {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                                    {"text": "Song"}, {"text": " • "},
                                    {"text": "Top Album", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREtop"}}}
                                ]}}}
                            ],
                            "fixedColumns": [{"musicResponsiveListItemFixedColumnRenderer": {"text": {"runs": [{"text": "3:10"}]}}}]
                        }}]
                    }},
                    {"musicCarouselShelfRenderer": {
                        "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Latest releases"}]}}},
                        "contents": [{"musicTwoRowItemRenderer": {
                            "title": {"runs": [{"text": "Newest Album"}]},
                            "subtitle": {"runs": [{"text": "Album • 2026"}]},
                            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPRElatest"}}
                        }}]
                    }},
                    {"musicCarouselShelfRenderer": {
                        "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Singles & EPs"}]}}},
                        "contents": [{"musicTwoRowItemRenderer": {
                            "title": {"runs": [{"text": "New EP"}]},
                            "subtitle": {"runs": [{"text": "EP • 2025"}]},
                            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREep"}}
                        }}]
                    }},
                    {"musicCarouselShelfRenderer": {
                        "header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Playlists by Page Artist"}]}}},
                        "contents": [{"musicTwoRowItemRenderer": {
                            "title": {"runs": [{"text": "Artist Picks"}]},
                            "subtitle": {"runs": [{"text": "Page Artist"}]},
                            "navigationEndpoint": {"browseEndpoint": {"browseId": "VLartistpicks"}}
                        }}]
                    }}
                ]
            }}
        });

        let page = parse_artist_page(&fixture, "UCpage").unwrap();
        assert_eq!(page.artist.id, "UCpage");
        assert_eq!(page.artist.name, "Page Artist");
        assert_eq!(page.top_songs.len(), 1);
        assert_eq!(page.top_songs[0].artist.id.as_deref(), Some("UCpage"));
        assert_eq!(page.top_songs[0].artist.name, "Page Artist");
        assert_eq!(page.latest_releases.len(), 1);
        assert_eq!(page.latest_releases[0].id, "MPRElatest");
        assert_eq!(page.latest_releases[0].kind, "album");
        assert_eq!(page.singles.len(), 1);
        assert_eq!(page.singles[0].kind, "ep");
        assert!(page.albums.is_empty());
        assert_eq!(page.playlists.len(), 1);
        assert_eq!(page.playlists[0].id, "VLartistpicks");
    }

    #[test]
    fn playback_quality_selects_bitrate_and_reads_expiry() {
        let fixture = json!({
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [
                {"mimeType": "audio/webm; codecs=\"opus\"", "bitrate": 256000, "url": "https://audio/webm?expire=10"},
                {"mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 48000, "url": "https://audio/data-saver?expire=1710000000"},
                {"mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 128000, "url": "https://audio/low?expire=1720000000"},
                {"mimeType": "audio/mp4; codecs=\"mp4a.40.2\"", "bitrate": 192000, "url": "https://audio/high?expire=1720000001"},
                {"mimeType": "video/mp4", "bitrate": 999999, "url": "https://video"}
            ]}
        });

        let low = parse_playback(
            &fixture,
            AudioQuality::Low,
            PlaybackRequestProfile::VisionOs,
            None,
        )
        .unwrap();
        assert_eq!(low.url, "https://audio/data-saver?expire=1710000000");
        let medium = parse_playback(
            &fixture,
            AudioQuality::Medium,
            PlaybackRequestProfile::VisionOs,
            None,
        )
        .unwrap();
        assert_eq!(medium.url, "https://audio/low?expire=1720000000");
        let high = parse_playback(
            &fixture,
            AudioQuality::High,
            PlaybackRequestProfile::VisionOs,
            None,
        )
        .unwrap();
        assert_eq!(high.url, "https://audio/high?expire=1720000001");
        assert_eq!(high.mime_type, "audio/mp4; codecs=\"mp4a.40.2\"");
        assert_eq!(high.expires_at_ms, Some(1_720_000_001_000));
    }

    #[test]
    fn playback_nonce_is_added_to_the_selected_stream() {
        let fixture = json!({
            "playabilityStatus": {"status": "OK"},
            "streamingData": {"adaptiveFormats": [{
                "mimeType": "audio/mp4; codecs=\"mp4a.40.2\"",
                "bitrate": 128000,
                "url": "https://audio.example/videoplayback?expire=1720000000&c=VISIONOS"
            }]}
        });

        let source = parse_playback(
            &fixture,
            AudioQuality::High,
            PlaybackRequestProfile::VisionOs,
            Some("0123456789abcdef"),
        )
        .unwrap();

        assert!(source.url.contains("cpn=0123456789abcdef"));
        assert_eq!(source.request_profile, PlaybackRequestProfile::VisionOs);
    }

    #[test]
    fn playback_refresh_rotates_clients() {
        assert_eq!(
            PlaybackClient::VisionOs.next(),
            PlaybackClient::AndroidVr143
        );
        assert_eq!(
            PlaybackClient::AndroidVr143.next(),
            PlaybackClient::VisionOs
        );
    }

    #[test]
    fn visionos_context_contains_current_profile_and_visitor_data() {
        let context = playback_context(PlaybackClient::VisionOs, Some("visitor-token"));
        assert_eq!(
            context
                .pointer("/client/clientName")
                .and_then(Value::as_str),
            Some("VISIONOS")
        );
        assert_eq!(
            context
                .pointer("/client/clientVersion")
                .and_then(Value::as_str),
            Some(VISIONOS_CLIENT_VERSION)
        );
        assert_eq!(
            context
                .pointer("/client/visitorData")
                .and_then(Value::as_str),
            Some("visitor-token")
        );
    }

    #[test]
    fn playback_reports_unplayable_status() {
        let fixture = json!({
            "playabilityStatus": {"status": "UNPLAYABLE", "reason": "Not available"}
        });

        assert!(matches!(
            parse_playback(
                &fixture,
                AudioQuality::High,
                PlaybackRequestProfile::VisionOs,
                None,
            ),
            Err(ProviderError::Unplayable(reason)) if reason == "Not available"
        ));
    }

    #[test]
    fn playback_classifies_login_and_bot_checks() {
        for fixture in [
            json!({"playabilityStatus": {"status": "LOGIN_REQUIRED", "reason": "Please sign in"}}),
            json!({"playabilityStatus": {"status": "UNPLAYABLE", "reason": "Sign in to confirm you're not a bot"}}),
        ] {
            assert!(matches!(
                parse_playback(
                    &fixture,
                    AudioQuality::High,
                    PlaybackRequestProfile::VisionOs,
                    None,
                ),
                Err(ProviderError::VerificationRequired(_))
            ));
        }
    }

    #[test]
    fn parses_youtube_netscape_cookies_and_redacts_debug_output() {
        let auth = YouTubeAuth::from_netscape(
            "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\ttest-sapisid\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsecret-sid\n.example.com\tTRUE\t/\tTRUE\t0\tSAPISID\tignored\n",
        )
        .unwrap();

        assert!(auth.cookie_header().contains("SAPISID=test-sapisid"));
        assert!(auth.cookie_header().contains("SID=secret-sid"));
        assert!(!auth.cookie_header().contains("ignored"));
        assert_eq!(
            auth.authorization_at(MUSIC_ORIGIN, 1_700_000_000),
            "SAPISIDHASH 1700000000_17d748c166afd876ceb872a291e5befdca771528"
        );
        assert_eq!(format!("{auth:?}"), "YouTubeAuth { configured: true }");
    }

    #[test]
    fn rejects_cookie_exports_without_a_signed_in_youtube_session() {
        let result = YouTubeAuth::from_netscape(
            ".youtube.com\tTRUE\t/\tTRUE\t0\tVISITOR_INFO1_LIVE\tvisitor\n",
        );
        assert!(result.is_err());
    }
}
