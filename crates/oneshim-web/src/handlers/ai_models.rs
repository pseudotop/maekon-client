use axum::{extract::State, Json};
use oneshim_api_contracts::ai_providers::{ProviderModelsRequest, ProviderModelsResponse};

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
    use axum::Json;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    fn test_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn test_context() -> AiModelCatalogWebContext {
        AiModelCatalogWebContext::from_state(&test_state())
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

    // iter-83 regression guards for iter-60 ApiError-form HTTP status mapping
    // in ai_model_catalog_web_service::discover_provider_models. The service
    // ultimately calls the user-provided endpoint for model discovery, so a
    // mockito server pointed at an OpenAI-style base URL suffices.
    async fn run_model_catalog_status_test(status: u16) -> ApiError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;

        let request = ProviderModelsRequest {
            provider_type: "openai".to_string(),
            api_key: "sk-test".to_string(),
            endpoint: Some(server.url()),
            surface: Some("llm_api".to_string()),
            surface_id: Some("provider_surface.openai.direct_api".to_string()),
            use_saved_secret: false,
        };

        discover_provider_models(State(test_context()), Json(request))
            .await
            .expect_err("expected error response from mock server")
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
