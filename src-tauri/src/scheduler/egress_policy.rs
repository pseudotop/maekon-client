//! Egress (off-device upload) policy — the fail-closed decision gate for
//! whether a captured `Event` may leave the device, plus the egress audit
//! ledger write-hook.
//!
//! Split out of `scheduler/config.rs` (#7731, ctd-W2 B4, 2026-07-03) — this
//! file used to be interleaved with the (now relocated) `SchedulerStorage`
//! trait/impl and `SchedulerConfig`. Behavior is unchanged; only the module
//! location moved.

use chrono::Utc;
use std::sync::Arc;

use maekon_core::config::{ExternalDataPolicy, PrivacyConfig};
use maekon_core::config_manager::ConfigManager;
use maekon_core::models::event::Event;
use maekon_core::ports::consent_manager::{ConsentGate, ConsentManagerPort};
use maekon_core::ports::scheduler_storage::SchedulerStorage;
use maekon_vision::privacy::{sanitize_title_with_level, should_exclude_by_policy};

use super::config::SchedulerConfig;

/// Egress destination (sink target) string — the batch uploader sends to a
/// single server endpoint. The `BatchSink` port does not expose the target
/// string, so we record it as a constant (#4803).
pub(super) const EGRESS_DESTINATION_BATCH_UPLOAD: &str = "server.batch_upload";

/// Pseudo-destination for capture-time exclusion entries (#7909, T1.1).
///
/// Unlike every other ledger row this is NOT an egress event: it records that a
/// frame was deliberately NOT captured because the active app matched the
/// exclusion policy, so the transparency panel (T1.2 #7910) can show
/// "capture blocked" evidence alongside upload dispositions. Nothing was
/// produced or sent, hence `byte_count = 0` / `recipient_count = 0`.
pub(super) const EGRESS_DESTINATION_LOCAL_CAPTURE: &str = "local.capture";

/// Disposition string for capture-time exclusion ledger entries (#7909).
pub(super) const DISPOSITION_CAPTURE_BLOCKED: &str = "capture_blocked";

/// Return the type-identifier string for an `Event` (for egress_ledger.event_type).
pub(super) fn egress_event_type(event: &Event) -> &'static str {
    match event {
        Event::Context(_) => "Context",
        Event::Window(_) => "Window",
        Event::User(_) => "User",
        Event::Input(_) => "Input",
        Event::Process(_) => "Process",
        Event::System(_) => "System",
        Event::Clipboard(_) => "Clipboard",
        Event::FileAccess(_) => "FileAccess",
    }
}

/// Serialized payload byte size of the event. Returns 0 on serialization failure.
pub(super) fn egress_byte_count(event: &Event) -> i64 {
    serde_json::to_vec(event)
        .map(|v| v.len() as i64)
        .unwrap_or(0)
}

/// Record a single egress event in the audit ledger (#4803/E20).
///
/// Because `prepare_event_for_upload` consumes the event, the caller computes
/// `event_type`/`byte_count` from a reference *before* consuming it and passes
/// them into this helper. `disposition` is `uploaded` (sink enqueue succeeded)
/// or `blocked` (excluded by policy). `consent_state` is the telemetry-consent
/// snapshot string at the egress moment. A recording failure is only logged via
/// `warn!` so it never disrupts the upload/capture flow.
///
/// #6134: `record_egress` is a synchronous SQLite write (sync `SchedulerStorage`
/// method). Calling it inline on the async monitor / event_snapshot loops would
/// block a tokio reactor worker thread on SQLite I/O. We mirror the sibling
/// `offload_storage` / `with_conn` / `spawn_blocking` pattern: clone the
/// `Arc<dyn SchedulerStorage>` (it is `Send + Sync`), move an owned
/// `EgressLedgerRecord` into the blocking pool, and `.await` the handle so a
/// task panic surfaces as a `JoinError` we can log instead of silently dropping.
#[allow(clippy::too_many_arguments)]
pub(super) async fn record_event_egress(
    storage: &Arc<dyn SchedulerStorage>,
    event_type: &str,
    byte_count: i64,
    disposition: &str,
    consent_state: &str,
) {
    use maekon_core::models::storage_records::EgressLedgerRecord;
    let record = EgressLedgerRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        event_type: event_type.to_string(),
        event_id: None,
        byte_count,
        // Telemetry uploads to a single endpoint (server.batch_upload) → 1 recipient.
        recipient_count: 1,
        destination: EGRESS_DESTINATION_BATCH_UPLOAD.to_string(),
        disposition: disposition.to_string(),
        consent_state: consent_state.to_string(),
        occurred_at: Utc::now().to_rfc3339(),
    };
    // Offload the synchronous SQLite INSERT to the blocking pool so the async
    // monitor/event loops are never blocked on disk I/O (#6134).
    let storage = storage.clone();
    match tokio::task::spawn_blocking(move || storage.record_egress(&record)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(err.code = %e.code(), "egress ledger record failure: {e}");
        }
        Err(e) => {
            tracing::warn!("egress ledger record task panicked: {e}");
        }
    }
}

