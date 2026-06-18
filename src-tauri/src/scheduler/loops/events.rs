use chrono::Utc;
use maekon_core::models::event::{Event, ProcessSnapshotEvent};
use maekon_monitor::input_activity::InputActivityCollector;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::config::PlatformEgressPolicy;
use super::super::Scheduler;
use crate::focus_mode::FocusModeState;

impl Scheduler {
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_event_snapshot_loop(
        &self,
        detailed_process_interval: Duration,
        input_activity_interval: Duration,
        egress_policy: Arc<PlatformEgressPolicy>,
        input_collector: Arc<InputActivityCollector>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let proc_mon9 = self.process_monitor.clone();
        let storage9 = self.storage.clone();
        // #4803: SchedulerStorage handle for writing the egress audit ledger.
        let sqlite9 = self.sqlite_storage.clone();
        let uploader9 = self.batch_sink.clone();
        let input_collector9 = input_collector;
        let egress9 = egress_policy;
        // D13: 4-term privacy gate DI — clone singletons for the async block.
        let config9 = self.config_manager.clone();
        let consent9 = self.consent_manager.clone();
        let capture_paused9 = self.capture_paused.clone();

        // Clipboard monitor — polls system clipboard for changes each input tick.
        let clipboard_pii_level = self
            .config_manager
            .as_ref()
            .map(|cm| cm.get().privacy.pii_filter_level)
            .unwrap_or(maekon_core::config::PiiFilterLevel::Standard);
        // D5 iter-2: inject VisionPiiSanitizer via PiiSanitizer port so the
        // clipboard preview's sanitize-before-truncate fix takes effect.
        let clipboard_sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
            Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
        let clipboard_monitor = Arc::new(
            maekon_monitor::clipboard::ClipboardMonitor::new(clipboard_pii_level)
                .with_pii_sanitizer(clipboard_sanitizer),
        );

        // File access watcher — polls monitored directories for changes each input tick.
        let file_access_config = self
            .config_manager
            .as_ref()
            .map(|cm| cm.get().file_access.clone())
            .unwrap_or_default();
        // review4 monitor (#6298): wire the same PII sanitizer the clipboard
        // monitor above uses, so emitted file paths/filenames (Resume_JohnDoe.pdf,
        // 2024_TaxReturn.xlsx) are masked before reaching SQLite/logs/upload.
        // Previously the watcher was constructed via `new` only, so its filter had
        // no sanitizer and emitted raw filenames.
        let file_access_sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
            Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
        let file_watcher = Arc::new(
            maekon_monitor::file_access::FileAccessWatcher::new(file_access_config)
                .with_pii_sanitizer(file_access_sanitizer, clipboard_pii_level),
        );

        tokio::spawn(async move {
            let mut process_interval =
                super::intervals::coalescing_interval(detailed_process_interval);
            let mut input_interval = super::intervals::coalescing_interval(input_activity_interval);
            let mut foreground_pid: Option<u32> = None;

            loop {
                tokio::select! {
                    _ = process_interval.tick() => {
                        // Row 7: 4-term composite gate (CONS-PC02 / D13).
                        // effective_permissions() returns permissions only in the Valid state —
                        // Expired/UpdateRequired return all-false, so a stale consent record is
                        // also fail-closed (Task 3).
                        let consent = consent9.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused9.load(Ordering::Relaxed);
                        let permitted = config9.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            debug!("event_snapshot(process): capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
                        // Own-field gate: process-snapshot collection requires process_monitoring
                        // consent. Even if only the composite gate (a monitoring bundle such as
                        // screen_capture) is granted, process_monitoring defaults to false, so
                        // processes are not collected. CRITICAL: the value read here is
                        // ConsentPermissions.process_monitoring, not the identically named
                        // MonitorConfig toggle (effective_permissions() returns true only when
                        // Valid — stale consent is also fail-closed).
                        if !consent.process_monitoring {
                            debug!("event_snapshot(process): process_monitoring own-field gate closed — skipping tick");
                            continue;
                        }
                        match proc_mon9.get_detailed_processes(foreground_pid, 10).await {
                            Ok(processes) => {
                                let total = processes.len() as u32;

                                foreground_pid = processes.iter()
                                    .find(|p| p.is_foreground)
                                    .map(|p| p.pid);

                                let snapshot_event = ProcessSnapshotEvent {
                                    timestamp: Utc::now(),
                                    processes,
                                    total_process_count: total,
                                };

                                let event = Event::Process(snapshot_event);
                                if let Err(e) = storage9.save_event(&event).await {
                                    warn!(err.code = %e.code(), "event save failure: {e}");
                                }

                                if let Some(ref sink) = uploader9 {
                                    // #4803: egress audit (uploaded/blocked).
                                    let etype = super::super::config::egress_event_type(&event);
                                    let bytes = super::super::config::egress_byte_count(&event);
                                    let consent_state = egress9.consent_state_snapshot();
                                    if let Some(upload_event) = egress9.prepare_event_for_upload(event) {
                                        sink.enqueue(upload_event);
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "uploaded", &consent_state,
                                        ).await;
                                    } else {
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "blocked", &consent_state,
                                        ).await;
                                    }
                                }

                                debug!(": {}items", total);
                            }
                            Err(e) => {
                                warn!("collect failure: {e}");
                            }
                        }
                    }
                    _ = input_interval.tick() => {
                        // Rows 8-10: 4-term composite gate — input, clipboard, file-access
                        // The three sub-branches in this block (input/clipboard/file) share the
                        // composite gate, but each adds its own-field gate on top
                        // (input_activity / clipboard_monitoring / file_access_monitoring) to
                        // decide honestly per-field (CONS-PC02 / D13).
                        // effective_permissions() returns permissions only in the Valid state —
                        // Expired/UpdateRequired return all-false, so a stale consent record is
                        // also fail-closed (Task 3).
                        let consent = consent9.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused9.load(Ordering::Relaxed);
                        let permitted = config9.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(false);
                        if !permitted {
                            debug!("event_snapshot(input/clipboard/file): capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }

                        // Own-field gate: input-activity collection requires input_activity
                        // consent. CRITICAL: this is ConsentPermissions.input_activity, not the
                        // identically named MonitorConfig toggle (gating on the config toggle
                        // would be meaningless).
                        if consent.input_activity {
                            let input_event = input_collector9.take_snapshot();

                            if input_event.mouse.click_count > 0
                                || input_event.keyboard.total_keystrokes > 0
                                || input_event.mouse.scroll_count > 0
                            {
                                let event = Event::Input(input_event);
                                if let Err(e) = storage9.save_event(&event).await {
                                    warn!(err.code = %e.code(), "event save failure: {e}");
                                }

                                if let Some(ref sink) = uploader9 {
                                    // #4803: egress audit (uploaded/blocked).
                                    let etype = super::super::config::egress_event_type(&event);
                                    let bytes = super::super::config::egress_byte_count(&event);
                                    let consent_state = egress9.consent_state_snapshot();
                                    if let Some(upload_event) = egress9.prepare_event_for_upload(event) {
                                        sink.enqueue(upload_event);
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "uploaded", &consent_state,
                                        ).await;
                                    } else {
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "blocked", &consent_state,
                                        ).await;
                                    }
                                }
                            }
                        }

                        // Own-field gate: clipboard collection requires clipboard_monitoring
                        // consent. Even if only the composite gate (a monitoring bundle such as
                        // screen_capture) is granted, clipboard_monitoring defaults to false, so
                        // the clipboard is not collected → the Clipboard record fields stay honest.
                        // (effective_permissions() returns true only when Valid — stale consent is
                        // also fail-closed.)
                        if consent.clipboard_monitoring {
                            // Poll clipboard for changes (non-blocking on macOS/Linux/Windows).
                            // Runs on the same cadence as input activity collection.
                            let cb = clipboard_monitor.clone();
                            if let Some(clip_event) = tokio::task::spawn_blocking(move || {
                                cb.poll_system_clipboard()
                            }).await.unwrap_or(None) {
                                debug!(
                                    content_type = ?clip_event.content_type,
                                    chars = clip_event.char_count,
                                    "clipboard change detected"
                                );
                                let event = Event::Clipboard(clip_event);
                                if let Err(e) = storage9.save_event(&event).await {
                                    warn!(err.code = %e.code(), "clipboard event save failure: {e}");
                                }
                            }
                        }

                        // Own-field gate: file-access collection requires file_access_monitoring
                        // consent. It defaults to false, so file changes are not collected when
                        // only a monitoring bundle is granted.
                        if consent.file_access_monitoring {
                            // Poll monitored directories for file changes.
                            let fw = file_watcher.clone();
                            let file_events = tokio::task::spawn_blocking(move || {
                                fw.poll_changes()
                            }).await.unwrap_or_default();
                            for file_event in file_events {
                                debug!(
                                    event_type = ?file_event.event_type,
                                    path = %file_event.relative_path.display(),
                                    "file change detected"
                                );
                                let event = Event::FileAccess(file_event);
                                if let Err(e) = storage9.save_event(&event).await {
                                    warn!(err.code = %e.code(), "file access event save failure: {e}");
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("server event collect ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_notification_loop(
        &self,
        focus_mode: Arc<FocusModeState>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let notif7 = self.notification_manager.clone();

        tokio::spawn(async move {
            let notif = match notif7 {
                Some(n) => n,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            let mut interval = super::intervals::coalescing_interval(Duration::from_secs(60)); // 1min
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // A4: Suppress notifications when focus mode active
                        if !focus_mode.is_active() {
                            notif.check_long_session().await;
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("notification ended");
                        break;
                    }
                }
            }
        })
    }
}

/// Unit tests for the consent-decision flow of the clipboard/file-access
/// own-field gates.
///
/// After the input tick of `spawn_event_snapshot_loop` passes the composite
/// gate (`capture_permitted_now`), it additionally gates clipboard/file-access
/// collection on the `consent.clipboard_monitoring` / `consent.file_access_monitoring`
/// own-fields. Here `consent` is the return value of
/// `ConsentManager::effective_permissions()`.
///
/// Constructing a full Scheduler is excessive (the loop requires 16 ports + a
/// tokio runtime), so we verify the *consent-value flow* that directly gates
/// the branches: even when only a monitoring bundle (e.g. `screen_capture`) is
/// granted, the clipboard/file own-fields default to false, so the branch must
/// not be entered; granting them explicitly must enter it. This is not a
/// tautology — it proves (1) a monitoring bundle does not implicitly turn on
/// the clipboard/file fields, and (2) Valid gating + the field-default wiring
/// produce an honest per-field decision. The actual branch wiring
/// (`if consent.clipboard_monitoring { poll... }`) is guaranteed by compilation
/// + code structure.
#[cfg(test)]
mod own_field_gate_tests {
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use std::sync::Arc;

    /// Unique temporary consent-file path for tests (same as the tmp_path
    /// pattern in system.rs).
    fn tmp_consent_path(suffix: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("maekon_events_test_{nonce}_{suffix}"))
    }

    /// Build a ConsentManager that has granted 30-day-valid consent with the
    /// given permissions.
    fn granted(perms: ConsentPermissions, suffix: &str) -> Arc<ConsentManager> {
        let mgr = Arc::new(ConsentManager::new(tmp_consent_path(suffix)));
        mgr.grant_consent(perms, 30).expect("grant_consent failed");
        mgr
    }

    /// Read effective_permissions exactly as the input tick uses it for the
    /// branch gates.
    fn effective(mgr: &Arc<ConsentManager>) -> ConsentPermissions {
        mgr.effective_permissions()
    }

    /// Only the monitoring bundle (`screen_capture:true`) granted +
    /// `clipboard_monitoring:false`: the composite gate passes
    /// (screen_capture=true), but the clipboard branch must not be entered.
    #[test]
    fn clipboard_not_collected_with_only_monitoring_bundle() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                // clipboard_monitoring not specified → defaults to false
                ..Default::default()
            },
            "clip_off.json",
        );
        let consent = effective(&mgr);
        // The composite-gate input (monitoring bundle) is on.
        assert!(
            consent.screen_capture,
            "monitoring bundle is granted — composite gate passes"
        );
        // But the own-field is off, so the clipboard branch is not entered.
        assert!(
            !consent.clipboard_monitoring,
            "without clipboard_monitoring the clipboard branch is gated off (not collected)"
        );
    }

    /// Granting `clipboard_monitoring:true` explicitly must enter the clipboard branch.
    #[test]
    fn clipboard_collected_when_own_field_granted() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                clipboard_monitoring: true,
                ..Default::default()
            },
            "clip_on.json",
        );
        assert!(
            effective(&mgr).clipboard_monitoring,
            "with clipboard_monitoring granted the clipboard branch is entered (collected)"
        );
    }

