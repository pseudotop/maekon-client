mod models;
mod suggestions;

// ── Public re-exports (external API) ────────────────────────────────
pub use models::{FocusAnalyzerConfig, FocusStorage};

use chrono::Utc;
use oneshim_core::models::work_session::AppCategory;
use oneshim_core::ports::notifier::DesktopNotifier;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::workflow_intelligence::WorkflowIntelligence;

use models::{SessionTracker, SuggestionCooldowns};

pub struct FocusAnalyzer {
    pub(super) config: FocusAnalyzerConfig,
    pub(super) storage: Arc<dyn FocusStorage>,
    pub(super) notifier: Arc<dyn DesktopNotifier>,
    pub(super) tracker: RwLock<SessionTracker>,
    pub(super) cooldowns: RwLock<SuggestionCooldowns>,
    pub(super) workflow_intelligence: RwLock<WorkflowIntelligence>,
}

impl FocusAnalyzer {
    pub fn new(
        config: FocusAnalyzerConfig,
        storage: Arc<dyn FocusStorage>,
        notifier: Arc<dyn DesktopNotifier>,
    ) -> Self {
        Self {
            config,
            storage,
            notifier,
            tracker: RwLock::new(SessionTracker::default()),
            cooldowns: RwLock::new(SuggestionCooldowns::default()),
            workflow_intelligence: RwLock::new(WorkflowIntelligence::default()),
        }
    }

    pub fn with_defaults(
        storage: Arc<dyn FocusStorage>,
        notifier: Arc<dyn DesktopNotifier>,
    ) -> Self {
        Self::new(FocusAnalyzerConfig::default(), storage, notifier)
    }

    #[allow(dead_code)] // convenience wrapper used in tests
    pub async fn on_app_switch(&self, new_app: &str) {
        self.on_app_switch_with_context(new_app, "", None).await;
    }

    pub async fn on_app_switch_with_context(
        &self,
        new_app: &str,
        window_title: &str,
        ocr_hint: Option<&str>,
    ) {
        let new_category = AppCategory::from_app_name(new_app);
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();

        let mut previous_usage: Option<(String, AppCategory, u64)> = None;
        let mut should_suggest_restore = false;

        {
            let mut tracker = self.tracker.write().await;

            let prev_app = tracker.current_app.clone();
            let prev_category = tracker.current_category;
            let prev_start = tracker.current_app_start;

            if prev_app.as_deref() == Some(new_app) {
                return;
            }

            debug!(
                "앱 전환: {:?} ({:?}) → {} ({:?})",
                prev_app, prev_category, new_app, new_category
            );

            if let (Some(prev_app_name), Some(prev_cat), Some(start)) =
                (prev_app, prev_category, prev_start)
            {
                let duration_secs = (now - start).num_seconds().max(0) as u64;
                previous_usage = Some((prev_app_name.clone(), prev_cat, duration_secs));

                let (deep_work, comm) = if prev_cat.is_deep_work() {
                    (duration_secs, 0)
                } else if prev_cat.is_communication() {
                    (0, duration_secs)
                } else {
                    (0, 0)
                };

                if let Err(e) = self.storage.increment_focus_metrics(
                    &today,
                    duration_secs, // total_active
                    deep_work,
                    comm,
                    1, // context_switch
                    0, // interruption
                ) {
                    warn!("in progress min failure: {e}");
                }

                if prev_cat.is_deep_work() {
                    tracker.continuous_deep_work_secs += duration_secs;

                    if let Some(session_id) = tracker.active_session_id {
                        if let Err(e) = self.storage.add_deep_work_secs(session_id, duration_secs) {
                            warn!("session deep_work_secs add failure: {e}");
                        }
                    }
                }

                if prev_cat.is_deep_work() && new_category.is_communication() {
                    let interruption = oneshim_core::models::work_session::Interruption::new(
                        0, // id assigned on persist
                        prev_app_name,
                        new_app.to_string(),
                        None, // snapshot_frame_id (future linkage)
                    );

                    match self.storage.record_interruption(&interruption) {
                        Ok(id) => {
                            debug!("record: id={}", id);
                            tracker.pending_interruption_id = Some(id);

                            if let Some(session_id) = tracker.active_session_id {
                                if let Err(e) =
                                    self.storage.increment_work_session_interruption(session_id)
                                {
                                    debug!("increment_work_session_interruption failed: {e}");
                                }
                            }

                            if let Err(e) =
                                self.storage.increment_focus_metrics(&today, 0, 0, 0, 0, 1)
                            {
                                debug!("increment_focus_metrics (interruption) failed: {e}");
                            }
                        }
                        Err(e) => warn!("record failure: {e}"),
                    }
                }

                if prev_cat.is_communication() && new_category.is_deep_work() {
                    if let Some(int_id) = tracker.pending_interruption_id.take() {
                        if let Err(e) = self.storage.record_interruption_resume(int_id, new_app) {
                            debug!("record_interruption_resume failed: {e}");
                        }
                        debug!(": id={}", int_id);
                        should_suggest_restore = true;
                    }
                }
            }

            if new_category.is_communication() {
                if let Some(session_id) = tracker.active_session_id.take() {
                    if let Err(e) = self.storage.end_work_session(session_id) {
                        debug!("end_work_session failed: {e}");
                    }
                    tracker.continuous_deep_work_secs = 0;
                    debug!("session ended ( switch): id={}", session_id);
                }
            } else if new_category.is_deep_work() && tracker.active_session_id.is_none() {
                match self.storage.start_work_session(new_app, new_category) {
                    Ok(session) => {
                        debug!("session started: id={}, app={}", session.id, new_app);
                        tracker.active_session_id = Some(session.id);
                    }
                    Err(e) => warn!("session started failure: {e}"),
                }
            }

            tracker.current_app = Some(new_app.to_string());
            tracker.current_category = Some(new_category);
            tracker.current_app_start = Some(now);
        }

        let playbook_signal = {
            let mut intelligence = self.workflow_intelligence.write().await;

            if let Some((prev_app, prev_cat, duration_secs)) = previous_usage {
                let score = intelligence.update_usage(&prev_app, prev_cat, duration_secs, now);
                debug!(
                    app = %prev_app,
                    category = ?prev_cat,
                    duration_secs,
                    relevance = score,
                    "앱 relevance update"
                );
            }

            let _ = intelligence.touch_app(new_app, new_category, now);
            intelligence.advance_workflow(
                new_app,
                new_category,
                window_title,
                ocr_hint,
                now,
                self.config.playbook_min_relevance,
                self.config.workflow_split_idle_secs,
            )
        };

        if let Some(signal) = playbook_signal {
            self.maybe_suggest_pattern_detected(signal).await;
        }

        if should_suggest_restore {
            self.maybe_suggest_restore_context(new_app, now).await;
        }
    }

