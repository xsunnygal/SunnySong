use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use solmusic_application::{
    domain::SongId, AudioQuality, PlaybackRequestProfile, PlaybackSource, ProviderError,
};
use tauri::{AppHandle, Manager};
use tokio::{process::Command, sync::Mutex as AsyncMutex};
use tracing::{info, warn};
use uuid::Uuid;

use solmusic_youtube::YouTubeAuthState;

pub struct PlaybackResolver {
    executable: PathBuf,
    cache: Mutex<HashMap<String, PlaybackSource>>,
    resolve_gates: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    audio_quality: AtomicU8,
    auth: YouTubeAuthState,
}

impl PlaybackResolver {
    pub fn new(app: &AppHandle, auth: YouTubeAuthState) -> Result<Self, String> {
        let executable = resolver_candidates(app)
            .into_iter()
            .find(|path| path.is_file())
            .ok_or_else(|| "bundled YouTube playback resolver was not found".to_owned())?;
        Ok(Self {
            executable,
            cache: Mutex::new(HashMap::new()),
            resolve_gates: Mutex::new(HashMap::new()),
            audio_quality: AtomicU8::new(audio_quality_value(AudioQuality::High)),
            auth,
        })
    }

    pub fn set_audio_quality(&self, quality: AudioQuality) {
        self.audio_quality
            .store(audio_quality_value(quality), Ordering::Relaxed);
        self.cache.lock().expect("playback cache poisoned").clear();
    }

    pub async fn resolve(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        self.resolve_internal(song_id, false).await
    }

    pub async fn resolve_fresh(&self, song_id: &SongId) -> Result<PlaybackSource, ProviderError> {
        self.resolve_internal(song_id, true).await
    }

    async fn resolve_internal(
        &self,
        song_id: &SongId,
        force_refresh: bool,
    ) -> Result<PlaybackSource, ProviderError> {
        if !force_refresh {
            if let Some(source) = self.cached_source(song_id) {
                info!(
                    category = "YOUTUBE",
                    event = "playback_cache_hit",
                    track_id = song_id.as_str()
                );
                return Ok(source);
            }
        }

        let gate = self
            .resolve_gates
            .lock()
            .expect("playback resolve gates poisoned")
            .entry(song_id.as_str().to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        let _gate = gate.lock().await;
        if !force_refresh {
            if let Some(source) = self.cached_source(song_id) {
                info!(
                    category = "YOUTUBE",
                    event = "playback_cache_hit_after_wait",
                    track_id = song_id.as_str()
                );
                return Ok(source);
            }
        }

        let started = Instant::now();
        let video_id = song_id.as_str();
        if !is_youtube_video_id(video_id) {
            return Err(ProviderError::Unplayable("invalid YouTube video ID".into()));
        }
        let watch_url = format!("https://www.youtube.com/watch?v={video_id}");
        let format = audio_format(self.audio_quality.load(Ordering::Relaxed));
        let cookie_file = self
            .auth
            .snapshot()
            .map(|auth| SecureCookieFile::create(auth.netscape_cookies()))
            .transpose()
            .map_err(ProviderError::Network)?;
        let mut command = Command::new(&self.executable);
        command.args([
            "--ignore-config",
            "--no-playlist",
            "--no-warnings",
            "--quiet",
            "--socket-timeout",
            "15",
            "--retries",
            "1",
            "--extractor-args",
            "youtube:player_client=default,-android_vr,-android_reel",
            "-f",
            format,
            "--get-url",
        ]);
        if let Some(cookie_file) = cookie_file.as_ref() {
            command.arg("--cookies").arg(cookie_file.path());
        }
        let output = command
            .arg(&watch_url)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| {
                ProviderError::Network(format!(
                    "could not start YouTube playback resolver: {error}"
                ))
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(category = "YOUTUBE", event = "playback_resolve_failed", track_id = video_id, duration_ms = started.elapsed().as_millis() as u64, exit_status = %output.status);
            if message_requires_verification(&stderr) {
                return Err(ProviderError::VerificationRequired(
                    "connect or replace your YouTube Music cookies in Settings, then retry".into(),
                ));
            }
            let detail = if cookie_file.is_some() {
                "authenticated resolver request failed; replace the saved cookies or retry later"
            } else {
                "resolver request failed; YouTube Music may require account verification"
            };
            return Err(ProviderError::Unplayable(detail.into()));
        }

        let stdout = String::from_utf8(output.stdout).map_err(|_| {
            ProviderError::Incompatible("YouTube playback resolver returned invalid text".into())
        })?;
        let mut urls = stdout
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty());
        let url = urls
            .next()
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| {
                ProviderError::Incompatible(
                    "YouTube playback resolver returned no HTTPS audio URL".into(),
                )
            })?;
        if urls.next().is_some() {
            return Err(ProviderError::Incompatible(
                "YouTube playback resolver returned multiple media URLs".into(),
            ));
        }
        info!(
            category = "YOUTUBE",
            event = "playback_resolved",
            track_id = video_id,
            duration_ms = started.elapsed().as_millis() as u64
        );
        let source = PlaybackSource {
            expires_at_ms: playback_expiry_ms(url),
            url: url.to_owned(),
            mime_type: "audio/mp4; codecs=\"mp4a.40.2\"".into(),
            local_path: None,
            request_profile: PlaybackRequestProfile::Web,
        };
        if source_is_fresh(&source) {
            self.cache
                .lock()
                .expect("playback cache poisoned")
                .insert(video_id.to_owned(), source.clone());
        }
        Ok(source)
    }

    fn cached_source(&self, song_id: &SongId) -> Option<PlaybackSource> {
        let mut cache = self.cache.lock().expect("playback cache poisoned");
        let source = cache.get(song_id.as_str()).cloned()?;
        if source_is_fresh(&source) {
            Some(source)
        } else {
            cache.remove(song_id.as_str());
            None
        }
    }
}