/// Record a capture-time exclusion block in the transparency ledger (#7909).
///
/// Written once per transition INTO an excluded app (not per tick — the
/// monitor loop runs at 1s cadence and per-tick rows would flood the ledger),
/// mirroring the `record_event_egress` spawn_blocking offload pattern (#6134).
pub(super) async fn record_capture_block(storage: &Arc<dyn SchedulerStorage>, consent_state: &str) {
    use maekon_core::models::storage_records::EgressLedgerRecord;
    let record = EgressLedgerRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        event_type: "Frame".to_string(),
        event_id: None,
        byte_count: 0,
        recipient_count: 0,
        destination: EGRESS_DESTINATION_LOCAL_CAPTURE.to_string(),
        disposition: DISPOSITION_CAPTURE_BLOCKED.to_string(),
        consent_state: consent_state.to_string(),
        occurred_at: Utc::now().to_rfc3339(),
    };
    let storage = storage.clone();
    match tokio::task::spawn_blocking(move || storage.record_egress(&record)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(err.code = %e.code(), "capture-block ledger record failure: {e}");
        }
        Err(e) => {
            tracing::warn!("capture-block ledger record task panicked: {e}");
        }
    }
}

pub(super) const REDACTED_WINDOW_TITLE: &str = "[REDACTED_WINDOW_TITLE]";

#[derive(Clone)]
pub(super) struct PlatformEgressPolicy {
    enabled: bool,
    external_data_policy: ExternalDataPolicy,
    privacy_config: PrivacyConfig,
    /// Telemetry-consent binding. In production a shared `Arc<dyn ConsentManagerPort>`
    /// is injected; when `None`, the legacy behavior (config.upload_enabled alone)
    /// is preserved.
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// Runtime config binding for tracking-schedule mute windows. When missing,
    /// legacy `upload_enabled` + telemetry-consent behavior is preserved.
    config_manager: Option<ConfigManager>,
}

impl PlatformEgressPolicy {
    pub(super) fn new(config: &SchedulerConfig) -> Self {
        Self {
            enabled: config.upload_enabled,
            external_data_policy: config.external_data_policy,
            privacy_config: config.privacy_config.clone(),
            consent_manager: None,
            config_manager: None,
        }
    }

    /// Bind telemetry consent (ConsentManager) to the egress policy.
    /// The production sync loop injects a shared `Arc<dyn ConsentManagerPort>`.
    pub(super) fn with_consent_manager(
        mut self,
        consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    ) -> Self {
        self.consent_manager = consent_manager;
        self
    }

    /// Bind runtime config so off-device batch egress honors tracking-schedule
    /// mute windows on the same snapshot authority as capture/audio gates.
    pub(super) fn with_config_manager(mut self, config_manager: Option<ConfigManager>) -> Self {
        self.config_manager = config_manager;
        self
    }

    /// Whether telemetry consent is given.
    ///
    /// #7728 (ctd-W2 E7): BEHAVIOR CHANGE — when no ConsentManager is injected
    /// this used to return `true` (`is_none_or`, "preserve legacy behavior"),
    /// which fail-OPENed telemetry egress the moment consent wiring was ever
    /// absent — the one outlier among ~17 hand-composed
    /// `effective_permissions()` call sites in src-tauri, all of which
    /// otherwise defaulted closed. Now routed through the shared
    /// [`ConsentGate::may_upload_telemetry`], whose no-manager default is
    /// fail-closed like every sibling permission tier (ADR-021/ADR-026:
    /// ONESHIM is a privacy product — "unknown" must mean "deny", not
    /// "allow"). When a manager IS injected, behavior is unchanged: `true`
    /// only if `telemetry` was granted and consent is currently `Valid`
    /// (Expired/UpdateRequired also fail closed).
    fn telemetry_consented(&self) -> bool {
        ConsentGate::from_ref(self.consent_manager.as_ref()).may_upload_telemetry()
    }

