// OOS-TBD: ADR-013 file split — baselined past the 900-line giant
// threshold while growing for #9639; split per ADR-003 when next touched.
use chrono::{DateTime, Utc};
use maekon_core::config::NotificationConfig;
use maekon_core::models::suggestion::{Priority, Suggestion};
use maekon_core::ports::notifier::DesktopNotifier;
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

pub const NOTIFICATION_NAVIGATION_EVENT: &str = "navigate";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotificationActivationOutcome {
    pub event_name: &'static str,
    pub route: String,
    pub focus_main_window: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationActivationError {
    MissingRoute,
    InvalidRoute,
}

impl NotificationActivationError {
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingRoute => "notification activation route is required",
            Self::InvalidRoute => "notification activation route is not allowlisted",
        }
    }
}

/// Resolve a notification payload into an allowlisted in-app navigation and
/// focus intent. The Windows WinRT activation adapter and the debug companion
/// both call this seam, so a native action cannot widen the routing policy.
pub fn notification_activation_outcome_from_route(
    route: Option<&str>,
) -> Result<NotificationActivationOutcome, NotificationActivationError> {
    let route = route
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(NotificationActivationError::MissingRoute)?;

    if !is_safe_notification_route(route) {
        return Err(NotificationActivationError::InvalidRoute);
    }

    Ok(NotificationActivationOutcome {
        event_name: NOTIFICATION_NAVIGATION_EVENT,
        route: route.to_string(),
        focus_main_window: true,
    })
}

fn is_safe_notification_route(route: &str) -> bool {
    const ALLOWLIST: &[&str] = &[
        "/replay/timeline",
        "/audit/summary",
        "/audit/entries",
        "/automation/policies",
        "/settings/general",
        "/updates/status",
    ];

    route.len() <= 256 && ALLOWLIST.contains(&route)
}

#[derive(Debug, Default)]
struct NotificationState {
    last_idle_notification: Option<DateTime<Utc>>,
    last_long_session_notification: Option<DateTime<Utc>>,
    last_high_usage_notification: Option<DateTime<Utc>>,
    last_suggestion_notification: Option<DateTime<Utc>>,
    session_start: Option<DateTime<Utc>>,
    last_activity: Option<DateTime<Utc>>,
}

/// Cooldown between suggestion toasts (#5694). Matches the high-usage 300s
/// pattern and the default periodic-analysis interval, so steady state is at
/// most ~one toast per analysis tick. `Priority::Critical` bypasses it.
const SUGGESTION_NOTIFY_COOLDOWN_SECS: i64 = 300;

pub struct NotificationManager {
    config: RwLock<NotificationConfig>,
    notifier: Arc<dyn DesktopNotifier>,
    state: RwLock<NotificationState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NotificationSuppressionLogFields {
    title_present: bool,
    body_present: bool,
    body_len: usize,
}

fn notification_suppression_log_fields(
    title: &str,
    body: &str,
) -> NotificationSuppressionLogFields {
    NotificationSuppressionLogFields {
        title_present: !title.is_empty(),
        body_present: !body.is_empty(),
        body_len: body.len(),
    }
}

impl NotificationManager {
    pub fn new(config: NotificationConfig, notifier: Arc<dyn DesktopNotifier>) -> Self {
        Self {
            config: RwLock::new(config),
            notifier,
            state: RwLock::new(NotificationState {
                session_start: Some(Utc::now()),
                last_activity: Some(Utc::now()),
                ..Default::default()
            }),
        }
    }

    /// Hot-reload entry point, driven by [`spawn_notification_config_watcher`].
    ///
    /// #9639 follow-up: this used to be dead code, so the manager kept its
    /// BOOT snapshot forever. Turning notifications on after launch changed
    /// nothing until a restart (the manager short-circuited on the stale
    /// `enabled: false` before reaching the notifier), which read as a broken
    /// setting. The watcher below now feeds every config write back in.
    pub async fn update_config(&self, config: NotificationConfig) {
        let mut current = self.config.write().await;
        *current = config;
        info!("notification settings updated");
    }

    // #7719: `state.last_activity` is currently only updated by
    // `reset_session()` at session boundaries, not by an explicit external
    // "activity happened" signal — no caller invokes this today.
    #[allow(dead_code)]
    pub async fn record_activity(&self) {
        let mut state = self.state.write().await;
        state.last_activity = Some(Utc::now());
    }

