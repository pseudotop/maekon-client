use axum::extract::{Query, State};
use axum::Json;
use oneshim_api_contracts::coaching::{
    CoachingEventResponse, CoachingHistoryQuery, CoachingStatsTodayResponse, GoalProgressResponse,
    HabitStreakQuery, HabitStreakResponse, UpdateGoalsRequest,
};

use crate::error::ApiError;
use crate::AppState;

/// GET /api/coaching/history
pub async fn get_coaching_history(
    State(state): State<AppState>,
    Query(params): Query<CoachingHistoryQuery>,
) -> Result<Json<Vec<CoachingEventResponse>>, ApiError> {
    let events = state
        .core
        .storage
        .query_coaching_events(params.limit.unwrap_or(50), params.offset.unwrap_or(0))
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
        let progress = engine.all_goal_progress_blocking();
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
pub async fn update_goals(
    State(state): State<AppState>,
    Json(body): Json<UpdateGoalsRequest>,
) -> Result<Json<()>, ApiError> {
    if let Some(ref engine) = state.analysis.coaching_engine {
        engine.update_regime_goals_blocking(&body.goals);
    }
    Ok(Json(()))
}

/// GET /api/coaching/stats/today — aggregated coaching stats for the current day.
pub async fn get_coaching_stats_today(
    State(state): State<AppState>,
) -> Result<Json<CoachingStatsTodayResponse>, ApiError> {
    let today_str = chrono::Local::now().format("%Y-%m-%d").to_string();
    let today_count = state
        .core
        .storage
        .query_coaching_events_since(&today_str)
        .map(|events| events.len() as u32)
        .unwrap_or(0);

    let current_regime = if let Some(ref engine) = state.analysis.coaching_engine {
        engine.current_regime_label_blocking()
    } else {
        None
    };

    let regime_minutes = if let Some(ref engine) = state.analysis.coaching_engine {
        engine.regime_minutes_today_blocking()
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
    let rows = state
        .core
        .storage
        .query_habit_streaks(days)
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
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};

    use oneshim_core::models::coaching::GoalProgressView;
    use oneshim_core::ports::coaching::CoachingPort;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

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
    }

    fn test_app_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn test_app_state_with_coaching() -> AppState {
        let mut state = test_app_state();
        state.analysis.coaching_engine = Some(Arc::new(MockCoachingEngine));
        state
    }

    fn loopback_app(state: AppState) -> axum::Router {
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
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
