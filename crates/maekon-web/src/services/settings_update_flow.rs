use maekon_api_contracts::settings::AppSettings;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use std::sync::Arc;

use crate::error::ApiError;
use crate::services::settings_policy_service::{emit_policy_change_events, PolicyAuditWriter};
use crate::services::settings_secret_persistence::persist_api_key_bindings;
use crate::services::settings_service::apply_settings_to_config;

/// #6117: `SettingsUpdateFlow` no longer *constructs* a `PolicyAuditWriter`.
/// It receives the single, server-lifetime `Arc<PolicyAuditWriter>` that
/// `WebServer::build_router` built once and stashed in
/// `AppState.automation.policy_audit_writer`.
///
/// The previous shape (F-RR-38) created the writer in `SettingsUpdateFlow::new`,
/// but because `SettingsWebContext` — and therefore `SettingsCommandService`
/// and `SettingsUpdateFlow` — is rebuilt on **every** request and dropped at
/// request end, the writer's bounded-channel drain task was spawned and then
/// `abort()`ed per `POST /api/settings`, dropping the just-enqueued audit event
/// before it could flush.  Sharing one long-lived writer fixes that: the drain
/// task lives for the whole server lifetime.
///
/// `Arc<PolicyAuditWriter>` still allows `Clone` on `SettingsUpdateFlow`
/// (required by Axum state sharing) while sharing the single writer task across
/// all clones.
#[derive(Clone)]
pub(crate) struct SettingsUpdateFlow {
    config_manager: ConfigManager,
    default_secret_backend_kind: CredentialBackendKind,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<SecretStoreSet>,
    /// Shared, server-lifetime audit writer (owned by `AppState`).  `None` when
    /// no `AuditLogPort` is configured (tests / standalone builds).
    policy_audit_writer: Option<Arc<PolicyAuditWriter>>,
    /// #5707: coaching engine hot-reload handle. `None` means no connection
    /// (tests / standalone).
    coaching_engine: Option<Arc<dyn CoachingPort>>,
}

impl SettingsUpdateFlow {
    pub(crate) fn new(
        config_manager: ConfigManager,
        default_secret_backend_kind: CredentialBackendKind,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        policy_audit_writer: Option<Arc<PolicyAuditWriter>>,
        coaching_engine: Option<Arc<dyn CoachingPort>>,
    ) -> Self {
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

        // #6274: validate the config (endpoint cleartext/blocked-model/timeout
        // gates + managed-policy) BEFORE writing any secret to the OS keystore.
        // persist_api_key_bindings is a true keystore upsert keyed only by
        // provider+profile, so running it first meant a save rejected by these
        // checks had already clobbered the live credential slot with no rollback
        // (config rolls back, keystore does not). These validations are all
        // config-level — the endpoint URL/model/timeout are set by
        // apply_settings_to_config above, and a remote provider with no key/binding
        // is correctly rejected here regardless of order — so they are safe to run
        // before the binding is persisted.
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

        // Validation passed — now persist the API key(s) to the keystore and
        // commit the config. (Persist mutates next_config's inline keys into
        // secret_ref bindings.)
        persist_api_key_bindings(
            &mut next_config,
            self.secret_store.clone(),
            self.secret_stores.as_ref(),
            self.default_secret_backend_kind,
        )
        .await?;

        self.config_manager
            .update(next_config.clone())
            .map_err(ApiError::from)?;

        // #6117: enqueue policy-change audit events into the long-lived,
        // server-lifetime writer and AWAIT the hand-off so the event is durably
        // queued before this request returns — no fire-and-forget into a task
        // that is dropped at request end.  No new channel or task is spawned.
        emit_policy_change_events(
            self.policy_audit_writer.as_deref(),
            &previous_config,
            &next_config,
        )
        .await;

        // #5707: when the coaching config changed, signal the engine to hot-reload
        // immediately. Change detection uses a serde_json Value comparison —
        // minimizing applies with no false positives.
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
        // #5707: coaching_engine=None → test environment (no engine)
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
