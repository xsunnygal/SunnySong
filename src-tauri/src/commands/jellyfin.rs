use std::sync::Arc;

use tauri::State;

use crate::jellyfin::{
    JellyfinRefreshResult, JellyfinServer, JellyfinService, PublicServerInfo, QuickConnectSession,
};

#[tauri::command]
pub async fn validate_jellyfin_server(
    service: State<'_, Arc<JellyfinService>>,
    address: String,
) -> Result<PublicServerInfo, String> {
    service.validate_server(&address).await
}

#[tauri::command]
pub async fn connect_jellyfin_password(
    service: State<'_, Arc<JellyfinService>>,
    address: String,
    username: String,
    password: String,
) -> Result<JellyfinServer, String> {
    if username.trim().is_empty() {
        return Err("Jellyfin username cannot be blank".into());
    }
    service.login_password(&address, &username, &password).await
}

#[tauri::command]
pub async fn begin_jellyfin_quick_connect(
    service: State<'_, Arc<JellyfinService>>,
    address: String,
) -> Result<QuickConnectSession, String> {
    service.begin_quick_connect(&address).await
}

#[tauri::command]
pub async fn finish_jellyfin_quick_connect(
    service: State<'_, Arc<JellyfinService>>,
    address: String,
    secret: String,
) -> Result<Option<JellyfinServer>, String> {
    service.finish_quick_connect(&address, &secret).await
}

#[tauri::command]
pub fn get_jellyfin_servers(
    service: State<'_, Arc<JellyfinService>>,
) -> Result<Vec<JellyfinServer>, String> {
    service.servers()
}

#[tauri::command]
pub async fn refresh_jellyfin_libraries(
    service: State<'_, Arc<JellyfinService>>,
    server_id: i64,
) -> Result<JellyfinRefreshResult, String> {
    service.refresh_server_libraries(server_id).await
}

#[tauri::command]
pub async fn set_jellyfin_library_enabled(
    service: State<'_, Arc<JellyfinService>>,
    server_id: i64,
    library_id: String,
    enabled: bool,
) -> Result<(), String> {
    service
        .set_library_enabled(server_id, &library_id, enabled)
        .await
}

#[tauri::command]
pub fn remove_jellyfin_server(
    service: State<'_, Arc<JellyfinService>>,
    server_id: i64,
) -> Result<(), String> {
    service.remove_server(server_id)
}
