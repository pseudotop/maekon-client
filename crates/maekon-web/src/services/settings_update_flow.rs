use maekon_api_contracts::settings::AppSettings;
use maekon_core::config::CredentialBackendKind;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::ApiError;
use crate::services::settings_policy_service::{emit_policy_change_events, PolicyAuditWriter};
use crate::services::settings_secret_persistence::persist_api_key_bindings;
use crate::services::settings_service::apply_settings_to_config;
use crate::storage_port::WebStorage;

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
    /// #8045 B2: local storage handle used to retroactively purge an app's
    /// already-recorded history when it is newly excluded. `None` in standalone
    /// / test builds without storage (retroactive deletion is then skipped).
    storage: Option<Arc<dyn WebStorage>>,
    /// #8045 B2: frame-image directory, so the retroactive purge can also delete
    /// the excluded app's frame files from disk (not just their DB metadata).
    frames_dir: Option<PathBuf>,
}

impl SettingsUpdateFlow {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        config_manager: ConfigManager,
        default_secret_backend_kind: CredentialBackendKind,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        policy_audit_writer: Option<Arc<PolicyAuditWriter>>,
        coaching_engine: Option<Arc<dyn CoachingPort>>,
        storage: Option<Arc<dyn WebStorage>>,
        frames_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            config_manager,
            default_secret_backend_kind,
            secret_store,
            secret_stores,
            policy_audit_writer,
            coaching_engine,
            storage,
            frames_dir,
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

        // #8045 B2: retroactive app-exclusion (Recall parity). Adding an app to
        // the exclusion list is otherwise a pure config write that leaves its
        // already-recorded history intact. Diff the previous vs new exclusion
        // lists and, for each NEWLY added app / pattern, purge its stored frames
        // (+ files), events, and derived LLM summaries / embeddings. Best-effort:
        // the exclusion config already persisted above, so a purge failure is
        // logged but must NOT fail the save (that would block the exclusion from
        // taking effect going forward). Runs as a side-effect of the existing
        // settings-save path — no new IPC command.
        self.retroactively_purge_newly_excluded(&previous_config, &next_config)
            .await;

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

    /// #8045 B2: for every app / pattern NEWLY added to the exclusion list in
    /// this save, purge its already-recorded history. Best-effort — never fails
    /// the save.
    async fn retroactively_purge_newly_excluded(
        &self,
        previous: &maekon_core::config::AppConfig,
        next: &maekon_core::config::AppConfig,
    ) {
        let Some(ref storage) = self.storage else {
            return; // no storage wired (standalone / tests) — nothing to purge
        };

        // Newly added apps (case-insensitive, matching the capture-time filter)
        // and newly added patterns (compared verbatim).
        let new_apps = newly_added_entries(
            &previous.privacy.excluded_apps,
            &next.privacy.excluded_apps,
            true,
        );
        let new_patterns = newly_added_entries(
            &previous.privacy.excluded_app_patterns,
            &next.privacy.excluded_app_patterns,
            false,
        );
        if new_apps.is_empty() && new_patterns.is_empty() {
            return;
        }

        let (file_paths, counts) =
            match storage.delete_data_for_apps(&new_apps, &new_patterns).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        apps = ?new_apps,
                        patterns = ?new_patterns,
                        "retroactive app-exclusion purge failed (exclusion config still applied \
                         going forward)"
                    );
                    return;
                }
            };

        // Best-effort frame-file deletion (mirrors the range/all delete paths).
        let mut files_deleted = 0u64;
        if let Some(ref dir) = self.frames_dir {
            for path in &file_paths {
                match crate::services::data_web_service::resolve_stored_frame_path(dir, path) {
                    Ok(full_path) => {
                        if let Err(e) = tokio::fs::remove_file(&full_path).await {
                            tracing::debug!("retroactive purge: remove_file failed: {e}");
                        } else {
                            files_deleted += 1;
                        }
                    }
                    Err(e) => tracing::debug!("retroactive purge: skip invalid frame path: {e}"),
                }
            }
        }

        // Audit/log entry for the retroactive deletion (structured — surfaces in
        // the runtime log / bug-report diagnostics).
        tracing::info!(
            apps = ?new_apps,
            patterns = ?new_patterns,
            events_deleted = counts.events_deleted,
            frames_deleted = counts.frames_deleted,
            activity_segments_deleted = counts.activity_segments_deleted,
            embedding_vectors_deleted = counts.embedding_vectors_deleted,
            frame_files_deleted = files_deleted,
            "retroactive app-exclusion purge complete (#8045 B2)"
        );
    }
}

