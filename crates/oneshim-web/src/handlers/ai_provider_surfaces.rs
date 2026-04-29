use axum::Json;
use oneshim_api_contracts::provider_specs::ProviderSurfaceCatalog;

use crate::error::ApiError;
use crate::services::ai_provider_spec_web_service::AiProviderSpecQueryService;

pub async fn list_provider_surfaces() -> Result<Json<ProviderSurfaceCatalog>, ApiError> {
    let response = AiProviderSpecQueryService::new().list_provider_surfaces()?;
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};

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
    async fn list_provider_surfaces_returns_200() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ai/provider-surfaces")
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
        assert!(parsed["version"].is_number());
        assert!(parsed["vendors"].is_array());
        assert!(parsed["surfaces"].is_array());
    }

    #[tokio::test]
    async fn list_provider_surfaces_contains_known_vendors() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ai/provider-surfaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let surfaces = parsed["surfaces"].as_array().unwrap();
        assert!(
            surfaces.len() >= 6,
            "expected at least 6 provider surfaces, got {}",
            surfaces.len()
        );
        // Each surface has required fields
        let first = &surfaces[0];
        assert!(first["surface_id"].is_string());
        assert!(first["vendor_id"].is_string());
        assert!(first["provider_type"].is_string());
    }

    #[test]
    fn provider_surface_catalog_serializes_roundtrip() {
        use oneshim_api_contracts::provider_specs::ProviderSurfaceCatalog;

        let catalog =
            crate::services::ai_provider_spec_service::list_provider_surface_specs().unwrap();
        let json = serde_json::to_string(&catalog).unwrap();
        let deserialized: ProviderSurfaceCatalog = serde_json::from_str(&json).unwrap();
        assert_eq!(catalog.version, deserialized.version);
        assert_eq!(catalog.surfaces.len(), deserialized.surfaces.len());
        assert_eq!(catalog.vendors.len(), deserialized.vendors.len());
    }
}
