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

    fs::write(&safe_path, content).map_err(|e| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!(
            "Failed to write config file: {}: {}",
            safe_path.display(),
            e
        ),
    })?;

    Ok(())
}
