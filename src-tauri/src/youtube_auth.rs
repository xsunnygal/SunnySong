#[cfg(desktop)]
use keyring::Entry;
use serde::Serialize;
use solmusic_youtube::{YouTubeAuth, YouTubeAuthState};
use tauri::AppHandle;

#[cfg(mobile)]
use tauri_plugin_solmusic_storage::StorageExt;

#[cfg(desktop)]
const KEYRING_SERVICE: &str = "app.solmusic.youtube";
#[cfg(desktop)]
const KEYRING_ACCOUNT: &str = "youtube.com.cookies";
#[cfg(mobile)]
const MOBILE_SECRET_KEY: &str = "youtube-cookies";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YouTubeAuthStatus {
    pub configured: bool,
}

pub struct YouTubeAuthService {
    state: YouTubeAuthState,
}

impl YouTubeAuthService {
    pub fn load(app: &AppHandle) -> Self {
        let state = YouTubeAuthState::default();
        if let Ok(Some(saved)) = load_saved_cookies(app) {
            if let Ok(auth) = YouTubeAuth::from_netscape(&saved) {
                state.replace(auth);
            }
        }
        Self { state }
    }

    pub fn state(&self) -> YouTubeAuthState {
        self.state.clone()
    }

    pub fn status(&self) -> YouTubeAuthStatus {
        YouTubeAuthStatus {
            configured: self.state.is_configured(),
        }
    }

    pub fn save(&self, app: &AppHandle, cookies: &str) -> Result<YouTubeAuthStatus, String> {
        let auth = YouTubeAuth::from_netscape(cookies)?;
        save_cookies(app, auth.netscape_cookies())?;
        self.state.replace(auth);
        Ok(self.status())
    }

    pub fn clear(&self, app: &AppHandle) -> Result<YouTubeAuthStatus, String> {
        if self.state.is_configured() {
            delete_cookies(app)?;
        }
        self.state.clear();
        Ok(self.status())
    }
}

#[cfg(desktop)]
fn load_saved_cookies(_app: &AppHandle) -> Result<Option<String>, String> {
    let entry = Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(secure_storage_error)?;
    Ok(entry.get_password().ok())
}

#[cfg(mobile)]
fn load_saved_cookies(app: &AppHandle) -> Result<Option<String>, String> {
    app.solmusic_storage()
        .get_secret(MOBILE_SECRET_KEY.into())
        .map_err(secure_storage_error)
}

#[cfg(desktop)]
fn save_cookies(_app: &AppHandle, cookies: &str) -> Result<(), String> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(secure_storage_error)?
        .set_password(cookies)
        .map_err(secure_storage_error)
}

#[cfg(mobile)]
fn save_cookies(app: &AppHandle, cookies: &str) -> Result<(), String> {
    app.solmusic_storage()
        .store_secret(MOBILE_SECRET_KEY.into(), cookies.into())
        .map_err(secure_storage_error)
}

#[cfg(desktop)]
fn delete_cookies(_app: &AppHandle) -> Result<(), String> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
        .map_err(secure_storage_error)?
        .delete_credential()
        .map_err(secure_storage_error)
}

#[cfg(mobile)]
fn delete_cookies(app: &AppHandle) -> Result<(), String> {
    app.solmusic_storage()
        .delete_secret(MOBILE_SECRET_KEY.into())
        .map_err(secure_storage_error)
}

fn secure_storage_error(error: impl std::fmt::Display) -> String {
    format!("secure credential storage is unavailable: {error}")
}
