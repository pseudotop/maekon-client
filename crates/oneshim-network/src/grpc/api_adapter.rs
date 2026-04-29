//! gRPC API adapter — bridges UnifiedClient to the ApiClient port trait.
//! REST-only operations (create_session, end_session, upload_batch, upload_context)
//! delegate directly to HttpApiClient. gRPC-capable operations (heartbeat, feedback)
//! delegate to UnifiedClient.

#[cfg(feature = "grpc")]
use std::sync::Arc;

#[cfg(feature = "grpc")]
use async_trait::async_trait;
#[cfg(feature = "grpc")]
use tracing::debug;

#[cfg(feature = "grpc")]
use oneshim_core::error::CoreError;
#[cfg(feature = "grpc")]
use oneshim_core::models::event::EventBatch;
#[cfg(feature = "grpc")]
use oneshim_core::models::frame::ContextUpload;
#[cfg(feature = "grpc")]
use oneshim_core::models::suggestion::{FeedbackType, SuggestionFeedback};
#[cfg(feature = "grpc")]
use oneshim_core::ports::api_client::{ApiClient, SessionCreateResponse};

#[cfg(feature = "grpc")]
use super::unified_client::{FeedbackAction, UnifiedClient};
#[cfg(feature = "grpc")]
use crate::http_client::HttpApiClient;

/// Adapter that implements [`ApiClient`] by routing requests to either
/// `UnifiedClient` (gRPC) or `HttpApiClient` (REST) depending on the operation.
#[cfg(feature = "grpc")]
pub struct GrpcApiAdapter {
    unified: Arc<UnifiedClient>,
    http_fallback: HttpApiClient,
}

#[cfg(feature = "grpc")]
impl GrpcApiAdapter {
    pub fn new(unified: Arc<UnifiedClient>, http_fallback: HttpApiClient) -> Self {
        Self {
            unified,
            http_fallback,
        }
    }
}

#[cfg(feature = "grpc")]
#[async_trait]
impl ApiClient for GrpcApiAdapter {
    async fn create_session(&self, client_id: &str) -> Result<SessionCreateResponse, CoreError> {
        debug!(client_id, "GrpcApiAdapter: create_session via REST");
        self.http_fallback.create_session(client_id).await
    }

    async fn end_session(&self, session_id: &str) -> Result<(), CoreError> {
        debug!(session_id, "GrpcApiAdapter: end_session via REST");
        self.http_fallback.end_session(session_id).await
    }

    async fn upload_batch(&self, batch: &EventBatch) -> Result<(), CoreError> {
        debug!("GrpcApiAdapter: upload_batch via REST");
        self.http_fallback.upload_batch(batch).await
    }

    async fn upload_context(&self, upload: &ContextUpload) -> Result<(), CoreError> {
        debug!("GrpcApiAdapter: upload_context via REST");
        self.http_fallback.upload_context(upload).await
    }

    async fn send_feedback(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        debug!(
            suggestion_id = %feedback.suggestion_id,
            "GrpcApiAdapter: send_feedback via UnifiedClient"
        );
        let action = match feedback.feedback_type {
            FeedbackType::Accepted => FeedbackAction::Accepted,
            FeedbackType::Rejected => FeedbackAction::Rejected,
            FeedbackType::Deferred => FeedbackAction::Deferred,
        };
        self.unified
            .send_feedback(&feedback.suggestion_id, action, feedback.comment.as_deref())
            .await
    }

    async fn send_heartbeat(&self, session_id: &str) -> Result<(), CoreError> {
        debug!(
            session_id,
            "GrpcApiAdapter: send_heartbeat via UnifiedClient"
        );
        self.unified.heartbeat(session_id).await?;
        Ok(())
    }
}

#[cfg(all(test, feature = "grpc"))]
mod tests {
    use super::*;

    #[test]
    fn adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GrpcApiAdapter>();
    }
}
