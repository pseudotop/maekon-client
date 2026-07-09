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
            Self::InvalidRoute => "notification activation route must be an internal app path",
        }
    }
}

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
    route.len() <= 256
        && route.starts_with('/')
        && !route.starts_with("//")
        && !route.contains("://")
        && !route.contains('\\')
        && route
            .chars()
            .all(|ch| !ch.is_control() && !ch.is_whitespace())
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

    // #7719: no settings-update IPC command calls this today — live config
    // changes currently rebuild `NotificationManager` rather than hot-reload
    // it. Kept as the intended hot-reload entry point.
    #[allow(dead_code)]
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
        let config = self.config.read().await;

        if !config.enabled || !config.idle_notification {
            return;
        }

        let threshold_secs = config.idle_notification_mins as u64 * 60;
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
        let config = self.config.read().await;

        if !config.enabled || !config.long_session_notification {
            return;
        }

        let mut state = self.state.write().await;
        let now = Utc::now();

        let session_start = state.session_start.get_or_insert(now);
        let session_mins = (now - *session_start).num_minutes() as u64;

        if session_mins < config.long_session_mins as u64 {
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
        let config = self.config.read().await;

        if !config.enabled || !config.high_usage_notification {
            return;
        }

        let threshold = config.high_usage_threshold as f32;
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

        if let Err(e) = self.notifier.show_notification(title, body).await {
            debug!("notification sent failure: {e}");
        }
    }

    /// Send a coaching notification through the desktop notification system.
    ///
    /// Uses a "Maekon Coach" title prefix to distinguish coaching from system alerts.
    /// Does not enforce its own cooldown — the CoachingEngine already applies per-profile cooldowns.
    pub async fn notify_coaching(&self, body: &str) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::models::suggestion::Suggestion;
    use std::sync::atomic::{AtomicU32, Ordering};

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
}