    fn tracking_schedule_muted(&self) -> bool {
        self.config_manager.as_ref().is_some_and(|cm| {
            let snapshot = cm.snapshot();
            super::schedule::tracking_schedule_active(snapshot.as_ref())
        })
    }

    /// Whether server upload of collected events is enabled, read LIVE from
    /// the shared `ConfigManager` snapshot on every call (#7698 S3) — mirrors
    /// `tracking_schedule_muted()`'s live-snapshot pattern exactly, instead of
    /// the `enabled` field frozen once at `PlatformEgressPolicy::new()` and
    /// never revisited for the rest of the scheduler session.
    ///
    /// Falls back to the frozen `self.enabled` (the value `upload_enabled` had
    /// at construction time) when no `ConfigManager` is injected — preserves
    /// the legacy/test-builder behavior for callers that never call
    /// `with_config_manager()`.
    fn upload_enabled_live(&self) -> bool {
        self.config_manager
            .as_ref()
            .map(|cm| cm.snapshot().monitor.upload_enabled)
            .unwrap_or(self.enabled)
    }

    /// Whether egress is active = upload setting AND telemetry consent AND
    /// no active tracking-schedule mute window. All three terms are read
    /// LIVE on every call (fail-closed: any one flipping false immediately
    /// stops egress on the very next event, no scheduler restart required).
    pub(super) fn is_enabled(&self) -> bool {
        self.upload_enabled_live() && self.telemetry_consented() && !self.tracking_schedule_muted()
    }

    /// Consent snapshot string at the egress moment (for egress_ledger.consent_state, #4803).
    ///
    /// Records the upload setting, telemetry-consent state, and tracking-mute
    /// state together so a post-hoc audit can distinguish whether a `blocked`
    /// reason was a missing consent or a disabled upload. Uses the same LIVE
    /// `upload_enabled_live()` read as `is_enabled()` so the ledger string
    /// never disagrees with the actual gating decision it is documenting.
    pub(super) fn consent_state_snapshot(&self) -> String {
        format!(
            "upload_enabled={};telemetry={};tracking_schedule_muted={}",
            self.upload_enabled_live(),
            self.telemetry_consented(),
            self.tracking_schedule_muted()
        )
    }

    pub(super) fn prepare_event_for_upload(&self, mut event: Event) -> Option<Event> {
        // Egress requires passing both the upload setting and telemetry consent
        // (fail-closed).
        if !self.is_enabled() {
            return None;
        }

        match &mut event {
            Event::Context(ctx) => {
                let app_name = ctx.app_name.clone();
                let title = ctx.window_title.clone();
                if self.should_skip(&app_name, &title) {
                    return None;
                }
                ctx.window_title = self.sanitize_title(&title);
            }
            Event::Window(layout) => {
                let app_name = layout.window.app_name.clone();
                let title = layout.window.window_title.clone();
                if self.should_skip(&app_name, &title) {
                    return None;
                }
                layout.window.window_title = self.sanitize_title(&title);
            }
            Event::User(user) => {
                let app_name = user.app_name.clone();
                let title = user.window_title.clone();
                if self.should_skip(&app_name, &title) {
                    return None;
                }
                user.window_title = self.sanitize_title(&title);
            }
            Event::System(_) | Event::Input(_) | Event::Process(_) => {}
            // Clipboard/file-access events may contain sensitive data, so the
            // egress policy treats them as fail-closed (blocked by default) and
            // excludes them from upload.
            Event::Clipboard(_) | Event::FileAccess(_) => return None,
        }

        Some(event)
    }

    fn sanitize_title(&self, title: &str) -> String {
        match self.external_data_policy {
            ExternalDataPolicy::AllowFiltered => {
                // #6442 F10: resolve the effective level via the shared SSOT floor so the
                // window-title and OCR-image egress paths resolve identically.
                // AllowFiltered floors Off -> Basic; the incoherent pairing is surfaced
                // once, loudly, at config load (SchedulerConfig::has_incoherent_egress_privacy)
                // — replacing #5992's silent per-call upgrade warn.
                let effective_level = self
                    .external_data_policy
                    .effective_egress_pii_level(self.privacy_config.pii_filter_level);
                sanitize_title_with_level(title, effective_level)
            }
            ExternalDataPolicy::PiiFilterStrict | ExternalDataPolicy::PiiFilterStandard => {
                REDACTED_WINDOW_TITLE.to_string()
            }
        }
    }

