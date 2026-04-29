use chrono::Utc;
use oneshim_core::models::event::{Event, ProcessSnapshotEvent};
use oneshim_monitor::input_activity::InputActivityCollector;
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
            .unwrap_or(oneshim_core::config::PiiFilterLevel::Standard);
        // D5 iter-2: inject VisionPiiSanitizer via PiiSanitizer port so the
        // clipboard preview's sanitize-before-truncate fix takes effect.
        let clipboard_sanitizer: Arc<dyn oneshim_core::ports::pii_sanitizer::PiiSanitizer> =
            Arc::new(oneshim_vision::privacy::VisionPiiSanitizer);
        let clipboard_monitor = Arc::new(
            oneshim_monitor::clipboard::ClipboardMonitor::new(clipboard_pii_level)
                .with_pii_sanitizer(clipboard_sanitizer),
        );

        // File access watcher — polls monitored directories for changes each input tick.
        let file_access_config = self
            .config_manager
            .as_ref()
            .map(|cm| cm.get().file_access.clone())
            .unwrap_or_default();
        let file_watcher = Arc::new(oneshim_monitor::file_access::FileAccessWatcher::new(
            file_access_config,
        ));

        tokio::spawn(async move {
            let mut process_interval = tokio::time::interval(detailed_process_interval);
            let mut input_interval = tokio::time::interval(input_activity_interval);
            let mut foreground_pid: Option<u32> = None;

            loop {
                tokio::select! {
                    _ = process_interval.tick() => {
                        // Row 7: 4-term composite gate (CONS-PC02 / D13).
                        let consent = consent9.as_ref()
                            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                            .unwrap_or_default();
                        let paused = capture_paused9.load(Ordering::Relaxed);
                        let permitted = config9.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
                        if !permitted {
                            debug!("event_snapshot(process): capture gate closed (TS/consent/paused) — skipping tick");
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
                                    if let Some(upload_event) = egress9.prepare_event_for_upload(event) {
                                        sink.enqueue(upload_event);
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
                        // sub-branches all inside this block, so a single gate covers all three
                        // (CONS-PC02 / D13).
                        let consent = consent9.as_ref()
                            .and_then(|cm| cm.current_consent().map(|r| r.permissions.clone()))
                            .unwrap_or_default();
                        let paused = capture_paused9.load(Ordering::Relaxed);
                        let permitted = config9.as_ref()
                            .map(|cm| crate::scheduler::capture_permitted_now(&cm.snapshot(), &consent, paused))
                            .unwrap_or(!paused);
                        if !permitted {
                            debug!("event_snapshot(input/clipboard/file): capture gate closed (TS/consent/paused) — skipping tick");
                            continue;
                        }
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
                                if let Some(upload_event) = egress9.prepare_event_for_upload(event) {
                                    sink.enqueue(upload_event);
                                }
                            }
                        }

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

            let mut interval = tokio::time::interval(Duration::from_secs(60)); // 1min
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
