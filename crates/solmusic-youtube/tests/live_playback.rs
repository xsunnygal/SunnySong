use reqwest::{header, Client, StatusCode};
use solmusic_application::{domain::SongId, MusicProvider, PlaybackRequestProfile, PlaybackSource};
use solmusic_youtube::YouTubeMusicProvider;

const DEFAULT_TEST_VIDEO_ID: &str = "4whD6uAryMs";
const CHUNK_LENGTH: u64 = 512 * 1024;
const VISIONOS_USER_AGENT: &str =
    "com.google.visionos.youtube/1.04(RealityDevice17,1; U; CPU visionOS 26_6_0 like Mac OS X; US)";
const ANDROID_VR_USER_AGENT: &str = "com.google.android.apps.youtube.vr.oculus/1.43.32 (Linux; U; Android 12; en_US; Quest 3; Build/SQ3A.220605.009.A1)";
const WEB_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/131 Safari/537.36";

/// Exercises the real Innertube resolver and reads the selected audio stream from byte zero to
/// its declared end. This is ignored during normal CI because it depends on YouTube and network
/// availability; run it explicitly before packaging a release build.
#[tokio::test]
#[ignore = "live YouTube integration test; run with --ignored"]
async fn resolved_audio_stream_loads_from_start_to_finish() {
    let video_id = std::env::var("SUNNYSONG_LIVE_YOUTUBE_VIDEO_ID")
        .unwrap_or_else(|_| DEFAULT_TEST_VIDEO_ID.to_owned());
    let song_id = SongId::new(video_id).expect("live test video id cannot be blank");
    let provider = YouTubeMusicProvider::new().expect("provider should initialize");
    let source = provider
        .resolve_playback(&song_id)
        .await
        .expect("live playback source should resolve");

    assert!(source.url.starts_with("https://"));
    assert!(source.mime_type.starts_with("audio/"));
    if source.request_profile == PlaybackRequestProfile::VisionOs {
        assert!(source.url.contains("cpn="));
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .unwrap();
    let mut loaded = 0_u64;
    let mut total = None;
    let mut chunk_count = 0_usize;

    while total.is_none_or(|length| loaded < length) {
        let end = total.map_or(loaded + CHUNK_LENGTH - 1, |length| {
            (loaded + CHUNK_LENGTH - 1).min(length - 1)
        });
        let response = client
            .get(&source.url)
            .header(header::USER_AGENT, user_agent(&source))
            .header(header::RANGE, format!("bytes={loaded}-{end}"))
            .send()
            .await
            .expect("media range request should complete");
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);

        let content_range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .expect("media response should declare Content-Range");
        let reported_total = content_range
            .rsplit_once('/')
            .and_then(|(_, value)| value.parse::<u64>().ok())
            .expect("Content-Range should contain a numeric total");
        assert!(reported_total > 0);
        assert_eq!(*total.get_or_insert(reported_total), reported_total);

        let bytes = response
            .bytes()
            .await
            .expect("media response body should be readable");
        assert!(!bytes.is_empty(), "media chunk at {loaded} was empty");
        assert!(bytes.len() as u64 <= end - loaded + 1);
        loaded += bytes.len() as u64;
        chunk_count += 1;
    }

    let total = total.expect("at least one range should have loaded");
    assert_eq!(
        loaded, total,
        "stream stopped before its declared final byte"
    );
    assert!(chunk_count >= 2, "test stream should span multiple ranges");
}

fn user_agent(source: &PlaybackSource) -> &'static str {
    match source.request_profile {
        PlaybackRequestProfile::VisionOs => VISIONOS_USER_AGENT,
        PlaybackRequestProfile::AndroidVr => ANDROID_VR_USER_AGENT,
        PlaybackRequestProfile::Web => WEB_USER_AGENT,
    }
}
