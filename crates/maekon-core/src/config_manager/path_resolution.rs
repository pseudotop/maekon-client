use crate::error::CoreError;
use std::path::PathBuf;

pub(super) const CONFIG_FILE_NAME: &str = "config.json";
pub(super) const MANAGED_FILE_NAME: &str = "managed.json";
pub(super) const APP_DIR_NAME: &str = "maekon";
pub(super) const APP_FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
/// Test/deployment override for the managed-policy file location (#4832).
/// Lets integration tests inject a managed.json without touching the
/// system-wide admin directory. Empty/unset ⇒ the platform default.
pub(super) const MANAGED_PATH_ENV: &str = "MAEKON_MANAGED_CONFIG_PATH";

pub(super) fn default_config_path() -> Result<PathBuf, CoreError> {
    let config_dir = config_dir()?;
    Ok(config_dir.join(CONFIG_FILE_NAME))
}

/// Resolve the managed-policy (`managed.json`) path.
///
/// A system-deployed policy at the platform's **system-wide, admin-owned**
/// directory (distinct from the per-user [`config_dir`]) takes **precedence**:
/// the `MAEKON_MANAGED_CONFIG_PATH` override may ADD or redirect policy on a
/// machine that has no system file (staging / tests), but it must never
/// SUBTRACT an existing admin policy. Otherwise a user who can set the client's
/// process environment could neutralize an MDM lock by redirecting to a missing
/// file (absence = "no policy"). The returned file may or may not exist; absence
/// is normal (handled as "no policy" by `load_managed_config`).
pub(super) fn managed_config_path() -> Result<PathBuf, CoreError> {
    let system_path = managed_config_dir()?.join(MANAGED_FILE_NAME);
    let env_override = std::env::var_os(MANAGED_PATH_ENV).map(PathBuf::from);
    Ok(resolve_managed_path(system_path, env_override))
}

/// Pure precedence rule (unit-testable): an existing system policy wins; only
/// when no system file is present does a non-empty env override apply.
fn resolve_managed_path(system_path: PathBuf, env_override: Option<PathBuf>) -> PathBuf {
    if system_path.exists() {
        return system_path;
    }
    if let Some(candidate) = env_override {
        if !candidate.as_os_str().is_empty() {
            return candidate;
        }
    }
    system_path
}

/// System-wide directory that holds the admin-deployed `managed.json`.
///
/// macOS: `/Library/Application Support/{app}` · Windows: `%ProgramData%\{app}`
/// · Linux: `/etc/{app}`. These are root/admin-owned by convention; the OS file
/// ACL — not this code — is the tamper barrier (documented in
/// `enterprise-deployment.md`).
pub fn managed_config_dir() -> Result<PathBuf, CoreError> {
    let app_dir_name = app_dir_name();

    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from("/Library/Application Support").join(app_dir_name))
    }

    #[cfg(target_os = "windows")]
    {
        // ProgramData is effectively always set on Windows; fall back to the
        // canonical default rather than aborting startup (Err) if it is absent,
        // so managed-policy resolution never becomes a new launch precondition.
        let program_data =
            std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        Ok(PathBuf::from(program_data).join(app_dir_name))
    }

    #[cfg(target_os = "linux")]
    {
        Ok(PathBuf::from("/etc").join(app_dir_name))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        tracing::warn!("Unsupported platform; using current directory for managed config base");
        Ok(PathBuf::from(".").join(app_dir_name))
    }
}

pub(super) fn app_dir_name() -> String {
    let flavor = std::env::var(APP_FLAVOR_ENV).ok();
    app_dir_name_for_flavor(flavor.as_deref())
}

pub(super) fn app_dir_name_for_flavor(flavor: Option<&str>) -> String {
    let Some(flavor) = flavor.map(str::trim).filter(|s| !s.is_empty()) else {
        return APP_DIR_NAME.to_string();
    };

    let suffix: String = flavor
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
        .collect();

    if suffix.is_empty() {
        APP_DIR_NAME.to_string()
    } else {
        format!("{APP_DIR_NAME}-{suffix}")
    }
}

pub fn config_dir() -> Result<PathBuf, CoreError> {
    let app_dir_name = app_dir_name();

    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Library/Application Support/{app_dir_name}/
        let home = std::env::var("HOME").map_err(|_| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "HOME environment variable not found".to_string(),
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(app_dir_name))
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: %APPDATA%\{app_dir_name}\
        let appdata = std::env::var("APPDATA").map_err(|_| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "APPDATA environment variable not found".to_string(),
        })?;
        Ok(PathBuf::from(appdata).join(app_dir_name))
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: ~/.config/{app_dir_name}/
        let home = std::env::var("HOME").map_err(|_| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "HOME environment variable not found".to_string(),
        })?;
        Ok(PathBuf::from(home).join(".config").join(app_dir_name))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        tracing::warn!("Unsupported platform; using current directory as config base");
        Ok(PathBuf::from(".").join(app_dir_name))
    }
}

pub fn data_dir() -> Result<PathBuf, CoreError> {
    #[cfg(target_os = "macos")]
    {
        // macOS: ~/Library/Application Support/{app_dir_name}/data/
        config_dir().map(|p| p.join("data"))
    }

    #[cfg(target_os = "windows")]
    {
        let app_dir_name = app_dir_name();
        // Windows: %LOCALAPPDATA%\{app_dir_name}\data\
        let local_appdata = std::env::var("LOCALAPPDATA").map_err(|_| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "LOCALAPPDATA environment variable not found".to_string(),
        })?;
        Ok(PathBuf::from(local_appdata).join(app_dir_name).join("data"))
    }

    #[cfg(target_os = "linux")]
    {
        let app_dir_name = app_dir_name();
        // Linux: ~/.local/share/{app_dir_name}/
        let home = std::env::var("HOME").map_err(|_| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "HOME environment variable not found".to_string(),
        })?;
        Ok(PathBuf::from(home)
            .join(".local")
            .join("share")
            .join(app_dir_name))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let app_dir_name = app_dir_name();
        Ok(PathBuf::from(".").join(app_dir_name).join("data"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn system_managed_policy_wins_over_env_override() {
        // An installed system policy must NOT be neutralizable by redirecting
        // MAEKON_MANAGED_CONFIG_PATH to a missing file (#4832 hardening).
        let dir = TempDir::new().unwrap();
        let system_path = dir.path().join("system-managed.json");
        std::fs::write(&system_path, "{}").unwrap();
        let env_redirect = dir.path().join("does-not-exist.json");

        let resolved = resolve_managed_path(system_path.clone(), Some(env_redirect));
        assert_eq!(resolved, system_path, "system policy must take precedence");
    }

    #[test]
    fn env_override_applies_only_when_no_system_policy() {
        let dir = TempDir::new().unwrap();
        let system_path = dir.path().join("absent-system.json"); // not created
        let env_path = dir.path().join("staging-managed.json");

        let resolved = resolve_managed_path(system_path, Some(env_path.clone()));
        assert_eq!(
            resolved, env_path,
            "env override applies with no system file"
        );
    }

    #[test]
    fn falls_back_to_system_path_when_no_override() {
        let dir = TempDir::new().unwrap();
        let system_path = dir.path().join("absent-system.json"); // not created

        // No env, and empty env both resolve to the (absent) system path.
        assert_eq!(resolve_managed_path(system_path.clone(), None), system_path);
        assert_eq!(
            resolve_managed_path(system_path.clone(), Some(PathBuf::new())),
            system_path
        );
    }
}
