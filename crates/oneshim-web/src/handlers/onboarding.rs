use axum::{extract::State, Json};
use oneshim_api_contracts::onboarding::OnboardingQuickstartDto;

use crate::services::onboarding_service::OnboardingQueryService;
use crate::services::web_contexts::ConfigWebContext;

pub async fn get_quickstart(
    State(context): State<ConfigWebContext>,
) -> Json<OnboardingQuickstartDto> {
    Json(OnboardingQueryService::new(context).get_quickstart())
}

#[cfg(test)]
mod tests {
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use oneshim_api_contracts::onboarding::{OnboardingQuickstartDto, QuickstartStepDto};

    use oneshim_storage::sqlite::SqliteStorage;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    fn test_app_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn loopback_app() -> axum::Router {
        let state = test_app_state();
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    #[tokio::test]
    async fn get_quickstart_returns_200() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/onboarding/quickstart")
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
        assert!(parsed["schema_version"].is_string());
        assert!(parsed["dashboard_url"].is_string());
        assert!(parsed["checklist"].is_array());
        assert!(parsed["recommended_presets"].is_array());
        assert!(parsed["verification_commands"].is_array());
    }

    #[tokio::test]
    async fn get_quickstart_contains_checklist_steps() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/onboarding/quickstart")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let checklist = parsed["checklist"].as_array().unwrap();
        assert!(
            checklist.len() >= 3,
            "expected at least 3 checklist steps, got {}",
            checklist.len()
        );
        // Each step has order, title, action, expected_outcome
        let first = &checklist[0];
        assert!(first["order"].is_number());
        assert!(first["title"].is_string());
        assert!(first["action"].is_string());
        assert!(first["expected_outcome"].is_string());
    }

    #[test]
    fn quickstart_response_serializes() {
        let response = OnboardingQuickstartDto {
            schema_version: "onboarding.quickstart.v1".to_string(),
            generated_at: "2026-03-21T00:00:00Z".to_string(),
            target_mode: "standalone".to_string(),
            dashboard_url: "http://127.0.0.1:10090".to_string(),
            checklist: vec![QuickstartStepDto {
                order: 1,
                title: "Run Standalone Mode".to_string(),
                action: "Launch the app".to_string(),
                expected_outcome: "Agent starts".to_string(),
            }],
            recommended_presets: vec![],
            verification_commands: vec!["cargo test".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("onboarding.quickstart.v1"));
        assert!(json.contains("Run Standalone Mode"));
        assert!(json.contains("standalone"));
    }
}
