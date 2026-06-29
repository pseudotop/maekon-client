use crate::config::AppConfig;
use crate::error::CoreError;
use std::fs;
use std::path::{Component, Path};
use tracing::debug;

/// Validate that `path` points to a file and contains no `..` components.
///
/// Called as a defense-in-depth guard at every entry point that touches the
/// filesystem. The `canonicalize()` calls in `load_from_file` / `save_to_file`
/// are the CodeQL-recognized sanitizer barriers; this function is the
/// value-preserving pre-check.
pub(super) fn validate_config_file_path(path: &Path) -> Result<(), CoreError> {
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: "Config path must point to a file".to_string(),
        });
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "Config path must not contain parent directory traversal: {}",
                path.display()
            ),
        });
    }

    Ok(())
}

/// Read and deserialize a config file from `path`.
///
/// Runs `validate_config_file_path` + an inline traversal guard + a
/// `canonicalize()` barrier before the `fs::` sink. The double-guard pattern
/// is intentional: the `validate_config_file_path` call is a value-preserving
/// pre-check; `canonicalize()` is the CodeQL-recognized sanitizer.
pub(super) fn load_from_file(path: &Path) -> Result<AppConfig, CoreError> {
    validate_config_file_path(path)?;
    // Inline traversal guard kept as defense-in-depth (constructor +
    // cross-function + this) — the canonicalize() below is the
    // CodeQL-recognized sanitizer barrier.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "Config path must not contain parent directory traversal: {}",
                path.display()
            ),
        });
    }
    // Canonicalize the path before the fs:: sink. CodeQL recognizes
    // Path::canonicalize() as a path-injection barrier (the previous
    // components()-iter check was a value-preserving condition and was
    // not recognized as a sanitizer).
    let safe_path = path.canonicalize().map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Failed to canonicalize config path: {}: {}",
            path.display(),
            e
        ),
    })?;
    let content = fs::read_to_string(&safe_path).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!("Failed to read config file: {}: {}", safe_path.display(), e),
    })?;

    let config: AppConfig = serde_json::from_str(&content).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Failed to parse config file: {}: {}",
            safe_path.display(),
            e
        ),
    })?;

    debug!("settings file load complete: {}", safe_path.display());
    Ok(config)
}

/// Serialize `config` and write it to `path`.
///
/// Canonicalizes the parent directory (not the file, which may not exist yet
/// on first save) before the `fs::write` sink.
pub(super) fn save_to_file(path: &Path, config: &AppConfig) -> Result<(), CoreError> {
    validate_config_file_path(path)?;
    // Inline traversal guard kept as defense-in-depth (constructor +
    // cross-function + this) — the canonicalize() below is the
    // CodeQL-recognized sanitizer barrier.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "Config path must not contain parent directory traversal: {}",
                path.display()
            ),
        });
    }
    // Canonicalize the parent directory + join the original file_name to
    // build the actual write target. Path::canonicalize() requires the
    // target to exist; the file itself may not exist yet on first save,
    // but the parent does (with_path creates it via fs::create_dir_all).
    // CodeQL recognizes Path::canonicalize() as a path-injection barrier.
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let canonical_parent = parent.canonicalize().map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Failed to canonicalize parent directory: {}: {}",
            parent.display(),
            e
        ),
    })?;
    let file_name = path.file_name().ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!("Config path has no file name: {}", path.display()),
    })?;
    let safe_path = canonical_parent.join(file_name);

    let content = serde_json::to_string_pretty(config).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!("Failed to serialize config: {}", e),
    })?;

    // #5977: atomic write — serialize to a sibling `.tmp` then rename into place.
    // A crash/OOM/power-loss mid-write cannot leave a truncated config.json that
    // load_from_file would fail to parse and then silently overwrite with
    // defaults (losing all user settings). The tmp file shares safe_path's parent
    // directory, so the rename is same-filesystem and atomic. Mirrors
    // consent.rs::save_to_file.
    let tmp_path = safe_path.with_extension("json.tmp");

    // #6175 / #7074: restrict the tmp file to owner-only BEFORE the rename so the
    // published config.json is never world/group-readable. config.json can hold
    // inline plaintext API keys / secrets, so a default-umask write (typically
    // 0o644) would leak them to every local user. The "create-empty → harden →
    // write" logic now lives in the shared `secure_file::write_owner_only` SSOT so
    // the config and consent write paths cannot diverge again (#7074): on Windows
    // the DACL is applied while the tmp is still EMPTY and the step is fail-closed
    // (a DACL failure removes the tmp and surfaces the error). Mirrors
    // consent.rs / EncryptionKey::save_to_file. Map the shared helper's
    // `io::Error` back into this module's `CoreError::Config` contract.
    crate::secure_file::write_owner_only(&tmp_path, content.as_bytes()).map_err(|e| {
        CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "Failed to write temp config file: {}: {}",
                tmp_path.display(),
                e
            ),
        }
    })?;

    fs::rename(&tmp_path, &safe_path).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Failed to commit config file: {} -> {}: {}",
            tmp_path.display(),
            safe_path.display(),
            e
        ),
    })?;

    Ok(())
}