    pub async fn check_idle(&self, idle_secs: u64) {
        // #9691: copy what we need and release the guard before any `.await`.
        // tokio's RwLock is write-preferring, so holding a read guard across
        // `show_notification` queues the config watcher's `write()` behind an
        // in-flight toast AND blocks every new reader until it lands.
        let threshold_secs = {
            let config = self.config.read().await;
            if !config.enabled || !config.idle_notification {
                return;
            }
            config.idle_notification_mins as u64 * 60
        };

        if idle_secs < threshold_secs {
            return;
        }

        let mut state = self.state.write().await;
        let now = Utc::now();
        if let Some(last) = state.last_idle_notification {
            if (now - last).num_seconds() < 600 {
                return;
            }
        }

        let mins = idle_secs / 60;
        let title = "💤 idle state notification";
        let body = format!("No activity for {} minutes. Are you taking a break?", mins);

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            debug!("idle notification failure: {e}");
        } else {
            state.last_idle_notification = Some(now);
            info!("idle notification sent: {}min", mins);
        }
    }

    pub async fn check_long_session(&self) {
        // #9691: see `check_idle` — release the config guard before `.await`.
        let long_session_mins = {
            let config = self.config.read().await;
            if !config.enabled || !config.long_session_notification {
                return;
            }
            config.long_session_mins as u64
        };

        let mut state = self.state.write().await;
        let now = Utc::now();

        let session_start = state.session_start.get_or_insert(now);
        let session_mins = (now - *session_start).num_minutes() as u64;

        if session_mins < long_session_mins {
            return;
        }

        if let Some(last) = state.last_long_session_notification {
            if (now - last).num_seconds() < 1800 {
                return;
            }
        }

        let hours = session_mins / 60;
        let mins = session_mins % 60;
        let title = "⏰ break time notification";
        let body = if hours > 0 {
            format!(
                "Working for {}h {} minutes. Consider taking a short break!",
                hours, mins
            )
        } else {
            format!(
                "Working for {} minutes. Consider taking a short break!",
                mins
            )
        };

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            debug!("hour notification failure: {e}");
        } else {
            state.last_long_session_notification = Some(now);
            info!("hour notification sent: {}min", session_mins);
        }
    }

    pub async fn check_high_usage(&self, cpu_percent: f32, memory_percent: f32) {
        // #9691: see `check_idle` — release the config guard before `.await`.
        let threshold = {
            let config = self.config.read().await;
            if !config.enabled || !config.high_usage_notification {
                return;
            }
            config.high_usage_threshold as f32
        };

        if cpu_percent < threshold && memory_percent < threshold {
            return;
        }

        let mut state = self.state.write().await;
        let now = Utc::now();
        if let Some(last) = state.last_high_usage_notification {
            if (now - last).num_seconds() < 300 {
                return;
            }
        }

        let title = "⚠️ system resource warning";
        let body = if cpu_percent >= threshold && memory_percent >= threshold {
            format!(
                "CPU {:.1}% and memory {:.1}% in use.",
                cpu_percent, memory_percent
            )
        } else if cpu_percent >= threshold {
            format!("CPU usage is {:.1}%.", cpu_percent)
        } else {
            format!("Memory usage is {:.1}%.", memory_percent)
        };

        if let Err(e) = self.notifier.show_notification(title, &body).await {
            debug!("notification failure: {e}");
        } else {
            state.last_high_usage_notification = Some(now);
            info!(
                "high-usage notification sent: CPU {:.1}%, Memory {:.1}%",
                cpu_percent, memory_percent
            );
        }
    }

    pub async fn reset_session(&self) {
        let mut state = self.state.write().await;
        state.session_start = Some(Utc::now());
        state.last_activity = Some(Utc::now());
        debug!("session reset");
    }

    /// Desktop toast for a locally produced suggestion (#5694). Until now only
    /// the server SSE receiver ever called `show_suggestion`, so standalone
    /// suggestions were silent. Rate-limited by [`SUGGESTION_NOTIFY_COOLDOWN_SECS`]
    /// (`Priority::Critical` bypasses the cooldown); the master `enabled`
    /// switch suppresses everything.
    pub async fn notify_suggestion(&self, suggestion: &Suggestion) {
        let config = self.config.read().await;
        if !config.enabled {
            debug!(
                suggestion_id = %suggestion.suggestion_id,
                "suggestion toast suppressed: notifications disabled"
            );
            return;
        }
        drop(config);

        let mut state = self.state.write().await;
        let now = Utc::now();
        if suggestion.priority != Priority::Critical {
            if let Some(last) = state.last_suggestion_notification {
                if (now - last).num_seconds() < SUGGESTION_NOTIFY_COOLDOWN_SECS {
                    debug!(
                        suggestion_id = %suggestion.suggestion_id,
                        "suggestion toast suppressed: cooldown"
                    );
                    return;
                }
            }
        }

        if let Err(e) = self.notifier.show_suggestion(suggestion).await {
            debug!("notification failure: {e}");
        } else {
            state.last_suggestion_notification = Some(now);
        }
    }

    pub async fn notify(&self, title: &str, body: &str) {
        // #9691: see `check_idle` — release the config guard before `.await`.
        {
            let config = self.config.read().await;
            if !config.enabled {
                let fields = notification_suppression_log_fields(title, body);
                info!(
                    reason = "consent_disabled",
                    title_present = fields.title_present,
                    body_present = fields.body_present,
                    body_len = fields.body_len,
                    "notification suppressed: consent_disabled"
                );
                return;
            }
        }

        if let Err(e) = self.notifier.show_notification(title, body).await {
            debug!("notification sent failure: {e}");
        }
    }

    /// Send a coaching notification through the desktop notification system.
    ///
    /// Uses a "Maekon Coach" title prefix to distinguish coaching from system alerts.
    /// Does not enforce its own cooldown — the CoachingEngine already applies per-profile cooldowns.
    pub async fn notify_coaching(&self, body: &str) {
        // #9691: see `check_idle` — release the config guard before `.await`.
        {
            let config = self.config.read().await;
            if !config.enabled {
                let fields = notification_suppression_log_fields("Maekon Coach", body);
                info!(
                    reason = "consent_disabled",
                    title_present = fields.title_present,
                    body_present = fields.body_present,
                    body_len = fields.body_len,
                    "notification suppressed: consent_disabled"
                );
                return;
            }
        }

        if let Err(e) = self.notifier.show_notification("Maekon Coach", body).await {
            debug!("coaching notification failure: {e}");
        }
    }

    /// Desktop toast for a freshly generated daily digest (#7678 D4: wires the
    /// previously-inert `daily_summary_notification` config flag). The aggregation
    /// loop only builds a missing digest once per day, so — unlike idle/long-session/
    /// high-usage — no separate cooldown state is needed here.
    pub async fn notify_daily_summary(&self, date_str: &str) {
        let config = self.config.read().await;
        if !config.enabled || !config.daily_summary_notification {
            return;
        }
        drop(config);

        let title = "📊 daily summary ready";
        let body = format!("Your activity digest for {date_str} is ready to view.");
        if let Err(e) = self.notifier.show_notification(title, &body).await {
            debug!("daily summary notification failure: {e}");
        }
    }
}