struct SecureCookieFile {
    path: PathBuf,
}

impl SecureCookieFile {
    fn create(contents: &str) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "solmusic-youtube-cookies-{}.txt",
            Uuid::new_v4().simple()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| format!("could not create temporary cookie file: {error}"))?;
        if let Err(error) = file.write_all(contents.as_bytes()) {
            let _ = std::fs::remove_file(&path);
            return Err(format!("could not write temporary cookie file: {error}"));
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SecureCookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn message_requires_verification(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("sign in to confirm")
        || normalized.contains("not a bot")
        || normalized.contains("login required")
}

fn audio_quality_value(quality: AudioQuality) -> u8 {
    match quality {
        AudioQuality::Low => 0,
        AudioQuality::Medium => 1,
        AudioQuality::High => 2,
    }
}

fn audio_format(value: u8) -> &'static str {
    match value {
        0 => "bestaudio[ext=m4a][abr<=64]/bestaudio[abr<=64]/worstaudio",
        1 => "bestaudio[ext=m4a][abr<=160]/bestaudio[abr<=160]/bestaudio[ext=m4a]/bestaudio",
        _ => "bestaudio[ext=m4a]/bestaudio",
    }
}

fn source_is_fresh(source: &PlaybackSource) -> bool {
    let Some(expires_at_ms) = source.expires_at_ms else {
        return false;
    };
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(i64::MAX);
    expires_at_ms > now_ms.saturating_add(120_000)
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

fn is_youtube_video_id(value: &str) -> bool {
    value.len() == 11
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn resolver_candidates(app: &AppHandle) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(sidecar_file_name()));
    }
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            candidates.push(parent.join(sidecar_file_name()));
        }
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(sidecar_build_file_name()),
    );
    candidates
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn sidecar_build_file_name() -> &'static str {
    "yt-dlp-x86_64-unknown-linux-gnu"
}

#[cfg(target_os = "windows")]
fn sidecar_build_file_name() -> &'static str {
    "yt-dlp-x86_64-pc-windows-msvc.exe"
}

#[cfg(target_os = "macos")]
fn sidecar_build_file_name() -> &'static str {
    "yt-dlp-aarch64-apple-darwin"
}

#[cfg(target_os = "windows")]
fn sidecar_file_name() -> &'static str {
    "yt-dlp.exe"
}

#[cfg(not(target_os = "windows"))]
fn sidecar_file_name() -> &'static str {
    "yt-dlp"
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use solmusic_application::{PlaybackRequestProfile, PlaybackSource};

    use super::{
        audio_format, is_youtube_video_id, message_requires_verification, source_is_fresh,
        SecureCookieFile,
    };

    #[test]
    fn accepts_canonical_youtube_video_ids() {
        assert!(is_youtube_video_id("4whD6uAryMs"));
        assert!(is_youtube_video_id("abc_DEF-123"));
    }

    #[test]
    fn rejects_noncanonical_or_injectable_ids() {
        assert!(!is_youtube_video_id(""));
        assert!(!is_youtube_video_id("too-short"));
        assert!(!is_youtube_video_id("4whD6uAryMs&list=bad"));
        assert!(!is_youtube_video_id("../bad-id!!"));
    }

    #[test]
    fn maps_audio_quality_to_bounded_format_preferences() {
        assert!(audio_format(0).contains("abr<=64"));
        assert!(audio_format(1).contains("abr<=160"));
        assert_eq!(audio_format(2), "bestaudio[ext=m4a]/bestaudio");
    }

    #[test]
    fn classifies_resolver_verification_messages() {
        assert!(message_requires_verification(
            "ERROR: Sign in to confirm you're not a bot"
        ));
        assert!(!message_requires_verification("ERROR: Video unavailable"));
    }

    #[test]
    fn temporary_cookie_file_is_deleted_on_drop() {
        let path = {
            let file = SecureCookieFile::create("# Netscape HTTP Cookie File\n").unwrap();
            let path = file.path().to_owned();
            assert!(path.is_file());
            path
        };
        assert!(!path.exists());
    }

    #[test]
    fn cache_requires_more_than_two_minutes_of_source_lifetime() {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let source = |expires_at_ms| PlaybackSource {
            url: "https://example.test/audio".into(),
            mime_type: "audio/mp4".into(),
            expires_at_ms,
            local_path: None,
            request_profile: PlaybackRequestProfile::Web,
        };
        assert!(source_is_fresh(&source(Some(now_ms + 180_000))));
        assert!(!source_is_fresh(&source(Some(now_ms + 60_000))));
        assert!(!source_is_fresh(&source(None)));
    }
}