    fn should_skip(&self, app_name: &str, window_title: &str) -> bool {
        should_exclude_by_policy(&self.privacy_config, app_name, window_title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::PiiFilterLevel;
    use maekon_core::consent::ConsentManager;
    use maekon_core::models::event::{
        ClipboardContentType, ClipboardEvent, ContextEvent, FileAccessEvent, FileEventType,
    };
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::path::PathBuf;

    /// A `ConsentManager` with `telemetry` granted (`Valid` status) — the
    /// standard fixture for a test that wants to isolate a DIFFERENT gating
    /// dimension (upload_enabled live-read, tracking-schedule mute, PII
    /// floor, …) from the telemetry-consent dimension itself. #7728 made
    /// "no ConsentManager installed" fail-closed for telemetry, so any test
    /// that previously relied on that default (`is_none_or` → `true`) to
    /// stay `is_enabled() == true` must now inject this fixture instead.
    fn granted_telemetry_consent_manager() -> Arc<dyn ConsentManagerPort> {
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        cm.grant_consent(
            maekon_core::consent::ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent");
        cm
    }

    fn enabled_policy() -> PlatformEgressPolicy {
        // Verify that clipboard/file events are blocked even under an
        // upload-enabled policy.
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        PlatformEgressPolicy::new(&config)
    }

    #[test]
    fn clipboard_event_is_dropped_from_upload() {
        let policy = enabled_policy();
        let event = Event::Clipboard(ClipboardEvent {
            timestamp: Utc::now(),
            content_type: ClipboardContentType::Text,
            char_count: 42,
            preview: Some("sensitive".to_string()),
        });

        // fail-closed: clipboard events must be excluded from upload.
        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    #[test]
    fn file_access_event_is_dropped_from_upload() {
        let policy = enabled_policy();
        let event = Event::FileAccess(FileAccessEvent {
            timestamp: Utc::now(),
            relative_path: PathBuf::from("secret/notes.txt"),
            event_type: FileEventType::Created,
            extension: Some("txt".to_string()),
        });

        // fail-closed: file-access events must be excluded from upload.
        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    // --- #4805: bind egress to telemetry consent ---

    fn policy_with_telemetry(consented: bool) -> PlatformEgressPolicy {
        use maekon_core::consent::ConsentPermissions;
        // grant_consent updates in-memory state (no file re-read), so the
        // tempdir can be dropped immediately.
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        cm.grant_consent(
            ConsentPermissions {
                telemetry: consented,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent");
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        PlatformEgressPolicy::new(&config).with_consent_manager(Some(cm))
    }

    #[test]
    fn egress_blocked_when_telemetry_consent_absent() {
        // Even with upload_enabled=true, egress is blocked without telemetry
        // consent (fail-closed).
        let policy = policy_with_telemetry(false);
        assert!(!policy.is_enabled());
        let event = Event::Clipboard(ClipboardEvent {
            timestamp: Utc::now(),
            content_type: ClipboardContentType::Text,
            char_count: 1,
            preview: None,
        });
        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    #[test]
    fn egress_allowed_when_telemetry_consent_present() {
        // telemetry consent + upload_enabled → egress active.
        let policy = policy_with_telemetry(true);
        assert!(policy.is_enabled());
    }

    /// #7728 (ctd-W2 E7) fails-before regression: before this fix,
    /// `telemetry_consented()` used `is_none_or(...)`, which defaults `true`
    /// when no ConsentManager is installed — the ONE fail-OPEN site among the
    /// ~17 hand-composed `effective_permissions()` call sites in src-tauri, in
    /// disagreement with every sibling gate (`integration_runtime.rs`,
    /// `scheduler/loops/system.rs::metrics_collection_permitted`, …), all of
    /// which already defaulted closed. This test (renamed from
    /// `egress_without_consent_manager_preserves_legacy_behavior`, which
    /// asserted the OLD `true` result) proves the corrected default: with
    /// `upload_enabled: true` but no ConsentManager at all, egress must now be
    /// CLOSED, exactly like every other permission tier when no manager is
    /// present (ADR-021/ADR-026 — ONESHIM is a privacy product).
    ///
    /// Revert-proof: restoring the old `is_none_or` composition makes this
    /// test fail (`is_enabled()` would be `true`).
    #[test]
    fn egress_without_consent_manager_fails_closed() {
        assert!(
            !enabled_policy().is_enabled(),
            "no ConsentManager installed must fail-closed telemetry egress (#7728), not fail-open"
        );
    }

    #[test]
    fn egress_blocked_when_telemetry_consent_expired() {
        // #4803: even if an expired consent record still carries the
        // telemetry=true bit, passing through the Valid gate
        // (effective_permissions) must make both the egress decision and the
        // ledger snapshot fail-closed (prevents a fail-open regression).
        use maekon_core::consent::{ConsentPermissions, ConsentRecord, CURRENT_POLICY_VERSION};

        let dir = tempfile::tempdir().expect("tempdir");
        // Write an expired on-disk record (telemetry=true) directly — mirrors
        // the consent.rs:555
        // effective_permissions_valid_gates_sub_tier_fields_when_expired pattern.
        let record = ConsentRecord {
            consent_id: "expired-telemetry".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        let path = dir.path().join("consent_expired.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let cm = Arc::new(ConsentManager::new(path));

        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        let policy = PlatformEgressPolicy::new(&config).with_consent_manager(Some(cm));

        // egress decision: expired consent → blocked.
        assert!(
            !policy.is_enabled(),
            "expired consent must fail-closed for egress"
        );
        // ledger snapshot: must be recorded as telemetry=false.
        assert!(
            policy.consent_state_snapshot().contains("telemetry=false"),
            "ledger snapshot must record telemetry=false on expired consent, got: {}",
            policy.consent_state_snapshot()
        );
    }

    /// #7698 S3 regression: `is_enabled()` must read `monitor.upload_enabled`
    /// LIVE from the shared `ConfigManager` snapshot, exactly like
    /// `tracking_schedule_muted()` already does — NOT the value frozen once
    /// at `PlatformEgressPolicy::new()`. Before this fix, flipping
    /// `upload_enabled` to `false` in the live config after construction had
    /// NO effect on `is_enabled()` for the rest of the scheduler session
    /// (the frozen `enabled: true` field kept winning the AND) — this test
    /// mutates the SAME `ConfigManager` handle the already-built `policy`
    /// holds, without rebuilding `policy`, and proves the flip takes effect
    /// immediately.
    #[test]
    fn is_enabled_reads_upload_enabled_live_without_rebuilding_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = ConfigManager::with_path(dir.path().join("config.json"))
            .expect("ConfigManager::with_path");
        cm.update_with(|cfg| {
            cfg.monitor.upload_enabled = true;
            Ok(())
        })
        .expect("enable upload_enabled");

        let scheduler_config = SchedulerConfig {
            upload_enabled: true, // matches the live config at construction time
            ..Default::default()
        };
        // #7728: telemetry_consented() is now fail-closed with no
        // ConsentManager, so inject a granted one to isolate this test to the
        // upload_enabled dimension (previously relied on the fail-open
        // `is_none_or` default, which #7728 removed).
        let policy = PlatformEgressPolicy::new(&scheduler_config)
            .with_config_manager(Some(cm.clone()))
            .with_consent_manager(Some(granted_telemetry_consent_manager()));

        assert!(
            policy.is_enabled(),
            "upload_enabled=true at construction must be enabled"
        );

        // Flip upload_enabled to false LIVE, on the same ConfigManager handle
        // — `policy` itself is never rebuilt or reconstructed.
        cm.update_with(|cfg| {
            cfg.monitor.upload_enabled = false;
            Ok(())
        })
        .expect("disable upload_enabled");

        assert!(
            !policy.is_enabled(),
            "upload_enabled=false in the live snapshot must disable egress \
             immediately, without rebuilding PlatformEgressPolicy \
             (fails-before: the frozen `enabled` field stayed true)"
        );
        assert!(
            policy
                .consent_state_snapshot()
                .contains("upload_enabled=false"),
            "the egress ledger snapshot string must also reflect the live \
             value, not the frozen construction-time value; got: {}",
            policy.consent_state_snapshot()
        );
    }

    /// Companion to the above: with no `ConfigManager` injected at all, the
    /// frozen `self.enabled` value from construction must still govern
    /// `is_enabled()` — the legacy/no-config-manager fallback path must not
    /// regress. Isolates the upload_enabled dimension from the (now
    /// fail-closed, #7728) telemetry-consent dimension by injecting a granted
    /// consent manager on both policies.
    #[test]
    fn is_enabled_falls_back_to_frozen_value_without_config_manager() {
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        let policy = PlatformEgressPolicy::new(&config)
            .with_consent_manager(Some(granted_telemetry_consent_manager()));
        assert!(policy.is_enabled());

        let config_off = SchedulerConfig {
            upload_enabled: false,
            ..Default::default()
        };
        let policy_off = PlatformEgressPolicy::new(&config_off)
            .with_consent_manager(Some(granted_telemetry_consent_manager()));
        assert!(!policy_off.is_enabled());
    }

    // --- #5992: AllowFiltered + PiiFilterLevel::Off must apply a Basic floor ---

    /// Builds a PlatformEgressPolicy with AllowFiltered policy and the given PII
    /// filter level, with upload enabled (and telemetry consent granted, #7728)
    /// so prepare_event_for_upload reaches the sanitize_title path.
    fn allow_filtered_policy(level: PiiFilterLevel) -> PlatformEgressPolicy {
        let privacy_config = PrivacyConfig {
            pii_filter_level: level,
            ..Default::default()
        };

        let config = SchedulerConfig {
            upload_enabled: true,
            external_data_policy: ExternalDataPolicy::AllowFiltered,
            privacy_config,
            ..Default::default()
        };
        PlatformEgressPolicy::new(&config)
            .with_consent_manager(Some(granted_telemetry_consent_manager()))
    }

    #[test]
    fn allow_filtered_with_pii_off_applies_basic_floor() {
        // A window title containing a plain e-mail address is used as the PII
        // probe: Basic level masks it as "[EMAIL]", Off would leave it verbatim.
        let raw_title = "Login - user@example.com";

        let policy = allow_filtered_policy(PiiFilterLevel::Off);

        let event = Event::Context(ContextEvent {
            app_name: "TestApp".to_string(),
            window_title: raw_title.to_string(),
            ..Default::default()
        });

        let output = policy
            .prepare_event_for_upload(event)
            .expect("AllowFiltered should not drop the event");

        let title = match output {
            Event::Context(ctx) => ctx.window_title,
            other => panic!("expected Context event, got {:?}", other),
        };

        // The Basic floor must have masked the e-mail address.
        assert!(
            title.contains("[EMAIL]"),
            "AllowFiltered+Off: expected e-mail to be masked at Basic floor, got: {title:?}"
        );
        assert!(
            !title.contains("user@example.com"),
            "AllowFiltered+Off: raw e-mail must not appear in upload title, got: {title:?}"
        );
    }

    #[test]
    fn allow_filtered_with_pii_basic_is_unchanged() {
        // When the configured level is already Basic, the floor has no effect and
        // the behaviour must be identical to the pre-fix code path.
        let raw_title = "Login - user@example.com";

        let policy = allow_filtered_policy(PiiFilterLevel::Basic);

        let event = Event::Context(ContextEvent {
            app_name: "TestApp".to_string(),
            window_title: raw_title.to_string(),
            ..Default::default()
        });

        let output = policy
            .prepare_event_for_upload(event)
            .expect("AllowFiltered should not drop the event");

        let title = match output {
            Event::Context(ctx) => ctx.window_title,
            other => panic!("expected Context event, got {:?}", other),
        };

        assert!(
            title.contains("[EMAIL]"),
            "AllowFiltered+Basic: e-mail must still be masked, got: {title:?}"
        );
    }

    #[test]
    fn pii_filter_level_ord_off_less_than_basic() {
        // Verify the Ord derivation encodes Off < Basic < Standard < Strict.
        assert!(PiiFilterLevel::Off < PiiFilterLevel::Basic);
        assert!(PiiFilterLevel::Basic < PiiFilterLevel::Standard);
        assert!(PiiFilterLevel::Standard < PiiFilterLevel::Strict);
        // max() must produce Basic when combining Off and Basic.
        assert_eq!(
            PiiFilterLevel::Off.max(PiiFilterLevel::Basic),
            PiiFilterLevel::Basic
        );
    }
}