    pub async fn analyze_periodic(&self) {
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();

        let metrics = match self.storage.get_or_create_focus_metrics(&today) {
            Ok(m) => m,
            Err(e) => {
                warn!("in progress query failure: {e}");
                return;
            }
        };

        let focus_score = self.calculate_focus_score(&metrics);
        if (focus_score - metrics.focus_score).abs() > 0.01 {
            let mut updated = metrics.clone();
            updated.focus_score = focus_score;
            if let Err(e) = self.storage.update_focus_metrics(&today, &updated) {
                debug!("update_focus_metrics failed: {e}");
            }
        }

        self.maybe_suggest_break().await;

        self.maybe_suggest_focus_time(&metrics).await;

        let playbook_signal = {
            let mut intelligence = self.workflow_intelligence.write().await;
            intelligence.flush_stale_segment(
                now,
                self.config.playbook_min_relevance,
                self.config.playbook_stale_flush_secs,
            )
        };
        if let Some(signal) = playbook_signal {
            self.maybe_suggest_pattern_detected(signal).await;
        }

        debug!(
            "집중도 분석: score={:.2}, deep_work={}초, comm={}초, interruptions={}",
            focus_score,
            metrics.deep_work_secs,
            metrics.communication_secs,
            metrics.interruption_count
        );
    }

