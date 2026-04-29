//! Playbook listing handlers — coaching templates + automation presets.
//!
//! Both endpoints return static data (no storage needed):
//! - `CoachingTemplateRegistry::new()` loads from a compiled-in slice.
//! - `builtin_presets()` returns the hardcoded preset list.

use axum::Json;
use oneshim_api_contracts::playbooks::{
    CoachingTemplateDto, CoachingTemplateListDto, PresetSummaryDto, PresetSummaryListDto,
};
use oneshim_core::models::coaching_template::TEMPLATES;
use oneshim_core::models::intent::builtin_presets;

/// GET /api/playbooks/coaching — list all coaching templates.
pub async fn list_coaching_templates() -> Json<CoachingTemplateListDto> {
    let templates: Vec<CoachingTemplateDto> = TEMPLATES
        .iter()
        .map(|t| CoachingTemplateDto {
            profile: format!("{:?}", t.profile),
            trigger_type: t.trigger_type.to_string(),
            tone: format!("{:?}", t.tone),
            locale: t.locale.to_string(),
            text: t.text.to_string(),
        })
        .collect();

    let total = templates.len();
    Json(CoachingTemplateListDto { total, templates })
}

/// GET /api/playbooks/presets — list built-in automation presets.
pub async fn list_presets() -> Json<PresetSummaryListDto> {
    let presets: Vec<PresetSummaryDto> = builtin_presets()
        .into_iter()
        .map(|p| PresetSummaryDto {
            id: p.id,
            name: p.name,
            description: p.description,
            category: format!("{:?}", p.category),
            step_count: p.steps.len(),
            builtin: p.builtin,
        })
        .collect();

    let total = presets.len();
    Json(PresetSummaryListDto { total, presets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use oneshim_storage::sqlite::SqliteStorage;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    fn loopback_app() -> axum::Router {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        let state = AppState::with_core(storage, event_tx);
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    #[tokio::test]
    async fn list_coaching_templates_returns_ok() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks/coaching")
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
        let total = parsed["total"].as_u64().unwrap();
        let templates = parsed["templates"].as_array().unwrap();
        assert!(
            total >= 50,
            "expected >= 50 coaching templates, got {total}"
        );
        assert_eq!(total as usize, templates.len());

        // Verify each entry has the expected fields
        let first = &templates[0];
        assert!(first["profile"].is_string());
        assert!(first["trigger_type"].is_string());
        assert!(first["tone"].is_string());
        assert!(first["locale"].is_string());
        assert!(first["text"].is_string());
    }

    #[tokio::test]
    async fn list_presets_returns_ok() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/playbooks/presets")
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
        let total = parsed["total"].as_u64().unwrap();
        let presets = parsed["presets"].as_array().unwrap();
        assert_eq!(total, 15, "expected 15 builtin presets, got {total}");
        assert_eq!(total as usize, presets.len());

        // Verify each entry has the expected fields
        let first = &presets[0];
        assert!(first["id"].is_string());
        assert!(first["name"].is_string());
        assert!(first["description"].is_string());
        assert!(first["category"].is_string());
        assert!(first["step_count"].is_number());
        assert!(first["builtin"].is_boolean());
        assert!(first["builtin"].as_bool().unwrap());
    }

    #[test]
    fn coaching_template_dto_serializes() {
        let dto = CoachingTemplateDto {
            profile: "FocusGuard".to_string(),
            trigger_type: "RegimeTransition".to_string(),
            tone: "Direct".to_string(),
            locale: "en".to_string(),
            text: "Focus alert: you switched {context_switches} times.".to_string(),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("FocusGuard"));
        assert!(json.contains("RegimeTransition"));
    }

    #[test]
    fn preset_summary_dto_serializes() {
        let dto = PresetSummaryDto {
            id: "save-file".to_string(),
            name: "file save".to_string(),
            description: "Save the current file".to_string(),
            category: "Productivity".to_string(),
            step_count: 1,
            builtin: true,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("save-file"));
        assert!(json.contains("Productivity"));
    }
}
