use maekon_api_contracts::settings::AppSettings;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use std::sync::Arc;

use crate::error::ApiError;
use crate::services::settings_policy_service::{emit_policy_change_events, PolicyAuditWriter};
use crate::services::settings_secret_persistence::persist_api_key_bindings;
use crate::services::settings_service::apply_settings_to_config;

/// F-RR-38: `PolicyAuditWriter` is promoted to a long-lived field, created
/// once per `SettingsUpdateFlow` instance and reused across all `apply()`
/// calls.  The previous design called `emit_policy_change_events` with a raw
/// `Option<Arc<dyn AuditLogPort>>`, which constructed a new
/// `(mpsc::channel, tokio::spawn)` pair on every invocation.  Rapid settings
/// saves (e.g. slider drags) could accumulate N concurrent writer tasks with
/// no bound.
///
/// `Arc<PolicyAuditWriter>` allows `Clone` on `SettingsUpdateFlow` (required
/// by Axum state sharing) while sharing a single writer task across all clones.
#[derive(Clone)]
pub(crate) struct SettingsUpdateFlow {
    config_manager: ConfigManager,
    default_secret_backend_kind: CredentialBackendKind,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<SecretStoreSet>,
    /// Long-lived audit writer.  `None` when no `AuditLogPort` is configured.
    /// Wrapped in `Arc` so `SettingsUpdateFlow` can derive `Clone`.
    policy_audit_writer: Option<Arc<PolicyAuditWriter>>,
    /// #5707: coaching 엔진 hot-reload 핸들. `None`이면 연결 없음(테스트/스탠드얼론).
    coaching_engine: Option<Arc<dyn CoachingPort>>,
}

impl SettingsUpdateFlow {
    pub(crate) fn new(
        config_manager: ConfigManager,
        default_secret_backend_kind: CredentialBackendKind,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        audit_logger: Option<Arc<dyn AuditLogPort>>,
        coaching_engine: Option<Arc<dyn CoachingPort>>,
    ) -> Self {
        // Create the writer eagerly so the background task is running before
        // any apply() call.  If no audit logger is configured, skip the spawn.
        let policy_audit_writer =
            audit_logger.map(|logger| Arc::new(PolicyAuditWriter::new(logger)));
        Self {
            config_manager,
            default_secret_backend_kind,
            secret_store,
            secret_stores,
            policy_audit_writer,
            coaching_engine,
        }
    }

    pub(crate) async fn apply(&self, settings: &AppSettings) -> Result<(), ApiError> {
        let previous_config = self.config_manager.get();
        let mut next_config = previous_config.clone();

        apply_settings_to_config(&mut next_config, settings)?;
        persist_api_key_bindings(
            &mut next_config,
            self.secret_store.clone(),
            self.secret_stores.as_ref(),
            self.default_secret_backend_kind,
        )
        .await?;

        next_config
            .ai_provider
            .validate_selected_remote_endpoints()
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;

        // Managed (MDM) policy: reject an attempt to change an admin-locked
        // field with a clear message BEFORE the write chokepoint silently clamps
        // it, so the user sees "managed by your administrator" rather than a
        // value that silently reverts (#4832).
        let violations = self.config_manager.detect_managed_violations(&next_config);
        if !violations.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "the following settings are locked by your administrator and cannot be changed: {}",
                violations.join(", ")
            )));
        }

        self.config_manager
            .update(next_config.clone())
            .map_err(ApiError::from)?;

        // Reuse the long-lived writer — no new channel or task is spawned here.
        emit_policy_change_events(
            self.policy_audit_writer.as_deref(),
            &previous_config,
            &next_config,
        );

        // #5707: coaching 설정이 바뀐 경우 엔진에 즉시 hot-reload 신호를 보낸다.
        // serde_json Value 비교로 변경 감지 — false-positive 없이 apply 를 최소화한다.
        let coaching_changed = serde_json::to_value(&previous_config.coaching)
            .ok()
            .zip(serde_json::to_value(&next_config.coaching).ok())
            .map(|(prev, next)| prev != next)
            .unwrap_or(true);
        if coaching_changed {
            if let Some(ref engine) = self.coaching_engine {
                engine.apply_config(next_config.coaching.clone()).await;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::{ManagedConfig, ManagedPrivacy, PiiFilterLevel};
    use std::path::Path;
    use tempfile::TempDir;

    fn write_managed_pii(dir: &Path, level: PiiFilterLevel) -> std::path::PathBuf {
        let managed = ManagedConfig {
            privacy: ManagedPrivacy {
                pii_filter_level: Some(level),
            },
            ..Default::default()
        };
        let path = dir.join("managed.json");
        std::fs::write(&path, serde_json::to_string(&managed).unwrap()).unwrap();
        path
    }

    fn flow_for(cm: ConfigManager) -> SettingsUpdateFlow {
        // #5707: coaching_engine=None → テスト環境 (엔진 없음)
        SettingsUpdateFlow::new(cm, CredentialBackendKind::Env, None, None, None, None)
    }

    /// Interactive settings API rejects a managed-locked change with a clear
    /// message (the production `apply` chain, not a mock) — proves the #4832
    /// rejection wiring actually fires, not just the core detection.
    #[tokio::test]
    async fn apply_rejects_managed_locked_field_with_clear_message() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = write_managed_pii(dir.path(), PiiFilterLevel::Off);
        let cm = ConfigManager::with_paths(cfg_path, Some(managed_path)).unwrap();
        let flow = flow_for(cm.clone());

        let mut settings = AppSettings::default();
        settings.privacy.pii_filter_level = "Strict".to_string();

        let err = flow
            .apply(&settings)
            .await
            .expect_err("a managed-locked field must be rejected, not silently clamped");
        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("privacy.pii_filter_level"), "msg: {msg}");
                assert!(msg.contains("administrator"), "msg: {msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
        // The locked value is unchanged (construction already clamped to Off).
        assert_eq!(cm.get().privacy.pii_filter_level, PiiFilterLevel::Off);
    }

    /// With no managed policy, the same change is allowed and persisted.
    #[tokio::test]
    async fn apply_allows_change_when_no_managed_policy() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let cm = ConfigManager::with_paths(cfg_path, None).unwrap();
        let flow = flow_for(cm.clone());

        let mut settings = AppSettings::default();
        settings.privacy.pii_filter_level = "Strict".to_string();

        flow.apply(&settings)
            .await
            .expect("no managed policy => the change must be allowed");
        assert_eq!(cm.get().privacy.pii_filter_level, PiiFilterLevel::Strict);
    }
}
