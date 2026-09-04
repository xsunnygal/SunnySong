#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStatus {
    pub name: &'static str,
    pub version: &'static str,
    pub ready: bool,
}

pub fn app_status() -> AppStatus {
    AppStatus {
        name: "SunnySong",
        version: env!("CARGO_PKG_VERSION"),
        ready: true,
    }
}
