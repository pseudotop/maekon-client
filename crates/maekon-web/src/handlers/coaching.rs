use axum::extract::{Query, State};
use axum::Json;
use maekon_api_contracts::coaching::{
    CoachingEventResponse, CoachingHistoryQuery, CoachingStatsTodayResponse, GoalProgressResponse,
    HabitStreakQuery, HabitStreakResponse, UpdateGoalsRequest,
};
use tracing::warn;

use crate::error::ApiError;
use crate::AppState;

/// GET /api/coaching/history
pub async fn get_coaching_history(
    State(state): State<AppState>,
    Query(params): Query<CoachingHistoryQuery>,
) -> Result<Json<Vec<CoachingEventResponse>>, ApiError> {
    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    // ADR-026 PR-8: `query_coaching_events` is now async and offloads the SQLite
    // read onto `spawn_blocking` internally, so await it directly instead of
    // wrapping the (now-async) call in a hand-rolled `spawn_blocking`.
    let events = state
        .core
        .storage
        .query_coaching_events(limit, offset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(
        events
            .into_iter()
            .map(CoachingEventResponse::from)
            .collect(),
    ))
}

/// GET /api/coaching/goals
pub async fn get_goals(
    State(state): State<AppState>,
) -> Result<Json<Vec<GoalProgressResponse>>, ApiError> {
    if let Some(ref engine) = state.analysis.coaching_engine {
        let progress = engine.all_goal_progress().await;
        Ok(Json(
            progress
                .into_iter()
                .map(GoalProgressResponse::from)
                .collect(),
        ))
    } else {
        Ok(Json(vec![]))
    }
}

/// PUT /api/coaching/goals
///
/// Updates the live coaching engine's regime goal targets AND persists them to
/// the durable config, which is the single source of truth for
/// `coaching.regime_goals` (#8083). Config is what `CoachingEngine::new`
/// re-hydrates on restart and what the settings-save path deliberately
/// preserves (regime_goals is display-only there), so this dedicated endpoint is
/// the sole writer — mirroring the Tauri `update_regime_goals` IPC command.
///
/// The durable config write happens FIRST (the only fallible step): a failure is
/// surfaced as an error instead of a false success that would silently revert on
/// the next restart. The engine (in-RAM, infallible) is updated only after the
/// config write succeeds, so the two never diverge. The config write is skipped
/// when no config manager is wired (standalone / tests), matching the sibling
/// tracking-schedule handler.
pub async fn update_goals(
    State(state): State<AppState>,
    Json(body): Json<UpdateGoalsRequest>,
) -> Result<Json<()>, ApiError> {
    if let Some(ref manager) = state.core.config_manager {
        manager
            .update_with(|config| {
                config.coaching.regime_goals = body.goals.clone();
                Ok(())
            })
            .map_err(ApiError::from)?;
    }
    if let Some(ref engine) = state.analysis.coaching_engine {
        engine.update_regime_goals(&body.goals).await;
    }
    Ok(Json(()))
}

/// GET /api/coaching/stats/today — aggregated coaching stats for the current day.
pub async fn get_coaching_stats_today(
    State(state): State<AppState>,
) -> Result<Json<CoachingStatsTodayResponse>, ApiError> {
    let today_str = chrono::Local::now().format("%Y-%m-%d").to_string();
    // ADR-026 PR-8: `query_coaching_events_since` is now async and offloads the
    // SQLite read onto `spawn_blocking` internally — await it directly.
    //
    // Best-effort: a storage failure (DB locked, schema mismatch, I/O error) must
    // not surface as an HTTP 500 for this stats endpoint — callers tolerate a stale
    // zero. Log at WARN with a structured err.code field (ADR-019) so operators can
    // detect persistent storage degradation without parsing the Display body.
    let today_count = match state
        .core
        .storage
        .query_coaching_events_since(&today_str)
        .await
    {
        Ok(events) => events.len() as u32,
        Err(e) => {
            warn!(
                err.code = %e.code(),
                "coaching stats: storage read failed, returning 0 as best-effort: {e}"
            );
            0
        }
    };

    // F-RR-C37-01: use the async variant instead of the `_blocking` variant.
    // A `block_in_place` + `block_on` chain occupies a worker thread inside an
    // async fn handler, which panics on Tokio's single-threaded runtime
    // (the `#[tokio::test]` default).
    let current_regime = if let Some(ref engine) = state.analysis.coaching_engine {
        engine.current_regime_label().await
    } else {
        None
    };

    let regime_minutes = if let Some(ref engine) = state.analysis.coaching_engine {
        engine.regime_minutes_today().await
    } else {
        0
    };

    Ok(Json(CoachingStatsTodayResponse {
        nudges_count: today_count,
        current_regime,
        regime_minutes_today: regime_minutes,
    }))
}

