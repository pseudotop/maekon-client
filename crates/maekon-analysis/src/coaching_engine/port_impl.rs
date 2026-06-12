use std::collections::HashMap;
use std::time::Duration;

use maekon_core::models::coaching::GoalProgressView;

use super::CoachingEngine;

#[async_trait::async_trait]
impl maekon_core::ports::coaching::CoachingPort for CoachingEngine {
    fn all_goal_progress_blocking(&self) -> Vec<GoalProgressView> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.all_goal_progress())
        })
    }

    fn update_regime_goals_blocking(&self, goals: &HashMap<String, u32>) {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.update_regime_goals(goals))
        })
    }

    async fn snooze_profile(&self, profile: &str, duration_secs: u64) {
        self.snooze_current_profile(profile, Duration::from_secs(duration_secs))
            .await;
    }

    async fn record_feedback(&self, message_id: &str, positive: bool) {
        self.record_explicit_feedback(message_id, positive).await;
    }

    async fn all_goal_progress(&self) -> Vec<GoalProgressView> {
        self.all_goal_progress().await
    }

    async fn update_regime_goals(&self, goals: &HashMap<String, u32>) {
        self.update_regime_goals(goals).await;
    }

    fn current_regime_label_blocking(&self) -> Option<String> {
        let handle = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            handle.block_on(async { self.current_regime_label.read().await.clone() })
        })
    }

    fn regime_minutes_today_blocking(&self) -> u32 {
        let handle = tokio::runtime::Handle::current();
        tokio::task::block_in_place(|| {
            handle.block_on(async {
                let gt = self.goal_tracker.read().await;
                gt.all_progress().iter().map(|p| p.current_minutes).sum()
            })
        })
    }

    /// 현재 체제 레이블 비동기 반환.
    /// `_blocking` 변형과 달리 `block_in_place` 없이 RwLock을 직접 await한다 (F-RR-C37-01).
    async fn current_regime_label(&self) -> Option<String> {
        self.current_regime_label.read().await.clone()
    }

    /// 오늘 체제별 총 분(minutes) 합계 비동기 반환.
    /// `_blocking` 변형과 달리 `block_in_place` 없이 RwLock을 직접 await한다 (F-RR-C37-01).
    async fn regime_minutes_today(&self) -> u32 {
        let gt = self.goal_tracker.read().await;
        gt.all_progress().iter().map(|p| p.current_minutes).sum()
    }

    /// Hot-reload coaching config: delegates to `CoachingEngine::update_config` (#5707).
    ///
    /// Lock ordering: config → goal_tracker (identical to `update_config`), so
    /// no additional deadlock risk is introduced.
    async fn apply_config(&self, config: maekon_core::config::CoachingConfig) {
        self.update_config(config).await;
    }
}
