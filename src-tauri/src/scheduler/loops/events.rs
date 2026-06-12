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
        // #4803: egress 감사 원장 기록용 SchedulerStorage 핸들.
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
        let file_watcher = Arc::new(maekon_monitor::file_access::FileAccessWatcher::new(
            file_access_config,
        ));

        tokio::spawn(async move {
            let mut process_interval =
                super::intervals::coalescing_interval(detailed_process_interval);
            let mut input_interval = super::intervals::coalescing_interval(input_activity_interval);
            let mut foreground_pid: Option<u32> = None;

            loop {
                tokio::select! {
                    _ = process_interval.tick() => {
                        // Row 7: 4-term composite gate (CONS-PC02 / D13).
                        // effective_permissions()은 Valid 상태일 때만 권한을 반환한다 — Expired/UpdateRequired는
                        // all-false를 반환하므로 스테일 동의 레코드도 fail-closed 처리된다 (Task 3).
                        let consent = consent9.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused9.load(Ordering::Relaxed);
                        let permitted = config9.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
                        if !permitted {
                            debug!("event_snapshot(process): capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
                        // Own-field gate: process 스냅샷 수집은 process_monitoring 동의가 있어야 한다.
                        // 복합 게이트(screen_capture 등 monitoring 번들)만 부여돼도 process_monitoring
                        // 은 기본 false 이므로 프로세스는 수집되지 않는다. CRITICAL: 여기서 읽는 값은
                        // ConsentPermissions 의 process_monitoring 이지 MonitorConfig 의 동명 토글이 아니다
                        // (effective_permissions() 는 Valid 일 때만 true 를 반환 — 스테일 동의도 fail-closed).
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
                                    // #4803: egress 감사 (uploaded/blocked).
                                    let etype = super::super::config::egress_event_type(&event);
                                    let bytes = super::super::config::egress_byte_count(&event);
                                    let consent_state = egress9.consent_state_snapshot();
                                    if let Some(upload_event) = egress9.prepare_event_for_upload(event) {
                                        sink.enqueue(upload_event);
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "uploaded", &consent_state,
                                        );
                                    } else {
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "blocked", &consent_state,
                                        );
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
                        // 이 블록 안의 세 하위 분기(input/clipboard/file)는 복합 게이트를 공유하지만,
                        // 그 위에 각자 own-field 게이트(input_activity / clipboard_monitoring /
                        // file_access_monitoring)를 추가로 얹어 per-field 로 정직하게 결정한다
                        // (CONS-PC02 / D13).
                        // effective_permissions()은 Valid 상태일 때만 권한을 반환한다 — Expired/UpdateRequired는
                        // all-false를 반환하므로 스테일 동의 레코드도 fail-closed 처리된다 (Task 3).
                        let consent = consent9.as_ref()
                            .map(|cm| cm.effective_permissions())
                            .unwrap_or_default();
                        let paused = capture_paused9.load(Ordering::Relaxed);
                        let permitted = config9.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
                        if !permitted {
                            debug!("event_snapshot(input/clipboard/file): capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }

                        // Own-field gate: 입력 활동 수집은 input_activity 동의가 있어야 한다.
                        // CRITICAL: ConsentPermissions 의 input_activity 이지 MonitorConfig 의 동명
                        // 토글이 아니다 (config 토글로 게이트하면 의미가 없어진다).
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
                                    // #4803: egress 감사 (uploaded/blocked).
                                    let etype = super::super::config::egress_event_type(&event);
                                    let bytes = super::super::config::egress_byte_count(&event);
                                    let consent_state = egress9.consent_state_snapshot();
                                    if let Some(upload_event) = egress9.prepare_event_for_upload(event) {
                                        sink.enqueue(upload_event);
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "uploaded", &consent_state,
                                        );
                                    } else {
                                        super::super::config::record_event_egress(
                                            &sqlite9, etype, bytes, "blocked", &consent_state,
                                        );
                                    }
                                }
                            }
                        }

                        // Own-field gate: clipboard 수집은 clipboard_monitoring 동의가 있어야 한다.
                        // 복합 게이트(screen_capture 등 monitoring 번들)만 부여돼도 clipboard_monitoring
                        // 은 기본 false 이므로 클립보드는 수집되지 않는다 → Clipboard 레코드 필드가 정직해진다.
                        // (effective_permissions() 는 Valid 일 때만 true 를 반환 — 스테일 동의도 fail-closed.)
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

                        // Own-field gate: file-access 수집은 file_access_monitoring 동의가 있어야 한다.
                        // 기본 false 이므로 monitoring 번들만 부여된 상태에서는 파일 변경이 수집되지 않는다.
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

/// clipboard/file-access own-field 게이트의 동의 결정 흐름 단위 테스트.
///
/// `spawn_event_snapshot_loop` 의 input tick 은 복합 게이트(`capture_permitted_now`)를
/// 통과한 뒤, clipboard/file-access 수집을 각각 `consent.clipboard_monitoring` /
/// `consent.file_access_monitoring` own-field 로 추가 게이트한다. 여기서 `consent` 는
/// `ConsentManager::effective_permissions()` 의 반환값이다.
///
/// 전체 Scheduler 를 구성하는 것은 과도하므로(loop 는 16개 포트 + tokio 런타임을 요구),
/// 분기를 직접 게이트하는 *동의 값 흐름* 을 검증한다: monitoring 번들(`screen_capture` 등)
/// 만 부여돼도 clipboard/file own-field 는 기본 false 이므로 분기가 진입하지 않아야 하고,
/// 명시적으로 부여하면 진입해야 한다. 이는 동어반복이 아니다 — (1) monitoring 번들이
/// clipboard/file 필드를 암묵적으로 켜지 않음, (2) Valid 게이팅 + 필드 기본값 배선이
/// 정직한 per-field 결정을 만들어냄을 증명한다. 실제 분기 배선
/// (`if consent.clipboard_monitoring { poll... }`)은 컴파일 + 코드 구조로 보증된다.
#[cfg(test)]
mod own_field_gate_tests {
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use std::sync::Arc;

    /// 테스트용 고유 임시 동의 파일 경로 (system.rs 의 tmp_path 패턴과 동일).
    fn tmp_consent_path(suffix: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("maekon_events_test_{nonce}_{suffix}"))
    }

    /// 주어진 권한으로 30일 유효 동의를 부여한 ConsentManager 를 만든다.
    fn granted(perms: ConsentPermissions, suffix: &str) -> Arc<ConsentManager> {
        let mgr = Arc::new(ConsentManager::new(tmp_consent_path(suffix)));
        mgr.grant_consent(perms, 30).expect("동의 부여 실패");
        mgr
    }

    /// input tick 이 분기 게이트에 사용하는 값과 동일하게 effective_permissions 를 읽는다.
    fn effective(mgr: &Arc<ConsentManager>) -> ConsentPermissions {
        mgr.effective_permissions()
    }

    /// monitoring 번들만(`screen_capture:true`) 부여 + `clipboard_monitoring:false`:
    /// 복합 게이트는 통과(screen_capture=true)하지만 clipboard 분기는 진입하지 않아야 한다.
    #[test]
    fn clipboard_not_collected_with_only_monitoring_bundle() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                // clipboard_monitoring 은 명시하지 않음 → 기본 false
                ..Default::default()
            },
            "clip_off.json",
        );
        let consent = effective(&mgr);
        // 복합 게이트 입력(monitoring 번들)은 켜져 있다.
        assert!(
            consent.screen_capture,
            "monitoring 번들은 부여됨 — 복합 게이트는 통과"
        );
        // 하지만 own-field 는 꺼져 있으므로 clipboard 분기는 진입하지 않는다.
        assert!(
            !consent.clipboard_monitoring,
            "clipboard_monitoring 미부여 시 clipboard 분기는 게이트 닫힘 (수집 안 됨)"
        );
    }

    /// `clipboard_monitoring:true` 를 명시 부여하면 clipboard 분기가 진입해야 한다.
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
            "clipboard_monitoring 부여 시 clipboard 분기 진입 (수집됨)"
        );
    }

    /// monitoring 번들만 부여 + `file_access_monitoring:false`:
    /// file-access 분기는 진입하지 않아야 한다.
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
        assert!(consent.screen_capture, "복합 게이트는 통과");
        assert!(
            !consent.file_access_monitoring,
            "file_access_monitoring 미부여 시 file-access 분기는 게이트 닫힘 (수집 안 됨)"
        );
    }

    /// `file_access_monitoring:true` 를 명시 부여하면 file-access 분기가 진입해야 한다.
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
            "file_access_monitoring 부여 시 file-access 분기 진입 (수집됨)"
        );
    }

    /// #4802 process_monitoring own-field 게이트: monitoring 번들만(`screen_capture:true`)
    /// 부여돼도 process_monitoring 은 기본 false 이므로 process 분기는 진입하지 않아야 한다.
    #[test]
    fn process_not_collected_with_only_monitoring_bundle() {
        let mgr = granted(
            ConsentPermissions {
                screen_capture: true,
                // process_monitoring 은 명시하지 않음 → 기본 false
                ..Default::default()
            },
            "proc_off.json",
        );
        let consent = effective(&mgr);
        assert!(consent.screen_capture, "복합 게이트는 통과");
        assert!(
            !consent.process_monitoring,
            "process_monitoring 미부여 시 process 분기는 게이트 닫힘 (수집 안 됨)"
        );
    }

    /// #4802: `process_monitoring:true` 명시 부여 시 process 분기가 진입해야 한다.
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
            "process_monitoring 부여 시 process 분기 진입 (수집됨)"
        );
    }

    /// #4802 input_activity own-field 게이트: monitoring 번들만 부여돼도 input_activity
    /// 는 기본 false 이므로 input 분기는 진입하지 않아야 한다.
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
        assert!(consent.screen_capture, "복합 게이트는 통과");
        assert!(
            !consent.input_activity,
            "input_activity 미부여 시 input 분기는 게이트 닫힘 (수집 안 됨)"
        );
    }

    /// #4802: `input_activity:true` 명시 부여 시 input 분기가 진입해야 한다.
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
            "input_activity 부여 시 input 분기 진입 (수집됨)"
        );
    }

    /// #4802 스테일 동의 fail-closed: process_monitoring/input_activity 가 true 라도
    /// 만료된 레코드는 effective_permissions() 가 all-false 를 반환하므로 두 분기 모두
    /// 닫혀야 한다 (own-field 게이트가 Valid 게이팅 위에 올라타 있음).
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
            "만료 동의는 process_monitoring:true 이더라도 fail-closed"
        );
        assert!(
            !consent.input_activity,
            "만료 동의는 input_activity:true 이더라도 fail-closed"
        );
    }

    /// 스테일 동의 fail-closed: `clipboard_monitoring:true` 이지만 만료된 레코드는
    /// effective_permissions() 가 all-false 를 반환하므로 clipboard 분기가 닫혀야 한다.
    /// (own-field 게이트가 Valid 게이팅 위에 올라타 있음을 증명.)
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
                clipboard_monitoring: true, // 필드는 켜졌으나 레코드가 만료됨
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr = Arc::new(ConsentManager::new(path));
        assert!(
            !effective(&mgr).clipboard_monitoring,
            "만료 동의는 clipboard_monitoring:true 이더라도 fail-closed (분기 닫힘)"
        );
    }
}
