use serde::Serialize;

/// Build metadata exposed to the desktop frontend and CLI.
///
/// All fields are embedded by build.rs at compile time
/// (`BUILD_DATE`, `GIT_SHA`, `CARGO_PKG_VERSION`).
#[derive(Debug, Clone, Serialize)]
pub struct AppBuildInfo {
    /// SemVer version, matching `Cargo.toml` workspace.version.
    pub version: &'static str,
    /// Build date in UTC, formatted as ISO `YYYY-MM-DD`.
    pub build_date: &'static str,
    /// Git short SHA at build time (9 chars), or `unknown` when git is unavailable.
    pub git_sha: &'static str,
}

impl AppBuildInfo {
    pub const fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            build_date: env!("BUILD_DATE"),
            git_sha: env!("GIT_SHA"),
        }
    }
}

/// Tauri command used by frontend surfaces such as Settings About.
#[tauri::command]
pub fn get_app_build_info() -> AppBuildInfo {
    AppBuildInfo::current()
}
