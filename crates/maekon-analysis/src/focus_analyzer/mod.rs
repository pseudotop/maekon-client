mod models;
mod suggestions;

// ── Public re-exports (external API) ────────────────────────────────
pub use models::FocusAnalyzerConfig;

use chrono::Utc;
use maekon_core::consent::ConsentPermissions;
use maekon_core::models::work_session::AppCategory;
// #7735 E-2: internal use only — the `FocusStorage` pass-through re-export
// was dropped from the public path (see `models.rs`); consumers import the
// SSOT `maekon_core::ports::focus_storage::FocusStorage` directly.
use maekon_core::ports::focus_storage::FocusStorage;
use maekon_core::ports::notifier::DesktopNotifier;
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

    // Test convenience wrapper: grants app_usage_analytics consent (true) so the
    // full path is active. No production caller. #7735 E-2: gated
    // `#[cfg(any(test, feature = "test-support"))]` rather than plain
    // `#[cfg(test)]` — this crate is a normal (non-test) dependency of
    // `maekon-app`, and `maekon-app`'s own `#[cfg(test)]` tests (e.g.
    // scheduler/loops/helpers/mod.rs) call this method across the crate
    // boundary. Plain `#[cfg(test)]` only activates when THIS crate compiles
    // under `--test`, not when it is a normal dependency of another crate's
    // test build (#7729 house pattern; consumer enables `test-support` via
    // `[dev-dependencies]` only).
    #[cfg(any(test, feature = "test-support"))]
    pub async fn on_app_switch(&self, new_app: &str) {
        let consent = ConsentPermissions {
            app_usage_analytics: true,
            activity_pattern_learning: true,
            ..Default::default()
        };
        self.on_app_switch_with_context(new_app, "", None, &consent)
            .await;
    }

    /// Handle an app switch using one live, fail-closed consent snapshot.
    ///
    /// `app_usage_analytics` gates only usage aggregation (`update_usage` and
    /// `touch_app`). `activity_pattern_learning` independently gates workflow
    /// segments, learned playbooks, and pattern suggestions. Focus session and
    /// interruption tracking are protected by the caller's separate composite
    /// capture gate and remain unchanged here.
    pub async fn on_app_switch_with_context(
        &self,
        new_app: &str,
        window_title: &str,
        ocr_hint: Option<&str>,
        consent: &ConsentPermissions,
    ) -> Vec<maekon_core::models::suggestion::Suggestion> {
        self.reconcile_activity_pattern_consent(consent).await;

        let new_category = AppCategory::from_app_name(new_app);
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();

        let mut previous_usage: Option<(String, AppCategory, u64)> = None;
        let mut resumed_interruption = None;

        {
            let mut tracker = self.tracker.write().await;

            let prev_app = tracker.current_app.clone();
            let prev_category = tracker.current_category;
            let prev_start = tracker.current_app_start;

            if prev_app.as_deref() == Some(new_app) {
                return Vec::new();
            }

            debug!(
                "app switch: {:?} ({:?}) → {} ({:?})",
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

                if let Err(e) = self
                    .storage
                    .increment_focus_metrics(
                        &today,
                        duration_secs, // total_active
                        deep_work,
                        comm,
                        1, // context_switch
                        0, // interruption
                    )
                    .await
                {
                    warn!("in progress min failure: {e}");
                }

                if prev_cat.is_deep_work() {
                    tracker.continuous_deep_work_secs += duration_secs;

                    if let Some(session_id) = tracker.active_session_id {
                        if let Err(e) = self
                            .storage
                            .add_deep_work_secs(session_id, duration_secs)
                            .await
                        {
                            warn!("session deep_work_secs add failure: {e}");
                        }
                    }
                }

                if prev_cat.is_deep_work() && new_category.is_communication() {
                    let interruption = maekon_core::models::work_session::Interruption::new(
                        0, // id assigned on persist
                        prev_app_name,
                        new_app.to_string(),
                        None, // snapshot_frame_id (future linkage)
                    );

                    match self.storage.record_interruption(&interruption).await {
                        Ok(id) => {
                            debug!("record: id={}", id);
                            tracker.pending_interruption_id = Some(id);

                            if let Some(session_id) = tracker.active_session_id {
                                if let Err(e) = self
                                    .storage
                                    .increment_work_session_interruption(session_id)
                                    .await
                                {
                                    debug!("increment_work_session_interruption failed: {e}");
                                }
                            }

                            if let Err(e) = self
                                .storage
                                .increment_focus_metrics(&today, 0, 0, 0, 0, 1)
                                .await
                            {
                                debug!("increment_focus_metrics (interruption) failed: {e}");
                            }
                        }
                        Err(e) => warn!("record failure: {e}"),
                    }
                }

                if prev_cat.is_communication() && new_category.is_deep_work() {
                    if let Some(int_id) = tracker.pending_interruption_id {
                        match self.storage.resume_interruption(int_id, new_app, now).await {
                            Ok(Some(interruption)) => {
                                tracker.pending_interruption_id = None;
                                debug!("interruption resumed: id={}", int_id);
                                resumed_interruption = Some(interruption);
                            }
                            Ok(None) => {
                                debug!(
                                    "pending interruption was not resumed; preserving tracker id: id={}",
                                    int_id
                                );
                            }
                            Err(e) => {
                                debug!(
                                    "resume_interruption failed; preserving tracker id {}: {e}",
                                    int_id
                                );
                            }
                        }
                    }
                }
            }

            if new_category.is_communication() {
                if let Some(session_id) = tracker.active_session_id.take() {
                    if let Err(e) = self.storage.end_work_session(session_id).await {
                        debug!("end_work_session failed: {e}");
                    }
                    tracker.continuous_deep_work_secs = 0;
                    debug!("session ended ( switch): id={}", session_id);
                }
            } else if new_category.is_deep_work() && tracker.active_session_id.is_none() {
                match self.storage.start_work_session(new_app, new_category).await {
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

            if consent.app_usage_analytics {
                if let Some((prev_app, prev_cat, duration_secs)) = previous_usage {
                    let score = intelligence.update_usage(&prev_app, prev_cat, duration_secs, now);
                    debug!(
                        app = %prev_app,
                        category = ?prev_cat,
                        duration_secs,
                        relevance = score,
                        "app relevance update"
                    );
                }

                let _ = intelligence.touch_app(new_app, new_category, now);
            } else {
                debug!("on_app_switch: app_usage_analytics own-field gate closed — skipping usage aggregation");
            }

            if consent.activity_pattern_learning {
                intelligence.advance_workflow(
                    new_app,
                    new_category,
                    window_title,
                    ocr_hint,
                    now,
                    self.config.playbook_min_relevance,
                    self.config.workflow_split_idle_secs,
                )
            } else {
                None
            }
        };

        // #5696: collect produced rule suggestions so the scheduler can bridge
        // them into the live review queue (save + OS toast already happened
        // inside each maybe_suggest_*).
        let mut produced = Vec::new();
        if let Some(signal) = playbook_signal {
            if let Some(s) = self.maybe_suggest_pattern_detected(signal).await {
                produced.push(s);
            }
        }

        if let Some(interruption) = resumed_interruption.as_ref() {
            if let Some(s) = self.maybe_suggest_restore_context(interruption, now).await {
                produced.push(s);
            }
        }
        produced
    }

    pub async fn analyze_periodic(
        &self,
        consent: &ConsentPermissions,
    ) -> Vec<maekon_core::models::suggestion::Suggestion> {
        self.reconcile_activity_pattern_consent(consent).await;

        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();

        let metrics = match self.storage.get_or_create_focus_metrics(&today).await {
            Ok(m) => m,
            Err(e) => {
                warn!("in progress query failure: {e}");
                return Vec::new();
            }
        };

        let focus_score = self.calculate_focus_score(&metrics);
        if (focus_score - metrics.focus_score).abs() > 0.01 {
            let mut updated = metrics.clone();
            updated.focus_score = focus_score;
            if let Err(e) = self.storage.update_focus_metrics(&today, &updated).await {
                debug!("update_focus_metrics failed: {e}");
            }
        }

        let mut produced = Vec::new();
        if let Some(s) = self.maybe_suggest_break().await {
            produced.push(s);
        }
        if let Some(s) = self.maybe_suggest_focus_time(&metrics).await {
            produced.push(s);
        }

        let playbook_signal = if consent.activity_pattern_learning {
            let mut intelligence = self.workflow_intelligence.write().await;
            intelligence.flush_stale_segment(
                now,
                self.config.playbook_min_relevance,
                self.config.playbook_stale_flush_secs,
            )
        } else {
            None
        };
        if let Some(signal) = playbook_signal {
            if let Some(s) = self.maybe_suggest_pattern_detected(signal).await {
                produced.push(s);
            }
        }

        debug!(
            "focus analysis: score={:.2}, deep_work={}s, comm={}s, interruptions={}",
            focus_score,
            metrics.deep_work_secs,
            metrics.communication_secs,
            metrics.interruption_count
        );
        produced
    }

    pub async fn on_idle_resume(
        &self,
        consent: &ConsentPermissions,
    ) -> Vec<maekon_core::models::suggestion::Suggestion> {
        self.reconcile_activity_pattern_consent(consent).await;

        let now = Utc::now();
        let playbook_signal = if consent.activity_pattern_learning {
            let mut intelligence = self.workflow_intelligence.write().await;
            intelligence.flush_stale_segment(now, self.config.playbook_min_relevance, 0)
        } else {
            None
        };

        let mut tracker = self.tracker.write().await;

        if let Some(session_id) = tracker.active_session_id.take() {
            if let Err(e) = self.storage.end_work_session(session_id).await {
                debug!("end_work_session (idle resume) failed: {e}");
            }
        }

        tracker.continuous_deep_work_secs = 0;
        tracker.pending_interruption_id = None;
        tracker.current_app = None;
        tracker.current_category = None;
        tracker.current_app_start = None;

        debug!("session reset (idle )");

        let mut produced = Vec::new();
        if let Some(signal) = playbook_signal {
            if let Some(s) = self.maybe_suggest_pattern_detected(signal).await {
                produced.push(s);
            }
        }
        produced
    }

    /// Apply the live activity-pattern consent term to in-memory state.
    ///
    /// The consent IPC path calls this immediately after a grant narrowing or
    /// revoke, while switch/periodic/idle paths call it with their own live
    /// effective snapshot. Closing the term is idempotent and never removes the
    /// separately consented usage aggregation map.
    pub async fn reconcile_activity_pattern_consent(&self, consent: &ConsentPermissions) {
        if consent.activity_pattern_learning {
            return;
        }

        let mut intelligence = self.workflow_intelligence.write().await;
        if intelligence.clear_pattern_state() {
            debug!(
                "activity_pattern_learning gate closed — cleared in-memory workflow pattern state"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Duration;
    use maekon_core::error::CoreError;
    use maekon_core::error_codes::StorageCode;
    use maekon_core::models::suggestion::{Suggestion, SuggestionType};
    use maekon_core::models::work_session::{FocusMetrics, Interruption, WorkSession};
    use maekon_storage::sqlite::SqliteStorage;
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

        // Mock accessor; no current test reads the call count back.
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

    struct ResumeFailingStorage;

    #[async_trait]
    impl FocusStorage for ResumeFailingStorage {
        async fn increment_focus_metrics(
            &self,
            _: &str,
            _: u64,
            _: u64,
            _: u64,
            _: u32,
            _: u32,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn add_deep_work_secs(&self, _: i64, _: u64) -> Result<(), CoreError> {
            Ok(())
        }

        async fn record_interruption(&self, _: &Interruption) -> Result<i64, CoreError> {
            unreachable!("failure test starts from an existing communication interruption")
        }

        async fn increment_work_session_interruption(&self, _: i64) -> Result<(), CoreError> {
            Ok(())
        }

        async fn resume_interruption(
            &self,
            _: i64,
            _: &str,
            _: chrono::DateTime<Utc>,
        ) -> Result<Option<Interruption>, CoreError> {
            Err(CoreError::Storage {
                code: StorageCode::Failed,
                message: "injected resume failure".to_string(),
            })
        }

        async fn end_work_session(&self, _: i64) -> Result<(), CoreError> {
            Ok(())
        }

        async fn start_work_session(
            &self,
            primary_app: &str,
            _: AppCategory,
        ) -> Result<WorkSession, CoreError> {
            Ok(WorkSession::new(1, primary_app.to_string()))
        }

        async fn get_or_create_focus_metrics(&self, _: &str) -> Result<FocusMetrics, CoreError> {
            unreachable!("failure test does not query focus metrics")
        }

        async fn update_focus_metrics(&self, _: &str, _: &FocusMetrics) -> Result<(), CoreError> {
            unreachable!("failure test does not update aggregate focus metrics")
        }

        async fn save_rule_suggestion(&self, _: &Suggestion) -> Result<String, CoreError> {
            unreachable!("failed resume must not create a suggestion")
        }

        async fn mark_suggestion_shown_by_id(&self, _: &str) -> Result<(), CoreError> {
            unreachable!("failed resume must not mark a suggestion shown")
        }

        async fn get_pending_interruption(&self) -> Result<Option<Interruption>, CoreError> {
            unreachable!("RestoreContext uses the committed resume snapshot directly")
        }
    }

    async fn create_test_analyzer_with_storage() -> (
        FocusAnalyzer,
        TempDir,
        Arc<MockNotifier>,
        Arc<SqliteStorage>,
    ) {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(
            SqliteStorage::open(&temp_dir.path().join("test.db"), 30, None)
                .expect("storage creation failed"),
        );
        let notifier = Arc::new(MockNotifier::new());

        let analyzer = FocusAnalyzer::with_defaults(storage.clone(), notifier.clone());
        (analyzer, temp_dir, notifier, storage)
    }

    async fn create_test_analyzer() -> (FocusAnalyzer, TempDir, Arc<MockNotifier>) {
        let (analyzer, temp_dir, notifier, _storage) = create_test_analyzer_with_storage().await;
        (analyzer, temp_dir, notifier)
    }

    fn consent(app_usage: bool, pattern_learning: bool) -> ConsentPermissions {
        ConsentPermissions {
            app_usage_analytics: app_usage,
            activity_pattern_learning: pattern_learning,
            ..Default::default()
        }
    }

    async fn test_switch(
        analyzer: &FocusAnalyzer,
        new_app: &str,
    ) -> Vec<maekon_core::models::suggestion::Suggestion> {
        analyzer
            .on_app_switch_with_context(new_app, "", None, &consent(true, true))
            .await
    }

    #[tokio::test]
    async fn app_switch_updates_tracker() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        analyzer.on_app_switch("Visual Studio Code").await;

        let tracker = analyzer.tracker.read().await;
        assert_eq!(tracker.current_app, Some("Visual Studio Code".to_string()));
        assert_eq!(tracker.current_category, Some(AppCategory::Development));
    }

    /// Own-field gate (#4802): when app_usage_analytics consent is absent (=false),
    /// nothing should accumulate in the app-usage aggregation
    /// (WorkflowIntelligence usage/segment).
    /// Focus session tracking (tracker) is governed by a separate composite gate, so
    /// it is not disabled here — the tracker is still updated, but usage aggregation
    /// must remain empty.
    #[tokio::test]
    async fn app_usage_not_aggregated_with_only_monitoring_bundle() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        let permissions = consent(false, false);
        analyzer
            .on_app_switch_with_context("Visual Studio Code", "main.rs", None, &permissions)
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        analyzer
            .on_app_switch_with_context("Slack", "general", None, &permissions)
            .await;

        let intelligence = analyzer.workflow_intelligence.read().await;
        assert_eq!(
            intelligence.usage_len(),
            0,
            "usage aggregation must be empty when app_usage_analytics is not granted"
        );
        assert!(
            !intelligence.has_active_segment(),
            "no workflow segment must start when app_usage_analytics is not granted"
        );
    }

    /// Own-field gate (#4802): when app_usage_analytics consent is present (=true),
    /// app-usage aggregation must work normally.
    #[tokio::test]
    async fn app_usage_aggregated_when_own_field_granted() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;
        let permissions = consent(true, true);

        analyzer
            .on_app_switch_with_context("Visual Studio Code", "main.rs", None, &permissions)
            .await;
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        analyzer
            .on_app_switch_with_context("Slack", "general", None, &permissions)
            .await;

        let intelligence = analyzer.workflow_intelligence.read().await;
        assert!(
            intelligence.usage_len() > 0,
            "usage aggregation must be populated when app_usage_analytics is granted"
        );
        assert!(
            intelligence.has_active_segment(),
            "workflow segment must advance when app_usage_analytics is granted"
        );
    }

    /// #8574: the two own-field permissions form an independent 2x2 matrix.
    /// Usage aggregation follows only `app_usage_analytics`; workflow segments
    /// follow only `activity_pattern_learning`.
    #[tokio::test]
    async fn app_usage_and_pattern_learning_follow_independent_consent_matrix() {
        for (app_usage, pattern_learning) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let (analyzer, _temp, _notifier) = create_test_analyzer().await;
            let permissions = consent(app_usage, pattern_learning);

            analyzer
                .on_app_switch_with_context("Visual Studio Code", "main.rs", None, &permissions)
                .await;
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
            analyzer
                .on_app_switch_with_context("Slack", "general", None, &permissions)
                .await;

            let intelligence = analyzer.workflow_intelligence.read().await;
            assert_eq!(
                intelligence.usage_len() > 0,
                app_usage,
                "usage map must follow only app_usage_analytics ({app_usage}, {pattern_learning})"
            );
            assert_eq!(
                intelligence.has_active_segment(),
                pattern_learning,
                "workflow state must follow only activity_pattern_learning ({app_usage}, {pattern_learning})"
            );
        }
    }

    /// #8574: narrowing or revoking pattern consent clears unfinished pattern
    /// state immediately without deleting independently consented usage data.
    #[tokio::test]
    async fn runtime_pattern_revoke_clears_pattern_state_but_preserves_usage() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;
        let granted = consent(true, true);

        analyzer
            .on_app_switch_with_context("Visual Studio Code", "main.rs", None, &granted)
            .await;
        {
            let intelligence = analyzer.workflow_intelligence.read().await;
            assert!(intelligence.usage_len() > 0);
            assert!(intelligence.has_active_segment());
        }

        let narrowed = consent(true, false);
        analyzer.reconcile_activity_pattern_consent(&narrowed).await;

        let intelligence = analyzer.workflow_intelligence.read().await;
        assert!(
            intelligence.usage_len() > 0,
            "app-usage aggregation must survive a pattern-only revoke"
        );
        assert!(!intelligence.has_active_segment());
        assert_eq!(intelligence.playbook_len(), 0);
    }

    /// #8574: periodic and idle flush paths must clear stale pattern state and
    /// emit no pattern output when the live effective permission is closed.
    #[tokio::test]
    async fn periodic_and_idle_flush_fail_closed_without_pattern_consent() {
        let closed = consent(true, false);
        for flush_path in ["periodic", "idle"] {
            let (analyzer, _temp, _notifier) = create_test_analyzer().await;
            let granted = consent(true, true);
            analyzer
                .on_app_switch_with_context("Visual Studio Code", "main.rs", None, &granted)
                .await;

            let produced = if flush_path == "periodic" {
                analyzer.analyze_periodic(&closed).await
            } else {
                analyzer.on_idle_resume(&closed).await
            };

            let intelligence = analyzer.workflow_intelligence.read().await;
            assert!(!intelligence.has_active_segment());
            assert_eq!(intelligence.playbook_len(), 0);
            assert!(
                produced.iter().all(|suggestion| !suggestion
                    .content
                    .starts_with("Recurring workflow pattern detected:")),
                "{flush_path} must not produce a playbook pattern suggestion"
            );
        }
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

    /// #8578: the exact tracked interruption is resumed once, removed from the
    /// pending query, and used directly to build RestoreContext copy.
    #[tokio::test]
    async fn communication_to_original_deep_work_resumes_once_and_restores_from_app() {
        let (analyzer, _temp, _notifier, storage) = create_test_analyzer_with_storage().await;

        assert!(test_switch(&analyzer, "Visual Studio Code")
            .await
            .is_empty());
        assert!(test_switch(&analyzer, "Slack").await.is_empty());

        let produced = test_switch(&analyzer, "Visual Studio Code").await;
        let restore: Vec<_> = produced
            .iter()
            .filter(|suggestion| suggestion.suggestion_type == SuggestionType::RestoreContext)
            .collect();
        assert_eq!(
            restore.len(),
            1,
            "one completed interruption yields one restore suggestion"
        );
        assert!(restore[0].content.contains("Visual Studio Code"));

        let pending = <SqliteStorage as FocusStorage>::get_pending_interruption(&storage)
            .await
            .unwrap();
        assert!(
            pending.is_none(),
            "resumed row must leave the pending query"
        );
        assert!(analyzer
            .tracker
            .read()
            .await
            .pending_interruption_id
            .is_none());
    }

    /// #8578: a newer unrelated pending row must not replace the tracker-owned
    /// exact ID or supply the suggestion copy.
    #[tokio::test]
    async fn restore_resume_does_not_mix_with_newer_pending_interruption() {
        let (analyzer, _temp, _notifier, storage) = create_test_analyzer_with_storage().await;

        test_switch(&analyzer, "Visual Studio Code").await;
        test_switch(&analyzer, "Slack").await;
        let tracked_id = analyzer
            .tracker
            .read()
            .await
            .pending_interruption_id
            .expect("deep-to-communication switch should be tracked");

        let unrelated = Interruption::new(
            0,
            "Terminal".to_string(),
            "Microsoft Teams".to_string(),
            None,
        );
        let unrelated_id =
            <SqliteStorage as FocusStorage>::record_interruption(&storage, &unrelated)
                .await
                .unwrap();
        assert!(unrelated_id > tracked_id);

        let produced = test_switch(&analyzer, "Visual Studio Code").await;
        let restore = produced
            .iter()
            .find(|suggestion| suggestion.suggestion_type == SuggestionType::RestoreContext)
            .expect("tracked interruption should produce RestoreContext");
        assert!(restore.content.contains("Visual Studio Code"));
        assert!(!restore.content.contains("Terminal"));

        let still_pending = <SqliteStorage as FocusStorage>::get_pending_interruption(&storage)
            .await
            .unwrap()
            .expect("unrelated row should remain pending");
        assert_eq!(still_pending.id, unrelated_id);
    }

    /// #8578: an unknown exact ID produces no suggestion and remains in the
    /// in-memory tracker so a storage mismatch is never hidden.
    #[tokio::test]
    async fn unknown_resume_id_preserves_tracker_and_emits_nothing() {
        let (analyzer, _temp, _notifier, storage) = create_test_analyzer_with_storage().await;

        test_switch(&analyzer, "Visual Studio Code").await;
        test_switch(&analyzer, "Slack").await;
        let recorded_id = analyzer
            .tracker
            .read()
            .await
            .pending_interruption_id
            .expect("interruption should be tracked");
        let unknown_id = recorded_id + 10_000;
        analyzer.tracker.write().await.pending_interruption_id = Some(unknown_id);

        let produced = test_switch(&analyzer, "Visual Studio Code").await;
        assert!(produced
            .iter()
            .all(|suggestion| suggestion.suggestion_type != SuggestionType::RestoreContext));
        assert_eq!(
            analyzer.tracker.read().await.pending_interruption_id,
            Some(unknown_id)
        );

        let pending = <SqliteStorage as FocusStorage>::get_pending_interruption(&storage)
            .await
            .unwrap()
            .expect("original row must remain pending");
        assert_eq!(pending.id, recorded_id);
    }

    /// #8578: expiry is evaluated from the resumed snapshot; no latest-pending
    /// lookup can replace it after the transaction.
    #[tokio::test]
    async fn restore_context_rejects_snapshot_older_than_thirty_minutes() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;
        let now = Utc::now();
        let mut interruption = Interruption::new(
            1,
            "Visual Studio Code".to_string(),
            "Slack".to_string(),
            None,
        );
        interruption.interrupted_at = now - Duration::minutes(31);
        interruption.resumed_at = Some(now);
        interruption.resumed_to_app = Some("Visual Studio Code".to_string());

        assert!(analyzer
            .maybe_suggest_restore_context(&interruption, now)
            .await
            .is_none());
    }

    /// #8578: a storage error cannot clear the exact pending tracker ID or
    /// create a restore suggestion from an uncommitted snapshot.
    #[tokio::test]
    async fn resume_storage_failure_preserves_tracker_and_emits_nothing() {
        let storage = Arc::new(ResumeFailingStorage);
        let notifier = Arc::new(MockNotifier::new());
        let analyzer = FocusAnalyzer::with_defaults(storage, notifier);
        {
            let mut tracker = analyzer.tracker.write().await;
            tracker.current_app = Some("Slack".to_string());
            tracker.current_category = Some(AppCategory::Communication);
            tracker.current_app_start = Some(Utc::now());
            tracker.pending_interruption_id = Some(42);
        }

        let produced = test_switch(&analyzer, "Visual Studio Code").await;
        assert!(produced
            .iter()
            .all(|suggestion| suggestion.suggestion_type != SuggestionType::RestoreContext));
        assert_eq!(
            analyzer.tracker.read().await.pending_interruption_id,
            Some(42)
        );
    }

    #[tokio::test]
    async fn focus_score_calculation() {
        let (analyzer, _temp, _notifier) = create_test_analyzer().await;

        let now = Utc::now();
        let metrics = FocusMetrics {
            period: maekon_core::types::TimeWindow::new(now, now + Duration::hours(8))
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

        let permissions = consent(true, true);
        analyzer.on_idle_resume(&permissions).await;

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
            period: maekon_core::types::TimeWindow::new(now, now + Duration::hours(8))
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
            period: maekon_core::types::TimeWindow::new(now, now + Duration::hours(8))
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
