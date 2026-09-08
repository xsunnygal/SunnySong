use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    sync::{Arc, Mutex},
};

use mpris_server::{Metadata, PlaybackStatus, Player, Time, TrackId};
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

use crate::MediaSessionUpdate;

const MEDIA_CONTROL_EVENT: &str = "media-control";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaControl {
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    position_ms: Option<u64>,
}

impl MediaControl {
    fn action(action: &'static str) -> Self {
        Self {
            action,
            position_ms: None,
        }
    }

    fn seek(position_ms: u64) -> Self {
        Self {
            action: "seek",
            position_ms: Some(position_ms),
        }
    }
}

type Listeners = Arc<Mutex<HashMap<u32, Channel<MediaControl>>>>;

pub(crate) struct MediaSession {
    updates: mpsc::UnboundedSender<MediaSessionUpdate>,
    listeners: Listeners,
}

impl MediaSession {
    pub(crate) fn new() -> Self {
        let (updates, receiver) = mpsc::unbounded_channel();
        let listeners = Arc::new(Mutex::new(HashMap::new()));
        let worker_listeners = listeners.clone();

        if let Err(error) = std::thread::Builder::new()
            .name("sunnysong-mpris".into())
            .spawn(move || run_worker(receiver, worker_listeners))
        {
            eprintln!("failed to start SunnySong MPRIS worker: {error}");
        }

        Self { updates, listeners }
    }

    pub(crate) fn update(&self, update: MediaSessionUpdate) -> crate::Result<()> {
        self.updates
            .send(update)
            .map_err(|_| crate::Error::MediaSessionUnavailable)
    }

    pub(crate) fn register_listener(
        &self,
        event: &str,
        handler: Channel<MediaControl>,
    ) -> std::result::Result<(), String> {
        if event != MEDIA_CONTROL_EVENT {
            return Err(format!("unsupported solmusic-storage event: {event}"));
        }

        self.listeners
            .lock()
            .map_err(|_| "media-control listener registry is unavailable".to_string())?
            .insert(handler.id(), handler);
        Ok(())
    }

    pub(crate) fn remove_listener(
        &self,
        event: &str,
        channel_id: u32,
    ) -> std::result::Result<(), String> {
        if event != MEDIA_CONTROL_EVENT {
            return Err(format!("unsupported solmusic-storage event: {event}"));
        }

        self.listeners
            .lock()
            .map_err(|_| "media-control listener registry is unavailable".to_string())?
            .remove(&channel_id);
        Ok(())
    }
}

fn run_worker(receiver: mpsc::UnboundedReceiver<MediaSessionUpdate>, listeners: Listeners) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("failed to create SunnySong MPRIS runtime: {error}");
            return;
        }
    };

    let local = tokio::task::LocalSet::new();
    if let Err(error) = local.block_on(&runtime, run_mpris(receiver, listeners)) {
        eprintln!("SunnySong MPRIS integration stopped: {error}");
    }
}

async fn run_mpris(
    mut receiver: mpsc::UnboundedReceiver<MediaSessionUpdate>,
    listeners: Listeners,
) -> mpris_server::zbus::Result<()> {
    let player = Player::builder("SunnySong")
        .identity("SunnySong")
        .can_control(true)
        .build()
        .await?;

    connect_controls(&player, listeners);

    let mut server_task = tokio::task::spawn_local(player.run());
    loop {
        tokio::select! {
            result = &mut server_task => {
                return result.map_err(|error| mpris_server::zbus::Error::Failure(error.to_string()));
            }
            update = receiver.recv() => {
                let Some(update) = update else {
                    return Ok(());
                };
                apply_update(&player, &update).await?;
            }
        }
    }
}

fn connect_controls(player: &Player, listeners: Listeners) {
    connect_action(player, &listeners, |player| {
        if player.playback_status() == PlaybackStatus::Playing {
            MediaControl::action("pause")
        } else {
            MediaControl::action("play")
        }
    });

    let target = listeners.clone();
    player.connect_play(move |_| dispatch(&target, MediaControl::action("play")));

    let target = listeners.clone();
    player.connect_pause(move |_| dispatch(&target, MediaControl::action("pause")));

    let target = listeners.clone();
    player.connect_next(move |_| dispatch(&target, MediaControl::action("next")));

    let target = listeners.clone();
    player.connect_previous(move |_| dispatch(&target, MediaControl::action("previous")));

    let target = listeners.clone();
    player.connect_stop(move |_| dispatch(&target, MediaControl::action("stop")));

    let target = listeners.clone();
    player.connect_seek(move |player, offset| {
        let duration = player.metadata().length();
        let position = seek_target(player.position(), offset, duration);
        dispatch(&target, MediaControl::seek(position));
    });

    player.connect_set_position(move |player, track_id, position| {
        let metadata = player.metadata();
        if metadata.trackid().as_ref() != Some(track_id) {
            return;
        }
        let position = clamp_position(position, metadata.length());
        drop(metadata);
        dispatch(&listeners, MediaControl::seek(position));
    });
}

fn connect_action(
    player: &Player,
    listeners: &Listeners,
    event: impl Fn(&Player) -> MediaControl + 'static,
) {
    let listeners = listeners.clone();
    player.connect_play_pause(move |player| dispatch(&listeners, event(player)));
}

