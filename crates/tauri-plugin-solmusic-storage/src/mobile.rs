use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

const PLUGIN_IDENTIFIER: &str = "app.solmusic.storage";

pub fn init<R: Runtime, C: serde::de::DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<PluginHandle<R>> {
    Ok(api.register_android_plugin(PLUGIN_IDENTIFIER, "StoragePlugin")?)
}
