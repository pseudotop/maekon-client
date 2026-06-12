use super::ConfigManager;
use crate::config::AppConfig;
use crate::error::CoreError;
use std::fs;
use std::path::{Component, Path};
use tracing::{debug, info, warn};

impl ConfigManager {
    pub(super) fn load_and_migrate_from_file(path: &Path) -> Result<AppConfig, CoreError> {
        let mut config = Self::load_from_file(path)?;
        if Self::migrate_loaded_config(&mut config) {
            if let Err(e) = Self::save_to_file(path, &config) {
                warn!(path = %path.display(), error = %e, "settings migration persist failed");
            } else {
                info!("settings migration applied: {}", path.display());
            }
        }
        Ok(config)
    }

    pub(super) fn load_from_file(path: &Path) -> Result<AppConfig, CoreError> {
        Self::validate_config_file_path(path)?;
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: format!(
                    "Config path must not contain parent directory traversal: {}",
                    path.display()
                ),
            });
        }

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

    pub(super) fn save_to_file(path: &Path, config: &AppConfig) -> Result<(), CoreError> {
        Self::validate_config_file_path(path)?;
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: format!(
                    "Config path must not contain parent directory traversal: {}",
                    path.display()
                ),
            });
        }

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
}
