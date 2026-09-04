use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::youtube_auth::{YouTubeAuthService, YouTubeAuthStatus};

#[tauri::command]
pub fn get_youtube_auth_status(auth: State<'_, Arc<YouTubeAuthService>>) -> YouTubeAuthStatus {
    auth.status()
}

#[tauri::command]
pub fn set_youtube_cookies(
    app_handle: AppHandle,
    auth: State<'_, Arc<YouTubeAuthService>>,
    cookies: String,
) -> Result<YouTubeAuthStatus, String> {
    auth.save(&app_handle, &cookies)
}

#[tauri::command]
pub fn clear_youtube_auth(
    app_handle: AppHandle,
    auth: State<'_, Arc<YouTubeAuthService>>,
) -> Result<YouTubeAuthStatus, String> {
    auth.clear(&app_handle)
}