fn dispatch(listeners: &Listeners, event: MediaControl) {
    let Ok(mut listeners) = listeners.lock() else {
        return;
    };
    listeners.retain(|_, handler| handler.send(event.clone()).is_ok());
}

async fn apply_update(
    player: &Player,
    update: &MediaSessionUpdate,
) -> mpris_server::zbus::Result<()> {
    if !update.active {
        player.set_metadata(Metadata::new()).await?;
        player.set_position(Time::ZERO);
        player.set_can_go_previous(false).await?;
        player.set_can_go_next(false).await?;
        player.set_can_play(false).await?;
        player.set_can_pause(false).await?;
        player.set_can_seek(false).await?;
        player.set_playback_status(PlaybackStatus::Stopped).await?;
        return Ok(());
    }

    player.set_metadata(metadata_for(update)).await?;
    player.set_position(time_from_millis(update.position_ms));
    player.set_can_go_previous(update.can_go_previous).await?;
    player.set_can_go_next(update.can_go_next).await?;
    player.set_can_play(true).await?;
    player.set_can_pause(true).await?;
    player.set_can_seek(update.duration_ms > 0).await?;
    player
        .set_playback_status(if update.playing {
            PlaybackStatus::Playing
        } else {
            PlaybackStatus::Paused
        })
        .await?;
    Ok(())
}

fn metadata_for(update: &MediaSessionUpdate) -> Metadata {
    let mut metadata = Metadata::new();
    metadata.set_trackid(Some(track_id_for(update)));
    metadata.set_title(Some(update.title.clone()));
    metadata.set_artist(Some([update.artist.clone()]));
    metadata.set_album(update.album.clone());
    metadata.set_art_url(update.artwork_url.clone());
    if update.duration_ms > 0 {
        metadata.set_length(Some(time_from_millis(update.duration_ms)));
    }
    metadata
}

fn track_id_for(update: &MediaSessionUpdate) -> TrackId {
    let id = update
        .current_item
        .as_ref()
        .map(|item| item.id.as_str())
        .or_else(|| {
            update
                .current_index
                .and_then(|index| update.queue.get(index))
                .map(|item| item.id.as_str())
        })
        .unwrap_or(&update.title);
    let mut hasher = DefaultHasher::new();
    id.hash(&mut hasher);
    TrackId::try_from(format!("/app/solmusic/track/{:016x}", hasher.finish()))
        .expect("hashed track IDs are valid D-Bus object paths")
}

fn time_from_millis(milliseconds: u64) -> Time {
    Time::from_millis(milliseconds.min(i64::MAX as u64 / 1_000) as i64)
}

fn seek_target(position: Time, offset: Time, duration: Option<Time>) -> u64 {
    clamp_position(position.saturating_add(offset), duration)
}

fn clamp_position(position: Time, duration: Option<Time>) -> u64 {
    let upper_bound = duration.unwrap_or(Time::MAX).as_millis().max(0);
    position.as_millis().clamp(0, upper_bound) as u64
}

#[cfg(test)]
mod tests {
    use super::{clamp_position, metadata_for, seek_target, MediaControl};
    use crate::{MediaSessionItem, MediaSessionUpdate};
    use mpris_server::Time;

    #[test]
    fn relative_seek_is_converted_to_a_clamped_absolute_position() {
        assert_eq!(
            seek_target(
                Time::from_millis(9_000),
                Time::from_millis(2_000),
                Some(Time::from_millis(10_000)),
            ),
            10_000
        );
        assert_eq!(
            seek_target(
                Time::from_millis(1_000),
                Time::from_millis(-2_000),
                Some(Time::from_millis(10_000)),
            ),
            0
        );
    }

    #[test]
    fn absolute_seek_is_clamped_to_duration() {
        assert_eq!(
            clamp_position(Time::from_millis(12_000), Some(Time::from_millis(8_000))),
            8_000
        );
    }

    #[test]
    fn metadata_includes_track_details_and_artwork() {
        let item = MediaSessionItem {
            id: "catalog-id".into(),
            title: "Track".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            artwork_url: Some("https://images.example/art.jpg".into()),
            duration_ms: 120_000,
        };
        let update = MediaSessionUpdate {
            active: true,
            title: item.title.clone(),
            artist: item.artist.clone(),
            album: item.album.clone(),
            artwork_url: item.artwork_url.clone(),
            playing: true,
            position_ms: 5_000,
            duration_ms: item.duration_ms,
            can_go_previous: true,
            can_go_next: true,
            queue: vec![item.clone()],
            current_item: Some(item),
            current_index: Some(0),
        };

        let metadata = metadata_for(&update);
        assert_eq!(metadata.title(), Some("Track"));
        assert_eq!(metadata.artist(), Some(vec!["Artist".to_string()]));
        assert_eq!(metadata.album(), Some("Album"));
        assert_eq!(
            metadata.art_url().as_deref(),
            Some("https://images.example/art.jpg")
        );
        assert_eq!(metadata.length(), Some(Time::from_millis(120_000)));
        assert!(metadata.trackid().is_some());
    }

    #[test]
    fn seek_event_uses_frontend_payload_shape() {
        let value = serde_json::to_value(MediaControl::seek(42)).unwrap();
        assert_eq!(value["action"], "seek");
        assert_eq!(value["positionMs"], 42);
    }
}
