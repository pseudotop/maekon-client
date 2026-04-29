use chrono::{DateTime, Utc};
use oneshim_core::config::NotificationConfig;
use oneshim_core::ports::notifier::DesktopNotifier;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

#[derive(Debug, Default)]
struct NotificationState {
    last_idle_notification: Option<DateTime<Utc>>,
    last_long_session_notification: Option<DateTime<Utc>>,
    last_high_usage_notification: Option<DateTime<Utc>>,
    session_start: Option<DateTime<Utc>>,
    last_activity: Option<DateTime<Utc>>,
}

pub struct NotificationManager {
    config: RwLock<NotificationConfig>,
    notifier: Arc<dyn DesktopNotifier>,
    state: RwLock<NotificationState>,
}

#[allow(dead_code)] // API surface wired in scheduler notification loop
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

    pub async fn update_config(&self, config: NotificationConfig) {
        let mut current = self.config.write().await;
        *current = config;
        info!("notification settings updated");
    }

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
        let title = "⏰ 휴식 시간 notification";
        let body = if hours > 0 {
            format!(
                "{}시간 Working for {} minutes. Consider taking a short break!",
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

        let title = "⚠️ 시스템 리소스 경고";
        let body = if cpu_percent >= threshold && memory_percent >= threshold {
            format!(
                "CPU {:.1}%, 메모리 {:.1}% 사용 중입니다.",
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
                "고사용량 notification 발송: CPU {:.1}%, Memory {:.1}%",
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

    pub async fn notify(&self, title: &str, body: &str) {
        let config = self.config.read().await;
        if !config.enabled {
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
            return;
        }

        if let Err(e) = self.notifier.show_notification("Maekon Coach", body).await {
            debug!("coaching notification failure: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oneshim_core::models::suggestion::Suggestion;
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
        ) -> Result<(), oneshim_core::error::CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn show_notification(
            &self,
            _title: &str,
            _body: &str,
        ) -> Result<(), oneshim_core::error::CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn show_error(&self, _message: &str) -> Result<(), oneshim_core::error::CoreError> {
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
}
