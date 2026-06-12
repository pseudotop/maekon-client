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
/// #4807 (U3): 로드된 파일의 `schema_version`이 현재 클라이언트가 지원하는
/// `CONFIG_SCHEMA_VERSION`보다 크면(= 더 새로운 클라이언트가 기록한 설정)
/// 다운그레이드 가드가 작동해 로드를 거부한다. (`maekon-storage`의
/// `run_migrations` future-version 가드 패턴 미러링.)
pub(super) fn load_and_migrate_from_file(path: &Path) -> Result<AppConfig, CoreError> {
    let mut config = persistence::load_from_file(path)?;

    // 다운그레이드 가드: 미래(더 큰) 스키마 버전은 거부한다.
    // 마이그레이션 ladder는 의도적으로 만들지 않는다 — 실제 breaking change가
    // 생기기 전까지는 단방향 가드만 둔다.
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

    /// #4807 (U3): schema_version 필드가 없는 (레거시) 설정 파일은
    /// 다운그레이드 가드에 걸리지 않고 정상 로드되어야 한다.
    #[test]
    fn legacy_config_without_schema_version_loads_ok() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        // 기본 설정을 직렬화한 뒤 schema_version 키를 제거해 레거시 파일을 흉내낸다.
        let mut value = serde_json::to_value(AppConfig::default_config()).unwrap();
        value.as_object_mut().unwrap().remove("schema_version");
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&value).unwrap()).unwrap();

        let loaded =
            load_and_migrate_from_file(&cfg_path).expect("레거시 설정은 정상 로드되어야 함");
        assert_eq!(
            loaded.schema_version,
            AppConfig::SCHEMA_VERSION,
            "버전 필드가 없는 설정은 baseline 버전으로 로드되어야 함"
        );
    }

    /// #4807 (U3): 현재 클라이언트가 지원하는 버전보다 큰 schema_version을
    /// 가진 설정 파일은 다운그레이드 가드에 의해 거부되어야 한다.
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
                    "에러 메시지는 다운그레이드 사유를 설명해야 함, got: {message}"
                );
            }
            other => panic!("expected CoreError::Config, got {other:?}"),
        }
    }

    /// 현재 버전과 동일한 schema_version은 정상 로드되어야 한다(가드 미작동).
    #[test]
    fn current_schema_version_loads_ok() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        let config = AppConfig::default_config();
        assert_eq!(config.schema_version, AppConfig::SCHEMA_VERSION);
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let loaded =
            load_and_migrate_from_file(&cfg_path).expect("현재 버전 설정은 정상 로드되어야 함");
        assert_eq!(loaded.schema_version, AppConfig::SCHEMA_VERSION);
    }
}
