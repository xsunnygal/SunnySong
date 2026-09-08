use serde::{Deserialize, Serialize};
use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(mobile)]
mod mobile;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[cfg(mobile)]
    #[error(transparent)]
    Plugin(#[from] tauri::plugin::mobile::PluginInvokeError),
    #[cfg(desktop)]
    #[error("native storage selection is only available on mobile")]
    Unsupported,
    #[cfg(target_os = "linux")]
    #[error("the Linux media-session worker is unavailable")]
    MediaSessionUnavailable,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageDirectory {
    pub uri: String,
    pub display_name: String,
    pub can_write: bool,
    pub persisted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishAudioRequest {
    pub tree_uri: String,
    pub display_name: String,
    pub mime_type: String,
    pub source_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishAudioResponse {
    pub uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretRequest {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretKeyRequest {
    pub key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretResponse {
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSessionUpdate {
    pub active: bool,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub artwork_url: Option<String>,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub can_go_previous: bool,
    pub can_go_next: bool,
    #[serde(default)]
    pub queue: Vec<MediaSessionItem>,
    #[serde(default)]
    pub current_item: Option<MediaSessionItem>,
    #[serde(default)]
    pub current_index: Option<usize>,
}

pub struct Storage<R: Runtime> {
    #[cfg(mobile)]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(target_os = "linux")]
    media_session: linux::MediaSession,
    #[cfg(desktop)]
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<R: Runtime> Storage<R> {
    #[cfg(mobile)]
    pub fn pick_directory(&self) -> Result<StorageDirectory> {
        Ok(self.handle.run_mobile_plugin("pickDirectory", ())?)
    }

    #[cfg(desktop)]
    pub fn pick_directory(&self) -> Result<StorageDirectory> {
        Err(Error::Unsupported)
    }

    #[cfg(mobile)]
    pub fn publish_audio(&self, request: PublishAudioRequest) -> Result<PublishAudioResponse> {
        Ok(self.handle.run_mobile_plugin("publishAudio", request)?)
    }

    #[cfg(desktop)]
    pub fn publish_audio(&self, _request: PublishAudioRequest) -> Result<PublishAudioResponse> {
        Err(Error::Unsupported)
    }

    #[cfg(mobile)]
    pub fn store_secret(&self, key: String, value: String) -> Result<()> {
        self.handle
            .run_mobile_plugin::<()>("storeSecret", SecretRequest { key, value })?;
        Ok(())
    }

    #[cfg(desktop)]
    pub fn store_secret(&self, _key: String, _value: String) -> Result<()> {
        Err(Error::Unsupported)
    }

    #[cfg(mobile)]
    pub fn get_secret(&self, key: String) -> Result<Option<String>> {
        let response: SecretResponse = self
            .handle
            .run_mobile_plugin("getSecret", SecretKeyRequest { key })?;
        Ok(response.value)
    }

    #[cfg(desktop)]
    pub fn get_secret(&self, _key: String) -> Result<Option<String>> {
        Err(Error::Unsupported)
    }

    #[cfg(mobile)]
    pub fn delete_secret(&self, key: String) -> Result<()> {
        self.handle
            .run_mobile_plugin::<()>("deleteSecret", SecretKeyRequest { key })?;
        Ok(())
    }

    #[cfg(desktop)]
    pub fn delete_secret(&self, _key: String) -> Result<()> {
        Err(Error::Unsupported)
    }

    #[cfg(mobile)]
    pub fn update_media_session(&self, update: MediaSessionUpdate) -> Result<()> {
        self.handle
            .run_mobile_plugin::<()>("updateMediaSession", update)?;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub fn update_media_session(&self, update: MediaSessionUpdate) -> Result<()> {
        self.media_session.update(update)
    }

    #[cfg(all(desktop, not(target_os = "linux")))]
    pub fn update_media_session(&self, _update: MediaSessionUpdate) -> Result<()> {
        Ok(())
    }

    #[cfg(mobile)]
    pub fn background_app(&self) -> Result<()> {
        self.handle.run_mobile_plugin::<()>("backgroundApp", ())?;
        Ok(())
    }

    #[cfg(desktop)]
    pub fn background_app(&self) -> Result<()> {
        Ok(())
    }
}

pub trait StorageExt<R: Runtime> {
    fn solmusic_storage(&self) -> &Storage<R>;
}

impl<R: Runtime, T: Manager<R>> StorageExt<R> for T {
    fn solmusic_storage(&self) -> &Storage<R> {
        self.state::<Storage<R>>().inner()
    }
}

#[cfg(target_os = "linux")]
#[tauri::command]
fn register_listener<R: Runtime>(
    app: tauri::AppHandle<R>,
    event: String,
    handler: tauri::ipc::Channel<linux::MediaControl>,
) -> std::result::Result<(), String> {
    app.state::<Storage<R>>()
        .media_session
        .register_listener(&event, handler)
}

#[cfg(target_os = "linux")]
#[tauri::command]
fn remove_listener<R: Runtime>(
    app: tauri::AppHandle<R>,
    event: String,
    channel_id: u32,
) -> std::result::Result<(), String> {
    app.state::<Storage<R>>()
        .media_session
        .remove_listener(&event, channel_id)
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    let builder = Builder::new("solmusic-storage").setup(|app, api| {
        #[cfg(desktop)]
        let _ = &api;
        #[cfg(mobile)]
        let storage = Storage {
            handle: mobile::init(app, api)?,
        };
        #[cfg(target_os = "linux")]
        let storage: Storage<R> = Storage {
            media_session: linux::MediaSession::new(),
            _runtime: std::marker::PhantomData,
        };
        #[cfg(all(desktop, not(target_os = "linux")))]
        let storage: Storage<R> = Storage {
            _runtime: std::marker::PhantomData,
        };
        app.manage(storage);
        Ok(())
    });

    #[cfg(target_os = "linux")]
    let builder =
        builder.invoke_handler(tauri::generate_handler![register_listener, remove_listener]);

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::{MediaSessionItem, MediaSessionUpdate};

    #[test]
    fn media_snapshot_serializes_browse_metadata_without_stream_urls() {
        let update = MediaSessionUpdate {
            active: true,
            title: "Track".into(),
            artist: "Artist".into(),
            album: None,
            artwork_url: Some("https://images.example/art.jpg".into()),
            playing: false,
            position_ms: 42,
            duration_ms: 120_000,
            can_go_previous: false,
            can_go_next: false,
            queue: vec![MediaSessionItem {
                id: "catalog-id".into(),
                title: "Track".into(),
                artist: "Artist".into(),
                album: None,
                artwork_url: None,
                duration_ms: 120_000,
            }],
            current_item: None,
            current_index: Some(0),
        };

        let json = serde_json::to_value(update).expect("snapshot should serialize");
        assert_eq!(json["currentIndex"], 0);
        assert_eq!(json["queue"][0]["id"], "catalog-id");
        assert!(json.get("streamUrl").is_none());
        assert!(json["queue"][0].get("streamUrl").is_none());
    }
}
