//! Unified gRPC + REST client — Consumer Contract (oneshim.client.v1).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use maekon_core::error::CoreError;
use maekon_core::models::suggestion::SuggestionFeedback as RestSuggestionFeedback;
use maekon_core::ports::api_client::ApiClient; // ApiClient trait for HttpApiClient methods
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

use super::auth_client::GrpcAuthClient;
use super::config::GrpcConfig;
use super::context_client::GrpcContextClient;
use super::session_client::GrpcSessionClient;
use crate::auth::TokenManager;
use crate::http_client::HttpApiClient;

pub use crate::proto::client_v1::{
    FeedbackAction, SuggestionEvent, SuggestionType, UploadBatchRequest, UploadBatchResponse,
};
pub use tonic::Streaming;

#[derive(Debug, Clone)]
pub struct AuthResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionResponse {
    pub session_id: String,
    pub user_id: String,
    pub client_id: String,
    pub capabilities: Vec<String>,
}

pub struct UnifiedClient {
    config: GrpcConfig,

    /// Uses `tokio::sync::Mutex` to prevent a TOCTOU race in the `ensure_*` methods.
    grpc_auth: Mutex<Option<GrpcAuthClient>>,
    grpc_session: Mutex<Option<GrpcSessionClient>>,
    grpc_context: Mutex<Option<GrpcContextClient>>,

    token_manager: Arc<TokenManager>,
    http_client: HttpApiClient,
}

impl UnifiedClient {
    pub fn new(config: GrpcConfig, token_manager: Arc<TokenManager>) -> Result<Self, CoreError> {
        info!(
            use_grpc_auth = config.use_grpc_auth,
            use_grpc_context = config.use_grpc_context,
            rest_tls = config.rest_tls.is_some(),
            "UnifiedClient initialize"
        );

        let timeout = Duration::from_secs(config.request_timeout_secs);
        #[allow(deprecated)] // Non-TLS fallback when rest_tls config is absent
        let http_client = if let Some(ref tls) = config.rest_tls {
            HttpApiClient::new_with_tls(&config.rest_endpoint, token_manager.clone(), timeout, tls)?
        } else {
            HttpApiClient::new(&config.rest_endpoint, token_manager.clone(), timeout)?
        };

        Ok(Self {
            config,
            grpc_auth: Mutex::new(None),
            grpc_session: Mutex::new(None),
            grpc_context: Mutex::new(None),
            token_manager,
            http_client,
        })
    }

    /// Initialize the gRPC auth client — the Mutex prevents a TOCTOU race.
    async fn ensure_grpc_auth(&self) -> Result<(), CoreError> {
        let mut guard = self.grpc_auth.lock().await;
        if guard.is_none() {
            *guard = Some(GrpcAuthClient::connect(self.config.clone()).await?);
        }
        Ok(())
    }

    /// Initialize the gRPC session client — the Mutex prevents a TOCTOU race.
    async fn ensure_grpc_session(&self) -> Result<(), CoreError> {
        let mut guard = self.grpc_session.lock().await;
        if guard.is_none() {
            *guard = Some(GrpcSessionClient::connect(self.config.clone()).await?);
        }
        Ok(())
    }

    /// Initialize the gRPC context client — the Mutex prevents a TOCTOU race.
    async fn ensure_grpc_context(&self) -> Result<(), CoreError> {
        let mut guard = self.grpc_context.lock().await;
        if guard.is_none() {
            *guard = Some(GrpcContextClient::connect(self.config.clone()).await?);
        }
        Ok(())
    }

