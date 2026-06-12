use axum::{extract::State, Json};
use maekon_api_contracts::ai_providers::{ProviderModelsRequest, ProviderModelsResponse};

use crate::error::ApiError;
use crate::services::ai_model_catalog_web_service::AiModelCatalogQueryService;
use crate::services::web_contexts::AiModelCatalogWebContext;

pub async fn discover_provider_models(
    State(context): State<AiModelCatalogWebContext>,
    Json(request): Json<ProviderModelsRequest>,
) -> Result<Json<ProviderModelsResponse>, ApiError> {
    let response = AiModelCatalogQueryService::new(context)
        .discover_provider_models(&request)
        .await?;
    Ok(Json(response))
}

pub async fn discover_provider_models_for_integration(
    State(context): State<AiModelCatalogWebContext>,
    Json(request): Json<ProviderModelsRequest>,
) -> Result<Json<ProviderModelsResponse>, ApiError> {
    let response = AiModelCatalogQueryService::new(context)
        .discover_provider_models_for_integration(&request)
        .await?;
    Ok(Json(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use async_trait::async_trait;
    use axum::Json;
    use maekon_core::ports::provider_model_catalog::{
        ProviderModelCatalogError, ProviderModelCatalogPort, ProviderModelCatalogRequest,
        ProviderModelCatalogResponse,
    };
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    #[derive(Clone)]
    struct FixedStatusModelCatalogPort {
        status: u16,
        body: String,
    }

    #[async_trait]
    impl ProviderModelCatalogPort for FixedStatusModelCatalogPort {
        async fn fetch_models(
            &self,
            _request: ProviderModelCatalogRequest,
        ) -> Result<ProviderModelCatalogResponse, ProviderModelCatalogError> {
            Ok(ProviderModelCatalogResponse {
                status: self.status,
                body: self.body.clone(),
            })
        }
    }

    fn test_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn test_context() -> AiModelCatalogWebContext {
        AiModelCatalogWebContext::from_state(&test_state())
    }

    fn test_context_with_catalog_status(status: u16) -> AiModelCatalogWebContext {
        let mut state = test_state();
        state.analysis.model_catalog_client = Some(Arc::new(FixedStatusModelCatalogPort {
            status,
            body: format!("http {status}"),
        }));
        AiModelCatalogWebContext::from_state(&state)
    }

    #[tokio::test]
    async fn integration_model_discovery_rejects_saved_secret() {
        let request = ProviderModelsRequest {
            provider_type: "openai".to_string(),
            api_key: String::new(),
            endpoint: None,
            surface: Some("llm_api".to_string()),
            surface_id: Some("provider_surface.openai.direct_api".to_string()),
            use_saved_secret: true,
        };

        let err = discover_provider_models_for_integration(State(test_context()), Json(request))
            .await
            .expect_err("integration discovery should reject saved secrets");

        match err {
            ApiError::BadRequest(message) => {
                assert!(message.contains("caller-supplied credentials"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    async fn run_model_catalog_status_test(status: u16) -> ApiError {
        let request = ProviderModelsRequest {
            provider_type: "openai".to_string(),
            api_key: "sk-test".to_string(),
            endpoint: Some("https://example.test/v1/models".to_string()),
            surface: Some("llm_api".to_string()),
            surface_id: Some("provider_surface.openai.direct_api".to_string()),
            use_saved_secret: false,
        };

        discover_provider_models(
            State(test_context_with_catalog_status(status)),
            Json(request),
        )
        .await
        .expect_err("expected error response from model catalog port")
    }

    #[tokio::test]
    async fn catalog_401_maps_to_unauthorized() {
        let err = run_model_catalog_status_test(401).await;
        assert!(
            matches!(err, ApiError::Unauthorized(_)),
            "401 → Unauthorized, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn catalog_403_maps_to_forbidden() {
        let err = run_model_catalog_status_test(403).await;
        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "403 → Forbidden, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn catalog_404_maps_to_not_found() {
        let err = run_model_catalog_status_test(404).await;
        assert!(
            matches!(err, ApiError::NotFound(_)),
            "404 → NotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn catalog_429_maps_to_service_unavailable() {
        let err = run_model_catalog_status_test(429).await;
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "429 → ServiceUnavailable (ApiError has no TooManyRequests variant), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn catalog_503_maps_to_service_unavailable() {
        let err = run_model_catalog_status_test(503).await;
        assert!(
            matches!(err, ApiError::ServiceUnavailable(_)),
            "503 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn catalog_500_falls_back_to_internal() {
        let err = run_model_catalog_status_test(500).await;
        assert!(
            matches!(err, ApiError::Internal(_)),
            "500 should fall back to Internal, got: {err:?}"
        );
    }
}