/// GET /api/coaching/habits?days=7 -- habit streak data for the last N days.
pub async fn get_habits(
    State(state): State<AppState>,
    Query(params): Query<HabitStreakQuery>,
) -> Result<Json<Vec<HabitStreakResponse>>, ApiError> {
    let days = params.days.unwrap_or(7);
    // ADR-026 PR-8: `query_habit_streaks` is now async and offloads the SQLite
    // read onto `spawn_blocking` internally — await it directly.
    let rows = state
        .core
        .storage
        .query_habit_streaks(days)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(
        rows.into_iter().map(HabitStreakResponse::from).collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    use maekon_core::models::coaching::GoalProgressView;
    use maekon_core::ports::coaching::CoachingPort;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tower::ServiceExt;

    use crate::test_local_auth::{authed_loopback_router as loopback_app, test_app_state};

    struct MockCoachingEngine;

    #[async_trait]
    impl CoachingPort for MockCoachingEngine {
        fn all_goal_progress_blocking(&self) -> Vec<GoalProgressView> {
            vec![GoalProgressView {
                regime_label: "deep_work".to_string(),
                current_minutes: 90,
                target_minutes: 120,
                percentage: 75,
                display_color: "#4CAF50".to_string(),
            }]
        }

        fn update_regime_goals_blocking(&self, _goals: &HashMap<String, u32>) {
            // no-op for test
        }

        async fn snooze_profile(&self, _profile: &str, _duration_secs: u64) {}
        async fn record_feedback(&self, _message_id: &str, _positive: bool) {}
        async fn all_goal_progress(&self) -> Vec<GoalProgressView> {
            self.all_goal_progress_blocking()
        }
        async fn update_regime_goals(&self, _goals: &HashMap<String, u32>) {}

        fn current_regime_label_blocking(&self) -> Option<String> {
            Some("deep_work".to_string())
        }
        fn regime_minutes_today_blocking(&self) -> u32 {
            90
        }
        // F-RR-C37-01: async variant — MockCoachingEngine implements it directly
        // instead of delegating to the `_blocking` default.
        async fn current_regime_label(&self) -> Option<String> {
            Some("deep_work".to_string())
        }
        async fn regime_minutes_today(&self) -> u32 {
            90
        }
    }

    fn test_app_state_with_coaching() -> AppState {
        let mut state = test_app_state();
        state.analysis.coaching_engine = Some(Arc::new(MockCoachingEngine));
        state
    }

    #[tokio::test]
    async fn get_coaching_history_returns_events() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/history")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Empty database returns an empty JSON array
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_goals_returns_empty_without_engine() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/goals")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_goals_returns_progress_with_engine() {
        let app = loopback_app(test_app_state_with_coaching());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/goals")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let goals = parsed.as_array().unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0]["regime_label"], "deep_work");
        assert_eq!(goals[0]["current_minutes"], 90);
        assert_eq!(goals[0]["target_minutes"], 120);
        assert_eq!(goals[0]["percentage"], 75);
    }

    #[tokio::test]
    async fn update_goals_succeeds() {
        let app = loopback_app(test_app_state_with_coaching());
        let body = r#"{"goals":{"deep_work":180}}"#.to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/coaching/goals")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn update_goals_succeeds_without_engine() {
        let app = loopback_app(test_app_state());
        let body = r#"{"goals":{"deep_work":180}}"#.to_string();

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/coaching/goals")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    /// #8083: the goals endpoint is the durable single source of truth for
    /// `coaching.regime_goals` — it must persist to config (not just the in-RAM
    /// engine), so the goal survives restart and the settings-save path can
    /// safely preserve it. Calls the handler directly to assert the config
    /// side-effect (the router path is covered by the tests above).
    #[tokio::test]
    async fn update_goals_persists_to_config_as_durable_ssot() {
        use maekon_core::config_manager::ConfigManager;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let config_manager = ConfigManager::with_path(dir.path().join("config.json")).unwrap();
        let mut state = test_app_state_with_coaching();
        state.core.config_manager = Some(config_manager.clone());

        let mut goals = HashMap::new();
        goals.insert("deep_work".to_string(), 180u32);

        let Json(()) = super::update_goals(State(state), Json(UpdateGoalsRequest { goals }))
            .await
            .expect("update_goals should succeed and persist to config");

        assert_eq!(
            config_manager
                .get()
                .coaching
                .regime_goals
                .get("deep_work")
                .copied(),
            Some(180),
            "the goals endpoint must persist to config as the durable SSOT",
        );
    }

    #[test]
    fn coaching_event_response_serializes() {
        let resp = CoachingEventResponse {
            event_id: "evt-001".to_string(),
            trigger_type: "regime_change".to_string(),
            profile_name: "focus_boost".to_string(),
            regime_id: Some("regime-abc".to_string()),
            message_template: "Time to focus!".to_string(),
            personalized_message: Some("You've been in meetings for 2h.".to_string()),
            shown_at: "2026-03-21T10:00:00Z".to_string(),
            dismissed_at: None,
            dismiss_action: None,
            feedback_type: None,
            feedback_score: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("focus_boost"));
        assert!(json.contains("regime_change"));
    }

    #[test]
    fn goal_progress_response_serializes() {
        let resp = GoalProgressResponse {
            regime_label: "deep_work".to_string(),
            current_minutes: 90,
            target_minutes: 120,
            percentage: 75,
            display_color: "#4CAF50".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("deep_work"));
        assert!(json.contains("\"percentage\":75"));
    }

    #[tokio::test]
    async fn get_coaching_stats_today_returns_defaults() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/stats/today")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["nudges_count"], 0);
        assert!(parsed["current_regime"].is_null());
        assert_eq!(parsed["regime_minutes_today"], 0);
    }

    #[tokio::test]
    async fn get_coaching_stats_today_returns_regime_data_from_engine() {
        let app = loopback_app(test_app_state_with_coaching());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/stats/today")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["current_regime"], "deep_work");
        assert_eq!(parsed["regime_minutes_today"], 90);
    }

    /// F-RR-C37-01 regression guard: verifies the async port methods run without
    /// panicking on a single-threaded runtime (the `#[tokio::test]` default). While
    /// this test exists, a `block_in_place` regression is caught immediately by the
    /// `cannot block_in_place inside a current-thread runtime` panic.
    #[tokio::test]
    async fn coaching_stats_async_port_methods_do_not_block_in_place() {
        let engine = MockCoachingEngine;
        // The async variants must return values without panicking in a single-threaded context.
        let label = engine.current_regime_label().await;
        let minutes = engine.regime_minutes_today().await;
        assert_eq!(label, Some("deep_work".to_string()));
        assert_eq!(minutes, 90);
    }

    #[tokio::test]
    async fn get_habits_returns_empty_by_default() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/habits?days=7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn get_habits_returns_upserted_data() {
        let state = test_app_state();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        state
            .core
            .storage
            .upsert_habit_streak("deep_work", &today, 90, 120, false)
            .await
            .unwrap();

        let app = loopback_app(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/coaching/habits?days=7")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["regime_label"], "deep_work");
        assert_eq!(arr[0]["minutes_logged"], 90);
        assert_eq!(arr[0]["target_minutes"], 120);
        assert_eq!(arr[0]["met"], false);
    }
}