    #[allow(dead_code)] // convenience wrapper used in tests
    pub async fn on_idle_resume(&self) {
        let now = Utc::now();
        let playbook_signal = {
            let mut intelligence = self.workflow_intelligence.write().await;
            intelligence.flush_stale_segment(now, self.config.playbook_min_relevance, 0)
        };

        let mut tracker = self.tracker.write().await;

        if let Some(session_id) = tracker.active_session_id.take() {
            if let Err(e) = self.storage.end_work_session(session_id) {
                debug!("end_work_session (idle resume) failed: {e}");
            }
        }

        tracker.continuous_deep_work_secs = 0;
        tracker.pending_interruption_id = None;
        tracker.current_app = None;
        tracker.current_category = None;
        tracker.current_app_start = None;

        debug!("session reset (idle )");

        if let Some(signal) = playbook_signal {
            self.maybe_suggest_pattern_detected(signal).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Duration;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::suggestion::Suggestion;
    use oneshim_core::models::work_session::FocusMetrics;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tempfile::TempDir;

    struct MockNotifier {
        call_count: AtomicU32,
    }

    impl MockNotifier {
        fn new() -> Self {
            Self {
                call_count: AtomicU32::new(0),
            }
        }

        #[allow(dead_code)]
        fn calls(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl DesktopNotifier for MockNotifier {
        async fn show_suggestion(&self, _: &Suggestion) -> Result<(), CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn show_notification(&self, _: &str, _: &str) -> Result<(), CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn show_error(&self, _: &str) -> Result<(), CoreError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    async fn create_test_analyzer() -> (FocusAnalyzer, TempDir, Arc<MockNotifier>) {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(
            SqliteStorage::open(&temp_dir.path().join("test.db"), 30, None)
                .expect("storage creation failed"),
        );
        let notifier = Arc::new(MockNotifier::new());

        let analyzer = FocusAnalyzer::with_defaults(storage, notifier.clone());
        (analyzer, temp_dir, notifier)
    }

    #[tokio::test]
    async fn app_switch_updates_tracker() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        analyzer.on_app_switch("Visual Studio Code").await;

        let tracker = analyzer.tracker.read().await;
        assert_eq!(tracker.current_app, Some("Visual Studio Code".to_string()));
        assert_eq!(tracker.current_category, Some(AppCategory::Development));
    }

    #[tokio::test]
    async fn deep_work_to_communication_creates_interruption() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        analyzer.on_app_switch("Visual Studio Code").await;

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        analyzer.on_app_switch("Slack").await;

        let tracker = analyzer.tracker.read().await;
        assert!(tracker.pending_interruption_id.is_some());
    }

    #[tokio::test]
    async fn focus_score_calculation() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        let now = Utc::now();
        let metrics = FocusMetrics {
            period: oneshim_core::types::TimeWindow::new(now, now + Duration::hours(8))
                .expect("trusted test bounds: now <= now + 8h"),
            total_active_secs: 3600,  // 1 hour
            deep_work_secs: 2400,     // 40 min
            communication_secs: 1200, // 20 min
            context_switches: 10,
            interruption_count: 3,
            avg_focus_duration_secs: 600,
            max_focus_duration_secs: 1200,
            focus_score: 0.0,
        };

        let score = analyzer.calculate_focus_score(&metrics);
        assert!(score > 0.1 && score < 0.3, "score was {}", score);
    }

    #[tokio::test]
    async fn idle_resume_resets_session() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        analyzer.on_app_switch("Visual Studio Code").await;

        analyzer.on_idle_resume().await;

        let tracker = analyzer.tracker.read().await;
        assert!(tracker.active_session_id.is_none());
        assert!(tracker.current_app.is_none());
        assert_eq!(tracker.continuous_deep_work_secs, 0);
    }

    #[tokio::test]
    async fn focus_score_zero_active_secs() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        let now = Utc::now();
        let metrics = FocusMetrics {
            period: oneshim_core::types::TimeWindow::new(now, now + Duration::hours(8))
                .expect("trusted test bounds: now <= now + 8h"),
            total_active_secs: 0,
            deep_work_secs: 0,
            communication_secs: 0,
            context_switches: 0,
            interruption_count: 0,
            avg_focus_duration_secs: 0,
            max_focus_duration_secs: 0,
            focus_score: 0.0,
        };

        let score = analyzer.calculate_focus_score(&metrics);
        assert_eq!(score, 0.0);
    }

    #[tokio::test]
    async fn focus_score_max_interruptions_clamped() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        let now = Utc::now();
        let metrics = FocusMetrics {
            period: oneshim_core::types::TimeWindow::new(now, now + Duration::hours(8))
                .expect("trusted test bounds: now <= now + 8h"),
            total_active_secs: 3600,
            deep_work_secs: 3600,
            communication_secs: 0,
            context_switches: 100,
            interruption_count: 100,
            avg_focus_duration_secs: 36,
            max_focus_duration_secs: 36,
            focus_score: 0.0,
        };

        let score = analyzer.calculate_focus_score(&metrics);
        assert!((0.0..=1.0).contains(&score), "score was {}", score);
        assert!((score - 0.2).abs() < 0.01, "score was {}", score);
    }

    #[tokio::test]
    async fn multiple_app_switches_tracking() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        analyzer.on_app_switch("Visual Studio Code").await;
        {
            let tracker = analyzer.tracker.read().await;
            assert_eq!(tracker.current_app, Some("Visual Studio Code".to_string()));
            assert_eq!(tracker.current_category, Some(AppCategory::Development));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        analyzer.on_app_switch("Google Chrome").await;
        {
            let tracker = analyzer.tracker.read().await;
            assert_eq!(tracker.current_app, Some("Google Chrome".to_string()));
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        analyzer.on_app_switch("Terminal").await;
        {
            let tracker = analyzer.tracker.read().await;
            assert_eq!(tracker.current_app, Some("Terminal".to_string()));
            assert_eq!(tracker.current_category, Some(AppCategory::Development));
        }
    }

    #[tokio::test]
    async fn same_app_switch_no_change() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        analyzer.on_app_switch("Visual Studio Code").await;
        analyzer.on_app_switch("Visual Studio Code").await;
        let tracker = analyzer.tracker.read().await;
        assert_eq!(tracker.current_app, Some("Visual Studio Code".to_string()));
    }
}