// ── TsNotifier impl (#7735 E-3) ───────────────────────────────────────────────
//
// `TsNotifier` (`maekon_core::capture_gate::TsNotifier`) is the narrow port the
// tracking-schedule gate uses to emit enter/exit notifications. This impl used
// to live next to the trait definition when both were in `src-tauri`; now that
// the trait has moved into the tauri-free `maekon-core` crate, the impl stays
// behind here (orphan-rule legal: foreign trait + local type).
#[async_trait::async_trait]
impl maekon_core::capture_gate::TsNotifier for NotificationManager {
    async fn notify_ts(&self, title: &str, body: &str) {
        self.notify(title, body).await;
    }
}

/// #9639 follow-up: keep a live `NotificationManager` in step with config
/// writes.
///
/// `NotificationManager` holds its own `NotificationConfig` (it needs the
/// sub-flags and thresholds on every check), and that copy used to be a boot
/// snapshot — so `notification.enabled` flipped ON after launch stayed
/// invisible until a restart. This watcher subscribes to the shared
/// `ConfigManager` and pushes each NEW notification section into the manager.
///
/// Unchanged sections are skipped so an unrelated settings save does not log a
/// notification update.
///
/// # Why sender-drop cannot be the exit condition
///
/// The obvious shutdown — "end when every `ConfigManager` sender is dropped" —
/// is unreachable in this app, in two independent ways:
///
/// 1. This task reaches a sender through the manager it watches:
///    `Arc<NotificationManager>` → `notifier: Arc<dyn DesktopNotifier>` →
///    `GatedNotifier` → `ConfigManager` → `Arc<Inner>` → `watch::Sender`. And
///    that is the only wired configuration, because the watcher is spawned in
///    the same `Some` branch that builds the `GatedNotifier`. Dropping this
///    function's own `ConfigManager` argument does not cut it.
/// 2. Even with that chain cut, the composition root keeps `ConfigManager`
///    clones alive for the whole process — Tauri managed state
///    (`app_runtime_launch/state_wiring.rs`) among them — so `changed()` would
///    never observe a closed channel anyway.
///
/// So the watcher takes the runtime `shutdown_rx` and exits on it, matching the
/// convention the rest of the runtime uses. `None` means no shutdown signal was
/// wired (minimal/test setups); the task then runs until it is dropped.
pub(crate) fn spawn_notification_config_watcher(
    manager: Arc<NotificationManager>,
    config_manager: maekon_core::config_manager::ConfigManager,
    shutdown_rx: Option<tokio::sync::watch::Receiver<bool>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = config_manager.subscribe();
        // Release our own handle regardless — it is one fewer sender kept alive
        // by the task, even though (per the doc above) it is not what decides
        // termination. `subscribe()` only borrows, so `rx` outlives this.
        drop(config_manager);
        let mut last = { rx.borrow_and_update().notification.clone() };
        // Apply once at start. `subscribe()` marks the current value as seen,
        // so a write landing between manager construction and this line would
        // otherwise be missed forever — and it makes the watcher
        // self-syncing regardless of spawn ordering.
        if *manager.config.read().await != last {
            manager.update_config(last.clone()).await;
        }
        let mut shutdown_rx = shutdown_rx;
        loop {
            let changed = match shutdown_rx.as_mut() {
                Some(shutdown) => {
                    tokio::select! {
                        result = rx.changed() => result.is_ok(),
                        _ = shutdown.changed() => {
                            debug!("notification config watcher stopping (runtime shutdown)");
                            return;
                        }
                    }
                }
                None => rx.changed().await.is_ok(),
            };
            if !changed {
                break;
            }
            // Clone out of the borrow guard before awaiting — the guard is
            // not Send.
            let next = { rx.borrow_and_update().notification.clone() };
            if next != last {
                manager.update_config(next.clone()).await;
                last = next;
            }
        }
        debug!("notification config watcher ended (config channel closed)");
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::models::suggestion::Suggestion;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// #9691: a notifier that parks inside `show_notification` until released,
    /// so a test can observe what the config lock is doing while a toast is in
    /// flight.
    struct BlockingNotifier {
        release: tokio::sync::Semaphore,
        entered: tokio::sync::Semaphore,
    }

    impl BlockingNotifier {
        fn new() -> Self {
            Self {
                release: tokio::sync::Semaphore::new(0),
                entered: tokio::sync::Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl DesktopNotifier for BlockingNotifier {
        async fn show_suggestion(
            &self,
            _suggestion: &Suggestion,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }

        async fn show_notification(
            &self,
            _title: &str,
            _body: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            self.entered.add_permits(1);
            let _ = self.release.acquire().await;
            Ok(())
        }

        async fn show_error(&self, _message: &str) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
    }

    struct MockNotifier {
        call_count: AtomicU32,
    }

    impl MockNotifier {
        fn new() -> Self {
            Self {
                call_count: AtomicU32::new(0),
            }
        }

        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DesktopNotifier for MockNotifier {
        async fn show_suggestion(
            &self,
            _suggestion: &Suggestion,
        ) -> Result<(), maekon_core::error::CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn show_notification(
            &self,
            _title: &str,
            _body: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn show_error(&self, _message: &str) -> Result<(), maekon_core::error::CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn idle_notification_triggers() {
        let config = NotificationConfig {
            enabled: true,
            idle_notification: true,
            idle_notification_mins: 1, // 1 min
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_idle(30).await;
        assert_eq!(notifier.calls(), 0);

        manager.check_idle(60).await;
        assert_eq!(notifier.calls(), 1);

        manager.check_idle(120).await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn disabled_notification_no_trigger() {
        let config = NotificationConfig {
            enabled: false,
            idle_notification: true,
            idle_notification_mins: 1,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_idle(120).await;
        assert_eq!(notifier.calls(), 0);
    }

    #[tokio::test]
    async fn high_usage_notification_triggers() {
        let config = NotificationConfig {
            enabled: true,
            high_usage_notification: true,
            high_usage_threshold: 80,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_high_usage(50.0, 60.0).await;
        assert_eq!(notifier.calls(), 0);

        manager.check_high_usage(85.0, 60.0).await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn memory_high_usage_triggers() {
        let config = NotificationConfig {
            enabled: true,
            high_usage_notification: true,
            high_usage_threshold: 80,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_high_usage(50.0, 90.0).await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn both_cpu_memory_high_triggers_once() {
        let config = NotificationConfig {
            enabled: true,
            high_usage_notification: true,
            high_usage_threshold: 80,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_high_usage(85.0, 90.0).await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn long_session_disabled_no_trigger() {
        let config = NotificationConfig {
            enabled: true,
            long_session_notification: false, // disabled
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_long_session().await;
        assert_eq!(notifier.calls(), 0);
    }

    #[tokio::test]
    async fn notify_disabled_skips() {
        let config = NotificationConfig {
            enabled: false,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.notify("test", "notification body").await;
        assert_eq!(notifier.calls(), 0);
    }

    #[tokio::test]
    async fn notify_disabled_logs_suppression_without_dispatch() {
        let config = NotificationConfig {
            enabled: false,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());
        let fields =
            notification_suppression_log_fields("private title", "private notification body");

        manager
            .notify("private title", "private notification body")
            .await;

        assert_eq!(notifier.calls(), 0);
        assert!(fields.title_present);
        assert!(fields.body_present);
        assert_eq!(fields.body_len, 25);
    }

    #[tokio::test]
    async fn notify_enabled_sends() {
        let config = NotificationConfig {
            enabled: true,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.notify("test", "notification body").await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn reset_session_updates_state() {
        let config = NotificationConfig {
            enabled: true,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.reset_session().await;
        manager.notify("test", "after reset").await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn notify_coaching_sends_when_enabled() {
        let config = NotificationConfig {
            enabled: true,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager
            .notify_coaching("Deep work for 2h. Take a break.")
            .await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn notify_coaching_skips_when_disabled() {
        let config = NotificationConfig {
            enabled: false,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager
            .notify_coaching("Deep work for 2h. Take a break.")
            .await;
        assert_eq!(notifier.calls(), 0);
    }

    #[tokio::test]
    async fn notify_daily_summary_sends_when_flag_enabled() {
        let config = NotificationConfig {
            enabled: true,
            daily_summary_notification: true,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.notify_daily_summary("2026-07-01").await;
        assert_eq!(notifier.calls(), 1);
    }

    #[tokio::test]
    async fn notify_daily_summary_skips_when_flag_disabled() {
        let config = NotificationConfig {
            enabled: true,
            daily_summary_notification: false,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.notify_daily_summary("2026-07-01").await;
        assert_eq!(notifier.calls(), 0);
    }

    #[tokio::test]
    async fn notify_daily_summary_skips_when_master_switch_disabled() {
        let config = NotificationConfig {
            enabled: false,
            daily_summary_notification: true,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.notify_daily_summary("2026-07-01").await;
        assert_eq!(notifier.calls(), 0);
    }

    #[tokio::test]
    async fn update_config_changes_behavior() {
        let config = NotificationConfig {
            enabled: false,
            idle_notification: true,
            idle_notification_mins: 1,
            ..Default::default()
        };
        let notifier = Arc::new(MockNotifier::new());
        let manager = NotificationManager::new(config, notifier.clone());

        manager.check_idle(120).await;
        assert_eq!(notifier.calls(), 0);

        manager
            .update_config(NotificationConfig {
                enabled: true,
                idle_notification: true,
                idle_notification_mins: 1,
                ..Default::default()
            })
            .await;

        manager.check_idle(120).await;
        assert_eq!(notifier.calls(), 1);
    }

    /// #9639 follow-up: a config write after launch must reach the live
    /// manager. Before the watcher existed, `notification.enabled` flipped ON
    /// at runtime stayed invisible until a restart — the manager checked its
    /// boot snapshot and short-circuited before ever calling the notifier.
    #[tokio::test]
    async fn config_watcher_applies_a_runtime_enable_without_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config_path = dir.path().join("config.json");
        let config_manager =
            maekon_core::config_manager::ConfigManager::with_paths(config_path, None)
                .expect("config manager");

        // Boot state: notifications OFF (idle checks must stay silent).
        let mut boot = config_manager.get();
        boot.notification = NotificationConfig {
            enabled: false,
            idle_notification: true,
            idle_notification_mins: 1,
            ..Default::default()
        };
        config_manager.update(boot).expect("seed config");

        let notifier = Arc::new(MockNotifier::new());
        let manager = Arc::new(NotificationManager::new(
            config_manager.get().notification.clone(),
            notifier.clone(),
        ));
        let handle =
            spawn_notification_config_watcher(manager.clone(), config_manager.clone(), None);

        manager.check_idle(120).await;
        assert_eq!(notifier.calls(), 0, "disabled at boot must stay silent");

        // Runtime enable — exactly what a settings save does.
        let mut next = config_manager.get();
        next.notification.enabled = true;
        config_manager.update(next).expect("runtime enable");

        // The watcher runs on its own task; wait for the push to land.
        let mut applied = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if manager.config.read().await.enabled {
                applied = true;
                break;
            }
        }
        assert!(
            applied,
            "config watcher must push the runtime enable through"
        );

        manager.check_idle(120).await;
        assert_eq!(
            notifier.calls(),
            1,
            "an idle check after the runtime enable must notify without a restart"
        );

        handle.abort();
    }

    /// The initial-apply branch closes the construct-vs-subscribe race: a config
    /// write that lands after the manager is built but before the watcher
    /// subscribes is never announced by `changed()`, because `subscribe()` marks
    /// the current value as already seen.
    ///
    /// The test above cannot reach this branch — it builds the manager from the
    /// same snapshot the watcher then reads, so the two already agree. Here the
    /// manager is deliberately built from a STALE snapshot to force the branch,
    /// which is also the one path that would deadlock if the `read()` guard
    /// leaked into `update_config`'s `write()`.
    ///
    /// That failure does NOT surface as the assertion below: tokio's `RwLock` is
    /// write-preferring, so a pending writer in the watcher blocks the poll
    /// loop's own `read()` too, and the test stalls rather than reporting. A
    /// hang here means the guard, not a flake.
    #[tokio::test]
    async fn config_watcher_self_syncs_a_write_that_landed_before_it_subscribed() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config_path = dir.path().join("config.json");
        let config_manager =
            maekon_core::config_manager::ConfigManager::with_paths(config_path, None)
                .expect("config manager");

        // The snapshot the manager is built from — notifications OFF.
        let stale = NotificationConfig {
            enabled: false,
            idle_notification: true,
            idle_notification_mins: 1,
            ..Default::default()
        };

        // Meanwhile the config file already says ON. This is the write that
        // `changed()` will never report.
        let mut current = config_manager.get();
        current.notification = NotificationConfig {
            enabled: true,
            ..stale.clone()
        };
        config_manager.update(current).expect("pre-subscribe write");

        let notifier = Arc::new(MockNotifier::new());
        let manager = Arc::new(NotificationManager::new(stale, notifier.clone()));
        assert!(
            !manager.config.read().await.enabled,
            "manager must start from the stale OFF snapshot"
        );

        let handle =
            spawn_notification_config_watcher(manager.clone(), config_manager.clone(), None);

        // No further config writes happen — only the initial apply can fix this.
        let mut applied = false;
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if manager.config.read().await.enabled {
                applied = true;
                break;
            }
        }
        assert!(
            applied,
            "the initial apply must reconcile a write that predates subscribe()"
        );

        manager.check_idle(120).await;
        assert_eq!(
            notifier.calls(),
            1,
            "the reconciled config must be the one the checks read"
        );

        handle.abort();
    }

    /// #9639 review I1: the watcher must actually stop on the runtime shutdown
    /// signal. Sender-drop cannot serve as its exit condition in production —
    /// the task reaches a `ConfigManager` through its own manager's
    /// `GatedNotifier`, and the composition root holds more clones for the
    /// process lifetime — so this is the only real termination path.
    ///
    /// A mis-wired select arm fails cleanly: the `handle` await is wrapped in a
    /// 5s `tokio::time::timeout`, so the assertion below reports it rather than
    /// letting the harness stall.
    #[tokio::test]
    async fn config_watcher_stops_on_the_runtime_shutdown_signal() {
        let dir = tempfile::tempdir().expect("temp dir");
        let config_path = dir.path().join("config.json");
        let config_manager =
            maekon_core::config_manager::ConfigManager::with_paths(config_path, None)
                .expect("config manager");

        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let notifier = Arc::new(MockNotifier::new());
        let manager = Arc::new(NotificationManager::new(
            config_manager.get().notification.clone(),
            notifier.clone(),
        ));
        let handle = spawn_notification_config_watcher(
            manager.clone(),
            config_manager.clone(),
            Some(shutdown_rx),
        );

        // The config manager is deliberately still alive here — that is the
        // production shape, and the point is that shutdown does not depend on
        // dropping it.
        shutdown_tx.send(true).expect("signal shutdown");

        let ended = tokio::time::timeout(std::time::Duration::from_secs(5), handle).await;
        // The `assert!(ended.is_ok(), ..)` that used to sit here was redundant —
        // the `expect` below already fails on the timeout, and does it while
        // naming the value. Keeping both meant the hedge gate flagged a check
        // that verified nothing the next two lines did not.
        ended
            .expect("watcher must exit on shutdown while the ConfigManager is alive")
            .expect("watcher task should not panic");
    }

    /// #9691: a config write must not queue behind an in-flight toast.
    ///
    /// The five check/notify paths used to hold the config READ guard across
    /// `show_notification().await`. tokio's `RwLock` is write-preferring, so the
    /// watcher's `write()` waited for the toast to finish AND blocked every new
    /// reader in the meantime — one slow toast stalled config propagation and
    /// every other notification path with it.
    ///
    /// A regression fails as a TIMEOUT here, not an assertion: the point is that
    /// `update_config` completes while the notifier is parked.
    ///
    /// SCOPE: the CONFIG guard is what this pins. The `state` guard is still
    /// deliberately held across the toast in `check_idle`/`check_long_session`/
    /// `check_high_usage`/`notify_suggestion`, because the success arm writes
    /// the cooldown timestamp and must not record a toast that failed. So a
    /// parked toast still serialises those paths against each other — it just
    /// no longer blocks config propagation, which is the lock the hot-reload
    /// watcher contends on.
    #[tokio::test]
    async fn a_config_write_is_not_blocked_by_an_in_flight_toast() {
        let notifier = Arc::new(BlockingNotifier::new());
        let manager = Arc::new(NotificationManager::new(
            NotificationConfig {
                enabled: true,
                idle_notification: true,
                idle_notification_mins: 1,
                ..Default::default()
            },
            notifier.clone(),
        ));

        // Park inside show_notification.
        let toast = tokio::spawn({
            let manager = manager.clone();
            async move { manager.check_idle(120).await }
        });
        let _entered = notifier
            .entered
            .acquire()
            .await
            .expect("the toast must reach the notifier");

        // The write must land now, not after the toast is released.
        let write = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            manager.update_config(NotificationConfig {
                enabled: false,
                ..Default::default()
            }),
        )
        .await;
        // `expect` rather than `assert!(write.is_ok(), ..)`: the timeout result
        // carries the reason it elapsed, and a value-blind `is_ok()` throws that
        // away while asserting nothing about what `update_config` actually did.
        write.expect("update_config must not wait for an in-flight toast");
        assert!(
            !manager.config.read().await.enabled,
            "a new reader must also get through while the toast is parked"
        );

        notifier.release.add_permits(1);
        toast.await.expect("toast task should finish");
    }

    #[test]
    fn crt_prv_notif_005_notification_activation_uses_payload_route() {
        let outcome = notification_activation_outcome_from_route(Some("/replay/timeline")).unwrap();

        assert_eq!(outcome.event_name, "navigate");
        assert_eq!(outcome.route, "/replay/timeline");
        assert!(outcome.focus_main_window);
    }

    #[test]
    fn crt_prv_notif_005_notification_activation_rejects_external_url() {
        let err =
            notification_activation_outcome_from_route(Some("https://example.com")).unwrap_err();

        assert_eq!(err, NotificationActivationError::InvalidRoute);
    }

    #[test]
    fn crt_prv_notif_007_notification_activation_rejects_unowned_internal_route() {
        let err = notification_activation_outcome_from_route(Some("/admin/hidden")).unwrap_err();

        assert_eq!(err, NotificationActivationError::InvalidRoute);
    }
}