/// Returns the entries present in `next` but not in `previous`. When
/// `case_insensitive`, comparison lower-cases both sides (matching the
/// capture-time app-name filter); otherwise it is exact (used for patterns).
/// Empty / whitespace-only entries are ignored.
fn newly_added_entries(
    previous: &[String],
    next: &[String],
    case_insensitive: bool,
) -> Vec<String> {
    let key = |s: &str| {
        if case_insensitive {
            s.trim().to_lowercase()
        } else {
            s.trim().to_string()
        }
    };
    let prior: std::collections::HashSet<String> = previous
        .iter()
        .map(|s| key(s))
        .filter(|s| !s.is_empty())
        .collect();
    next.iter()
        .filter(|s| !s.trim().is_empty())
        .filter(|s| !prior.contains(&key(s)))
        .map(|s| s.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::{ManagedConfig, ManagedPrivacy, PiiFilterLevel};
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn newly_added_entries_diffs_case_insensitively_for_apps() {
        // Case-insensitive app diff: "Slack" already present (as "slack") is not
        // re-flagged; "Bank" is genuinely new.
        let prev = vec!["slack".to_string()];
        let next = vec!["Slack".to_string(), "Bank".to_string(), "  ".to_string()];
        let added = newly_added_entries(&prev, &next, true);
        assert_eq!(added, vec!["Bank".to_string()]);
    }

    #[test]
    fn newly_added_entries_diffs_verbatim_for_patterns() {
        let prev = vec!["*bank*".to_string()];
        let next = vec!["*bank*".to_string(), "*health*".to_string()];
        let added = newly_added_entries(&prev, &next, false);
        assert_eq!(added, vec!["*health*".to_string()]);
    }

    /// #8045 B2 wiring: saving settings that ADD an app to the exclusion list
    /// purges that app's already-recorded history via the storage handle — the
    /// end-to-end config-diff → storage-purge path, no new IPC command.
    #[tokio::test]
    async fn apply_purges_history_of_newly_excluded_app() {
        use maekon_core::ports::web_storage::EventQueryStorage;
        use maekon_core::types::TimeWindow;
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let cm = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();

        // Seed one event for the app to be excluded ("Banking") and one for a
        // different app ("Editor") that must survive.
        let storage = Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(30).unwrap());
        storage
            .upsert_backup_event(
                "e-bank",
                "Context",
                "2026-01-01T00:00:00+00:00",
                Some("Banking"),
                Some("Account balance"),
            )
            .unwrap();
        storage
            .upsert_backup_event(
                "e-edit",
                "Context",
                "2026-01-01T00:00:00+00:00",
                Some("Editor"),
                Some("main.rs"),
            )
            .unwrap();

        let window =
            TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00+00:00", "2027-01-01T00:00:00+00:00")
                .unwrap();
        assert_eq!(
            EventQueryStorage::count_events_in_range(&*storage, &window)
                .await
                .unwrap(),
            2,
            "both events must be seeded"
        );

        let flow = SettingsUpdateFlow::new(
            cm,
            CredentialBackendKind::Env,
            None,
            None,
            None,
            None,
            Some(storage.clone() as Arc<dyn WebStorage>),
            None,
        );

        let mut settings = AppSettings::default();
        settings.privacy.excluded_apps = vec!["Banking".to_string()];
        flow.apply(&settings)
            .await
            .expect("saving a new exclusion must succeed");

        // Only the excluded app's history is purged.
        assert_eq!(
            EventQueryStorage::count_events_in_range(&*storage, &window)
                .await
                .unwrap(),
            1,
            "the excluded app's event must be purged, the other app's must survive"
        );
    }

    fn write_managed_pii(dir: &Path, level: PiiFilterLevel) -> std::path::PathBuf {
        let managed = ManagedConfig {
            privacy: ManagedPrivacy {
                pii_filter_level: Some(level),
                min_pii_filter_level: None,
            },
            ..Default::default()
        };
        let path = dir.join("managed.json");
        std::fs::write(&path, serde_json::to_string(&managed).unwrap()).unwrap();
        path
    }

    fn flow_for(cm: ConfigManager) -> SettingsUpdateFlow {
        // #5707: coaching_engine=None → test environment (no engine)
        // #8045 B2: storage=None → retroactive purge is skipped in these tests
        SettingsUpdateFlow::new(
            cm,
            CredentialBackendKind::Env,
            None,
            None,
            None,
            None,
            None,
            None,
        )
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

    /// #8083/#8052 regression at the engine hot-reload seam: `apply` must feed the
    /// PRESERVED goals into `CoachingPort::apply_config`, not the stale (empty)
    /// settings payload. A recording double captures the config handed to
    /// `apply_config`; after a settings save whose payload carries no goals, the
    /// captured config must still contain the goal that config already held —
    /// proving the live engine tracker is re-seeded with the correct goals rather
    /// than clobbered to empty (the second victim of the CJ-03-01 clobber).
    #[tokio::test]
    async fn apply_feeds_preserved_regime_goals_into_engine_hot_reload() {
        use async_trait::async_trait;
        use maekon_core::config::CoachingConfig;
        use maekon_core::models::coaching::GoalProgressView;
        use std::collections::HashMap;
        use std::sync::Mutex;

        /// Records the `regime_goals` of the last config passed to `apply_config`.
        struct RecordingCoachingEngine {
            applied_goals: Mutex<Option<HashMap<String, u32>>>,
        }

        #[async_trait]
        impl CoachingPort for RecordingCoachingEngine {
            fn all_goal_progress_blocking(&self) -> Vec<GoalProgressView> {
                Vec::new()
            }
            fn update_regime_goals_blocking(&self, _goals: &HashMap<String, u32>) {}
            async fn snooze_profile(&self, _profile: &str, _duration_secs: u64) {}
            async fn record_feedback(&self, _message_id: &str, _positive: bool) {}
            async fn all_goal_progress(&self) -> Vec<GoalProgressView> {
                Vec::new()
            }
            async fn update_regime_goals(&self, _goals: &HashMap<String, u32>) {}
            async fn apply_config(&self, config: CoachingConfig) {
                *self
                    .applied_goals
                    .lock()
                    .expect("recording lock is never poisoned in this test") =
                    Some(config.regime_goals);
            }
        }

        let dir = TempDir::new().unwrap();
        let cm = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
        // A goal already persisted in config via the dedicated goals endpoint.
        cm.update_with(|config| {
            config
                .coaching
                .regime_goals
                .insert("deep_work".to_string(), 180);
            Ok(())
        })
        .expect("seed a goal into config");

        let engine = Arc::new(RecordingCoachingEngine {
            applied_goals: Mutex::new(None),
        });
        let flow = SettingsUpdateFlow::new(
            cm.clone(),
            CredentialBackendKind::Env,
            None,
            None,
            None,
            Some(engine.clone() as Arc<dyn CoachingPort>),
            None,
            None,
        );

        // Stale settings payload: an unrelated coaching change (a quiet-hours
        // entry — guaranteed to differ from the empty default, forcing
        // coaching_changed → apply_config) but NO goals.
        let mut settings = AppSettings::default();
        settings.coaching.quiet_hours =
            vec![maekon_api_contracts::settings::CoachingTimeRangeSettings {
                start: "22:00".to_string(),
                end: "07:30".to_string(),
            }];
        assert!(
            settings.coaching.regime_goals.is_empty(),
            "the stale form payload must carry no goals to reproduce the clobber"
        );

        flow.apply(&settings)
            .await
            .expect("settings save must succeed");

        // The engine hot-reload received the PRESERVED goal, not the empty payload.
        let applied = engine
            .applied_goals
            .lock()
            .expect("recording lock is never poisoned in this test")
            .clone()
            .expect("apply_config must have been called (coaching changed)");
        assert_eq!(
            applied.get("deep_work").copied(),
            Some(180),
            "apply_config must be fed the preserved goal, not the stale empty payload",
        );
        // And config itself retains the goal.
        assert_eq!(
            cm.get().coaching.regime_goals.get("deep_work").copied(),
            Some(180),
            "config must retain the goal owned by the dedicated goals endpoint",
        );
    }
}