    /// Only the monitoring bundle granted + `file_access_monitoring:false`:
    /// the file-access branch must not be entered.
    #[test]
    fn file_access_not_collected_with_only_monitoring_bundle() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            "file_off.json",
        );
        let consent = effective(&mgr);
        assert!(consent.screen_capture, "composite gate passes");
        assert!(
            !consent.file_access_monitoring,
            "without file_access_monitoring the file-access branch is gated off (not collected)"
        );
    }

    /// Granting `file_access_monitoring:true` explicitly must enter the file-access branch.
    #[test]
    fn file_access_collected_when_own_field_granted() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                file_access_monitoring: true,
                ..Default::default()
            },
            "file_on.json",
        );
        assert!(
            effective(&mgr).file_access_monitoring,
            "with file_access_monitoring granted the file-access branch is entered (collected)"
        );
    }

    /// #4802 process_monitoring own-field gate: even when only the monitoring
    /// bundle (`screen_capture:true`) is granted, process_monitoring defaults to
    /// false, so the process branch must not be entered.
    #[test]
    fn process_not_collected_with_only_monitoring_bundle() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                // process_monitoring not specified → defaults to false
                ..Default::default()
            },
            "proc_off.json",
        );
        let consent = effective(&mgr);
        assert!(consent.screen_capture, "composite gate passes");
        assert!(
            !consent.process_monitoring,
            "without process_monitoring the process branch is gated off (not collected)"
        );
    }

    /// #4802: granting `process_monitoring:true` explicitly must enter the process branch.
    #[test]
    fn process_collected_when_own_field_granted() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                process_monitoring: true,
                ..Default::default()
            },
            "proc_on.json",
        );
        assert!(
            effective(&mgr).process_monitoring,
            "with process_monitoring granted the process branch is entered (collected)"
        );
    }

    /// #4802 input_activity own-field gate: even when only the monitoring bundle
    /// is granted, input_activity defaults to false, so the input branch must
    /// not be entered.
    #[test]
    fn input_not_collected_with_only_monitoring_bundle() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            "input_off.json",
        );
        let consent = effective(&mgr);
        assert!(consent.screen_capture, "composite gate passes");
        assert!(
            !consent.input_activity,
            "without input_activity the input branch is gated off (not collected)"
        );
    }

    /// #4802: granting `input_activity:true` explicitly must enter the input branch.
    #[test]
    fn input_collected_when_own_field_granted() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                input_activity: true,
                ..Default::default()
            },
            "input_on.json",
        );
        assert!(
            effective(&mgr).input_activity,
            "with input_activity granted the input branch is entered (collected)"
        );
    }

    /// #4802 stale-consent fail-closed: even if process_monitoring/input_activity
    /// are true, an expired record makes effective_permissions() return all-false,
    /// so both branches must be closed (the own-field gate rides on top of Valid
    /// gating).
    #[test]
    fn expired_consent_closes_process_and_input_branches_even_if_fields_true() {
        use maekon_core::consent::{ConsentRecord, CURRENT_POLICY_VERSION};
        let path = tmp_consent_path("proc_input_expired.json");
        let expired = ConsentRecord {
            consent_id: "exp-proc-input".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: chrono::Utc::now() - chrono::Duration::days(2),
            expires_at: Some(chrono::Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true,
                process_monitoring: true,
                input_activity: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr = Arc::new(ConsentManager::new(path));
        let consent = effective(&mgr);
        assert!(
            !consent.process_monitoring,
            "expired consent is fail-closed even with process_monitoring:true"
        );
        assert!(
            !consent.input_activity,
            "expired consent is fail-closed even with input_activity:true"
        );
    }

    /// Stale-consent fail-closed: with `clipboard_monitoring:true` but an expired
    /// record, effective_permissions() returns all-false, so the clipboard branch
    /// must be closed. (Proves the own-field gate rides on top of Valid gating.)
    #[test]
    fn expired_consent_closes_clipboard_branch_even_if_field_true() {
        use maekon_core::consent::{ConsentRecord, CURRENT_POLICY_VERSION};
        let path = tmp_consent_path("clip_expired.json");
        let expired = ConsentRecord {
            consent_id: "exp-clip".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: chrono::Utc::now() - chrono::Duration::days(2),
            expires_at: Some(chrono::Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true,
                clipboard_monitoring: true, // field is on, but the record is expired
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr = Arc::new(ConsentManager::new(path));
        assert!(
            !effective(&mgr).clipboard_monitoring,
            "expired consent is fail-closed even with clipboard_monitoring:true (branch closed)"
        );
    }
}
