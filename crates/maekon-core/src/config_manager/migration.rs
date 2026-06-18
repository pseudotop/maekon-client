use crate::config::AppConfig;
use crate::error::CoreError;
use std::path::Path;
use tracing::{error, info, warn};

use super::persistence;

/// Load config from `path` and apply any pending migrations in place.
///
/// If a migration was applied the migrated config is persisted back to disk
/// immediately so that the next launch reads the already-migrated file. A
/// persist failure is logged as a warning and does not abort the load.
///
/// #4807 (U3): if the loaded file's `schema_version` is greater than the
/// `CONFIG_SCHEMA_VERSION` this client supports (i.e. a config written by a
/// newer client), the downgrade guard kicks in and refuses to load it. (Mirrors
/// the `run_migrations` future-version guard pattern in `maekon-storage`.)
pub(super) fn load_and_migrate_from_file(path: &Path) -> Result<AppConfig, CoreError> {
    let mut config = persistence::load_from_file(path)?;

    // Downgrade guard: refuse future (larger) schema versions.
    // We deliberately do not build a migration ladder — until an actual breaking
    // change occurs, we keep only the one-way guard.
    if config.schema_version > AppConfig::SCHEMA_VERSION {
        error!(
            path = %path.display(),
            loaded_schema_version = config.schema_version,
            supported_schema_version = AppConfig::SCHEMA_VERSION,
            "config schema version is newer than this client supports"
        );
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "config schema version {} is newer than this client supports ({}); \
                 upgrade the client or use a separate config directory: {}",
                config.schema_version,
                AppConfig::SCHEMA_VERSION,
                path.display()
            ),
        });
    }

    if migrate_loaded_config(&mut config) {
        if let Err(e) = persistence::save_to_file(path, &config) {
            warn!(path = %path.display(), error = %e, "settings migration persist failed");
        } else {
            info!("settings migration applied: {}", path.display());
        }
    }
    Ok(config)
}

/// Apply all known in-place migrations to `config`.
///
/// Returns `true` if any migration was applied (caller should re-persist).
pub(super) fn migrate_loaded_config(config: &mut AppConfig) -> bool {
    let mut migrated = false;

    if config.web.grpc_port == crate::config::LEGACY_GRPC_DASHBOARD_PORT {
        config.web.grpc_port = crate::config::DEFAULT_GRPC_DASHBOARD_PORT;
        migrated = true;
    }

    if config.schema_version < AppConfig::SCHEMA_VERSION {
        config.schema_version = AppConfig::SCHEMA_VERSION;
        migrated = true;
    }

    migrated
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// #4807 (U3): a (legacy) config file without a `schema_version` field must
    /// not trip the downgrade guard and must load normally.
    #[test]
    fn legacy_config_without_schema_version_loads_ok() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        // Serialize the default config and remove the schema_version key to mimic a legacy file.
        let mut value = serde_json::to_value(AppConfig::default_config()).unwrap();
        value.as_object_mut().unwrap().remove("schema_version");
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

        let loaded =
            load_and_migrate_from_file(&cfg_path).expect("legacy config must load normally");
        assert_eq!(
            loaded.schema_version,
            AppConfig::SCHEMA_VERSION,
            "config without a version field must load as the baseline version"
        );
    }

    /// #4807 (U3): a config file whose `schema_version` is greater than the
    /// version this client supports must be refused by the downgrade guard.
    #[test]
    fn future_schema_version_is_refused() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        let mut config = AppConfig::default_config();
        config.schema_version = AppConfig::SCHEMA_VERSION + 1;
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let result = load_and_migrate_from_file(&cfg_path);
        match result.unwrap_err() {
            CoreError::Config { message, .. } => {
                assert!(
                    message.contains("newer than this client supports"),
                    "error message must explain the downgrade reason, got: {message}"
                );
            }
            other => panic!("expected CoreError::Config, got {other:?}"),
        }
    }

    /// A schema_version equal to the current version must load normally (guard inactive).
    #[test]
    fn current_schema_version_loads_ok() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        let config = AppConfig::default_config();
        assert_eq!(config.schema_version, AppConfig::SCHEMA_VERSION);
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let loaded = load_and_migrate_from_file(&cfg_path)
            .expect("config at the current version must load normally");
        assert_eq!(loaded.schema_version, AppConfig::SCHEMA_VERSION);
    }
}
