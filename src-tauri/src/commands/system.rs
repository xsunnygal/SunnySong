use serde::Serialize;
use solmusic_application::app_status;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatusDto {
    name: &'static str,
    version: &'static str,
    ready: bool,
}

#[tauri::command]
pub fn background_app(app_handle: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_solmusic_storage::StorageExt;
    app_handle
        .solmusic_storage()
        .background_app()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_app_status() -> AppStatusDto {
    let status = app_status();

    AppStatusDto {
        name: status.name,
        version: status.version,
        ready: status.ready,
    }
}