    async fn with_grpc_context_client<R, F>(&self, op: &str, f: F) -> Result<R, CoreError>
    where
        F: for<'a> FnOnce(
            &'a mut GrpcContextClient,
        )
            -> Pin<Box<dyn Future<Output = Result<R, CoreError>> + Send + 'a>>,
    {
        self.ensure_grpc_context().await?;
        // #6442 F11: clone the client out of the lock and drop the guard before f()
        // awaits its RPC. Holding the context Mutex across the await serializes every
        // context call and defeats the tonic channel's HTTP/2 multiplexing.
        let mut client = {
            let guard = self.grpc_context.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: format!("gRPC context client initialize failure ({op})"),
                })?
                .clone()
        };
        f(&mut client).await
    }

    /// Authenticate via gRPC GetToken or REST login.
    pub async fn login(
        &self,
        identifier: &str,
        password: &str,
        organization_id: &str,
    ) -> Result<AuthResponse, CoreError> {
        if self.config.should_use_grpc_for_auth() {
            debug!("gRPC login attempt");
            self.login_grpc(identifier, password, organization_id).await
        } else {
            debug!("REST login attempt");
            self.login_rest(identifier, password, organization_id).await
        }
    }

    async fn login_grpc(
        &self,
        identifier: &str,
        credential: &str,
        organization_id: &str,
    ) -> Result<AuthResponse, CoreError> {
        self.ensure_grpc_auth().await?;

        // #6442 F11: clone out of the lock before the RPC await (see the helper note).
        let mut client = {
            let guard = self.grpc_auth.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "Failed to initialize gRPC auth client".to_string(),
                })?
                .clone()
        };

        let response = client
            .get_token(identifier, credential, organization_id)
            .await?;

        Ok(AuthResponse {
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_in: response.expires_in_secs,
            user_id: if response.user_id.is_empty() {
                None
            } else {
                Some(response.user_id)
            },
        })
    }

    async fn login_rest(
        &self,
        identifier: &str,
        password: &str,
        organization_id: &str,
    ) -> Result<AuthResponse, CoreError> {
        self.token_manager
            .login_with_org(identifier, password, organization_id)
            .await?;

        let access_token = self.token_manager.get_token().await?;

        Ok(AuthResponse {
            access_token,
            refresh_token: String::new(), // refresh token is not exposed in REST mode
            expires_in: 3600,
            user_id: None,
        })
    }

    pub async fn refresh_token(&self) -> Result<AuthResponse, CoreError> {
        self.refresh_token_rest().await
    }

    async fn refresh_token_rest(&self) -> Result<AuthResponse, CoreError> {
        self.token_manager.refresh().await?;

        let access_token = self.token_manager.get_token().await?;

        Ok(AuthResponse {
            access_token,
            refresh_token: String::new(),
            expires_in: 3600,
            user_id: None,
        })
    }

    pub async fn create_session(
        &self,
        client_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<SessionResponse, CoreError> {
        if self.config.should_use_grpc_for_context() {
            self.create_session_grpc(client_id, metadata).await
        } else {
            Ok(SessionResponse {
                session_id: String::new(),
                user_id: String::new(),
                client_id: client_id.to_string(),
                capabilities: vec![],
            })
        }
    }

    async fn create_session_grpc(
        &self,
        client_id: &str,
        metadata: HashMap<String, String>,
    ) -> Result<SessionResponse, CoreError> {
        self.ensure_grpc_session().await?;

        // #6442 F11: clone out of the lock before the RPC await.
        let mut client = {
            let guard = self.grpc_session.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "gRPC session client initialize failure".to_string(),
                })?
                .clone()
        };

        let response = client.create_session(client_id, metadata).await?;

        Ok(SessionResponse {
            session_id: response.session_id,
            user_id: response.user_id,
            client_id: response.client_id,
            capabilities: response.capabilities,
        })
    }

    pub async fn heartbeat(&self, session_id: &str) -> Result<bool, CoreError> {
        if self.config.should_use_grpc_for_context() {
            self.heartbeat_grpc(session_id).await
        } else {
            self.http_client.send_heartbeat(session_id).await?;
            Ok(true)
        }
    }

    async fn heartbeat_grpc(&self, session_id: &str) -> Result<bool, CoreError> {
        // F-RC-C22-04: heartbeat is an authenticated RPC — inject the Bearer token
        // like subscribe does (the server's AuthenticatedServiceWrapper requires it).
        let access_token = self.token_manager.get_token().await?;
        self.ensure_grpc_session().await?;

        // #6442 F11: clone out of the lock before the RPC await.
        let mut client = {
            let guard = self.grpc_session.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "gRPC session client initialize failure".to_string(),
                })?
                .clone()
        };

        // Heartbeat now returns Empty — success means the server acknowledged.
        client.heartbeat(session_id, &access_token).await?;
        Ok(true)
    }

    /// Subscribe to server-streamed suggestions.
    ///
    /// F-RC-C22-04: reads the access token from the TokenManager and injects an
    /// `authorization: Bearer <token>` header into the tonic Request metadata. The
    /// server-side AuthenticatedServiceWrapper requires this header, so calling
    /// without injecting it is rejected as UNAUTHENTICATED.
    ///
    /// # Example
    /// ```ignore
    /// let mut stream = client.subscribe_suggestions("session-123").await?;
    /// while let Some(event) = stream.message().await? {
    ///     println!("suggestion: {}", event.content);
    /// }
    /// ```
    pub async fn subscribe_suggestions(
        &self,
        session_id: &str,
    ) -> Result<Streaming<SuggestionEvent>, CoreError> {
        if !self.config.should_use_grpc_for_context() {
            return Err(CoreError::Network { code: maekon_core::error_codes::NetworkCode::Generic, message: "Suggestion streaming is available only in gRPC mode. Set use_grpc_context=true."
                    .to_string() });
        }

        debug!(
            "gRPC suggestion stream subscribe started: session_id={}",
            session_id,
        );

        // F-RC-C22-04: fetch the access token — on expiry the TokenManager attempts
        // an automatic refresh.
        let access_token = self.token_manager.get_token().await?;

        self.ensure_grpc_context().await?;

        // #6442 F11: clone out of the lock before subscribing. A streaming RPC holds its
        // result for the stream's whole lifetime — holding the context Mutex across it
        // would block every other context call for as long as the stream stays open.
        let mut client = {
            let guard = self.grpc_context.lock().await;
            guard
                .as_ref()
                .ok_or_else(|| CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    message: "gRPC context client initialize failure".to_string(),
                })?
                .clone()
        };

        let stream = client
            .subscribe_suggestions_with_token(session_id, &access_token)
            .await?;
        info!("gRPC suggestion stream subscribe success");

        Ok(stream)
    }

    /// Upload a batch of events and frame metadata.
    ///
    /// # Example
    /// ```ignore
    /// let request = UploadBatchRequest {
    ///     session_id: "session-456".to_string(),
    ///     events: vec![...],
    ///     frames: vec![...],
    /// };
    /// let response = client.upload_batch(request).await?;
    /// ```
    pub async fn upload_batch(
        &self,
        request: UploadBatchRequest,
    ) -> Result<UploadBatchResponse, CoreError> {
        if self.config.should_use_grpc_for_context() {
            debug!(
                "gRPC batch upload started: session_id={}, events={}, frames={}",
                request.session_id,
                request.events.len(),
                request.frames.len()
            );
            // F-RC-C22-04: inject the Bearer token (server auth wrapper requires it).
            let access_token = self.token_manager.get_token().await?;
            let response = self
                .with_grpc_context_client("upload_batch", |client| {
                    Box::pin(async move { client.upload_batch(request, &access_token).await })
                })
                .await?;
            info!(
                "gRPC batch upload completed: accepted_count={}",
                response.accepted_count
            );

            Ok(response)
        } else {
            // REST fallback cannot convert proto ClientEvent → REST Event model.
            // Log the skipped data and return 0 accepted so callers know nothing was sent.
            let skipped_events = request.events.len();
            let skipped_frames = request.frames.len();
            if skipped_events > 0 || skipped_frames > 0 {
                warn!(
                    "REST fallback: batch upload skipped — proto→REST event conversion unsupported. \
                     Skipped {} event(s), {} frame(s) for session {}",
                    skipped_events, skipped_frames, request.session_id
                );
            }

            Ok(UploadBatchResponse { accepted_count: 0 })
        }
    }

    /// Send feedback on a suggestion.
    ///
    /// # Example
    /// ```ignore
    /// client.send_feedback(
    ///     "suggestion-123",
    ///     FeedbackAction::Accepted,
    ///     None,
    /// ).await?;
    /// ```
    pub async fn send_feedback(
        &self,
        suggestion_id: &str,
        action: FeedbackAction,
        comment: Option<&str>,
    ) -> Result<(), CoreError> {
        if self.config.should_use_grpc_for_context() {
            debug!(
                "gRPC feedback sent: suggestion_id={}, action={:?}",
                suggestion_id, action
            );
            // F-RC-C22-04: inject the Bearer token (server auth wrapper requires it).
            let access_token = self.token_manager.get_token().await?;
            let suggestion_id_owned = suggestion_id.to_string();
            let comment_owned = comment.map(String::from);
            self.with_grpc_context_client("send_feedback", |client| {
                let suggestion_id = suggestion_id_owned;
                let comment = comment_owned;
                let token = access_token;
                Box::pin(async move {
                    client
                        .send_feedback(&suggestion_id, action, comment.as_deref(), &token)
                        .await
                })
            })
            .await?;
            info!(
                "gRPC feedback sent completed: suggestion_id={}",
                suggestion_id
            );

            Ok(())
        } else {
            debug!(
                "REST feedback sent: suggestion_id={}, action={:?}",
                suggestion_id, action
            );

            let rest_feedback_type = match action {
                FeedbackAction::Accepted => maekon_core::models::suggestion::FeedbackType::Accepted,
                FeedbackAction::Rejected => maekon_core::models::suggestion::FeedbackType::Rejected,
                FeedbackAction::Deferred => maekon_core::models::suggestion::FeedbackType::Deferred,
                _ => maekon_core::models::suggestion::FeedbackType::Rejected, // unknown -> rejected
            };

            let feedback = RestSuggestionFeedback {
                suggestion_id: suggestion_id.to_string(),
                feedback_type: rest_feedback_type,
                comment: comment.map(String::from),
                timestamp: chrono::Utc::now(),
                // #7600: this narrow (suggestion_id, action, comment) API predates
                // regime_id and has already lost it by the time it reaches this REST
                // fallback branch — GrpcApiAdapter::send_feedback narrows the full
                // SuggestionFeedback down to these 3 args before calling in. Carrying
                // regime_id through the gRPC/REST client surface is a separate,
                // out-of-scope follow-up from the local per-regime learning loop.
                regime_id: None,
            };

            self.http_client.send_feedback(&feedback).await?;
            info!(
                "REST feedback sent completed: suggestion_id={}",
                suggestion_id
            );

            Ok(())
        }
    }

    pub fn config(&self) -> &GrpcConfig {
        &self.config
    }

    pub fn is_using_grpc(&self) -> bool {
        self.config.use_grpc_auth || self.config.use_grpc_context
    }

    pub fn token_manager(&self) -> &Arc<TokenManager> {
        &self.token_manager
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn primary_password() -> String {
        String::from_utf8(vec![b'x'; 16]).expect("password fixture bytes must be UTF-8")
    }

    // Test fixture talks to a mockito server, not a real TLS endpoint — the
    // legacy non-TLS constructor is the documented/intended choice here
    // (see TokenManager::new doc comment), matching the same-pattern
    // #[allow(deprecated)] used by sibling test fixtures in http_client.rs
    // / sse_client.rs / auth/tests.rs.
    #[allow(deprecated)]
    async fn authed_unified_client(
        server: &mut mockito::ServerGuard,
    ) -> (UnifiedClient, mockito::Mock) {
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test_jwt","refresh_token":"ref","expires_in":3600}"#)
            .create_async()
            .await;
        let token_manager = Arc::new(TokenManager::new(&server.url()));
        token_manager
            .login("test@test.com", &primary_password())
            .await
            .unwrap();
        let config = GrpcConfig {
            rest_endpoint: server.url(),
            use_grpc_context: false,
            ..GrpcConfig::default()
        };
        let client = UnifiedClient::new(config, token_manager).unwrap();
        (client, login_mock)
    }

    #[test]
    fn test_auth_response() {
        let response = AuthResponse {
            access_token: "token".to_string(),
            refresh_token: "refresh".to_string(),
            expires_in: 3600,
            user_id: Some("user-123".to_string()),
        };
        assert_eq!(response.access_token, "token");
        assert_eq!(response.user_id, Some("user-123".to_string()));
    }

    #[test]
    fn test_session_response() {
        let response = SessionResponse {
            session_id: "session-123".to_string(),
            user_id: "user-456".to_string(),
            client_id: "client-789".to_string(),
            capabilities: vec!["upload".to_string()],
        };
        assert_eq!(response.session_id, "session-123");
        assert_eq!(response.client_id, "client-789");
    }

    #[tokio::test]
    async fn heartbeat_rest_mode_posts_to_rest_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let (client, login_mock) = authed_unified_client(&mut server).await;
        let heartbeat_mock = server
            .mock("POST", "/user_context/sessions/sess_rest/heartbeat")
            .with_status(200)
            .create_async()
            .await;

        let alive = client.heartbeat("sess_rest").await.unwrap();

        assert!(alive);
        login_mock.assert_async().await;
        heartbeat_mock.assert_async().await;
    }
}
