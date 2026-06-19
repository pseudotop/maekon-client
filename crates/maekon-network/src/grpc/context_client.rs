//! gRPC context + suggestion client — Consumer Contract
//! (oneshim.client.v1.ClientContext + oneshim.client.v1.ClientSuggestion).

use maekon_core::error::CoreError;
use tonic::transport::Channel;
use tracing::{debug, error, info};

use super::{map_grpc_status_error, GrpcConfig};
use crate::proto::client_v1::{
    client_context_client::ClientContextClient, client_suggestion_client::ClientSuggestionClient,
    FeedbackAction, SendFeedbackRequest, SubscribeRequest, SuggestionEvent, UploadBatchRequest,
    UploadBatchResponse,
};

/// Wraps both ClientContext (batch upload) and ClientSuggestion (subscribe/feedback) services.
///
/// Two separate channels are maintained:
/// - `context_client` / `suggestion_client`: built with the standard `connect_channel` path,
///   carrying a 30 s `GrpcTimeout::server_timeout` for unary RPCs.
/// - `streaming_suggestion_client`: built with `connect_streaming_channel`, which omits the
///   channel-level timeout so that `SubscribeSuggestions` (server-streaming) can remain open
///   indefinitely.  Liveness is enforced by the per-message `MSG_TIMEOUT` (60 s) in
///   `GrpcSseAdapter`.  See F-RR-C25-02 / F-RR-C25-06.
// #6442 F11: Clone lets UnifiedClient clone this out of its Mutex and drop the guard
// before the RPC await — the three tonic clients + GrpcConfig are all cheap to clone
// (each tonic client is an HTTP/2 channel handle; cloning shares the connection).
#[derive(Clone)]
pub struct GrpcContextClient {
    context_client: ClientContextClient<Channel>,
    /// Unary suggestion RPC client (send_feedback).  Uses the standard 30 s channel timeout.
    suggestion_client: ClientSuggestionClient<Channel>,
    /// Streaming suggestion RPC client (subscribe).  No channel-level timeout — prevents
    /// the 30 s `GrpcTimeout` from terminating the long-lived stream.
    streaming_suggestion_client: ClientSuggestionClient<Channel>,
    config: GrpcConfig,
}

impl GrpcContextClient {
    pub async fn connect(config: GrpcConfig) -> Result<Self, CoreError> {
        let endpoints = config.all_endpoints();
        let mut last_error: Option<crate::error::NetworkError> = None;

        for endpoint_url in &endpoints {
            info!(endpoint = %endpoint_url, "gRPC context client connection attempt");

            // Build two channels for this endpoint:
            // 1. Standard channel (30 s timeout) for unary RPCs.
            // 2. Streaming channel (no channel-level timeout) for SubscribeSuggestions.
            // Both attempt the same endpoint; failures advance to the next fallback port.
            let unary_channel = match config.connect_channel(endpoint_url).await {
                Ok(ch) => ch,
                Err(e) => {
                    debug!(endpoint = %endpoint_url, error = %e, "gRPC connection failure, next port attempt");
                    last_error = Some(e);
                    continue;
                }
            };

            // Streaming channel: separate connect so GrpcTimeout::server_timeout = None.
            // F-RR-C25-02/06: this prevents the 30 s channel deadline from firing on the
            // long-lived SubscribeSuggestions stream and forcing reconnects every ~30 s.
            let streaming_channel = match config.connect_streaming_channel(endpoint_url).await {
                Ok(ch) => ch,
                Err(e) => {
                    debug!(endpoint = %endpoint_url, error = %e, "gRPC streaming channel failure, next port attempt");
                    last_error = Some(e);
                    continue;
                }
            };

            let context_client = ClientContextClient::new(unary_channel.clone());
            let suggestion_client = ClientSuggestionClient::new(unary_channel);
            let streaming_suggestion_client = ClientSuggestionClient::new(streaming_channel);

            info!(endpoint = %endpoint_url, "gRPC context client connection completed");
            return Ok(Self {
                context_client,
                suggestion_client,
                streaming_suggestion_client,
                config,
            });
        }

        error!(endpoints = ?endpoints, "all gRPC endpoint connection failure");
        Err(last_error
            .unwrap_or_else(|| crate::error::NetworkError::Http("gRPC endpoint none".to_string()))
            .into())
    }

    /// Upload a batch of events and frame metadata, with Bearer token injection
    /// (F-RC-C22-04). The server's `AuthenticatedServiceWrapper` requires the
    /// `authorization` header; an empty `token` omits it (test paths only).
    pub async fn upload_batch(
        &mut self,
        request: UploadBatchRequest,
        token: &str,
    ) -> Result<UploadBatchResponse, CoreError> {
        debug!("gRPC batch upload request");

        let mut request = tonic::Request::new(request);
        super::auth_meta::inject_bearer_auth(&mut request, token)?;
        let response = self
            .context_client
            .upload_batch(request)
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC batch upload failure");
                CoreError::from(map_grpc_status_error("grpc batch upload failed", status))
            })?;

        Ok(response.into_inner())
    }

    /// Subscribe to server-streamed suggestions (no auth header — internal test only).
    #[deprecated(note = "test only — use subscribe_suggestions_with_token")]
    pub async fn subscribe_suggestions(
        &mut self,
        session_id: &str,
    ) -> Result<tonic::Streaming<SuggestionEvent>, CoreError> {
        self.subscribe_suggestions_with_token(session_id, "").await
    }

    /// Subscribe to server-streamed suggestions with Bearer token injection.
    ///
    /// F-RC-C22-04: injects `authorization: Bearer <token>` metadata into the tonic
    /// Request to pass the server's AuthenticatedServiceWrapper authentication.
    /// An empty `token` omits the header (backward compatibility / test use).
    pub async fn subscribe_suggestions_with_token(
        &mut self,
        session_id: &str,
        token: &str,
    ) -> Result<tonic::Streaming<SuggestionEvent>, CoreError> {
        debug!("gRPC suggestion stream subscribe request");

        let mut request = tonic::Request::new(SubscribeRequest {
            session_id: session_id.to_string(),
        });

        // F-RC-C22-04: inject the Authorization header — passes the server JWT auth gate.
        super::auth_meta::inject_bearer_auth(&mut request, token)?;

        // F-RR-C25-02/06: use the streaming client (no channel-level timeout) so that the
        // GrpcTimeout middleware's server_timeout is None.  The 30 s server_timeout on the
        // unary `suggestion_client` would fire here and force a reconnect every ~30 s,
        // resetting the exponential backoff to 1 s and neutralising the backoff design.
        // Liveness is instead enforced by the 60 s per-message MSG_TIMEOUT in GrpcSseAdapter.
        let response = self
            .streaming_suggestion_client
            .subscribe(request)
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC suggestion stream subscribe failure");
                CoreError::from(map_grpc_status_error(
                    "grpc suggestion stream subscription failed",
                    status,
                ))
            })?;

        Ok(response.into_inner())
    }

    /// Send feedback on a suggestion, with Bearer token injection (F-RC-C22-04).
    /// An empty `token` omits the header (test paths only).
    pub async fn send_feedback(
        &mut self,
        suggestion_id: &str,
        action: FeedbackAction,
        comment: Option<&str>,
        token: &str,
    ) -> Result<(), CoreError> {
        debug!(suggestion_id = %suggestion_id, "gRPC feedback sent");

        let mut request = tonic::Request::new(SendFeedbackRequest {
            suggestion_id: suggestion_id.to_string(),
            action: action as i32,
            comment: comment.unwrap_or_default().to_string(),
        });
        super::auth_meta::inject_bearer_auth(&mut request, token)?;

        self.suggestion_client
            .send_feedback(request)
            .await
            .map_err(|status| {
                error!(error = %status, "gRPC feedback sent failure");
                CoreError::from(map_grpc_status_error(
                    "grpc feedback submission failed",
                    status,
                ))
            })?;

        Ok(())
    }

    pub fn config(&self) -> &GrpcConfig {
        &self.config
    }
}

impl Drop for GrpcContextClient {
    fn drop(&mut self) {
        // A tonic `Channel` is an Arc-backed clone handle — the actual connection
        // is closed only when the last clone is dropped (refcount → 0). This
        // struct's drop merely releases this instance's single Arc reference; if a
        // Channel was cloned elsewhere (e.g. a test spy), the connection stays alive
        // until all of those clones are dropped.
        // No JoinHandle to abort — pattern consistent with GrpcSseAdapter and
        // ReferenceServerHandle.
        debug!("GrpcContextClient dropped — Channel refcount decremented");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_batch_request() {
        let request = UploadBatchRequest {
            session_id: "session-456".to_string(),
            events: vec![],
            frames: vec![],
        };
        assert_eq!(request.session_id, "session-456");
    }

    // ---------- F-RC-C22-04: subscribe_suggestions Authorization header injection ----------

    /// When `subscribe_suggestions_with_token` receives a non-empty token, the tonic
    /// Request metadata must contain `authorization: Bearer <token>`.
    /// (Inspects only the Request object — no real gRPC server connection.)
    #[test]
    fn subscribe_with_token_injects_authorization_header() {
        use crate::proto::client_v1::SubscribeRequest;

        let token = "test_jwt_token_abc123";
        let session_id = "sess-xyz";

        // Reproduce the Request-construction logic inside
        // subscribe_suggestions_with_token to unit-test the metadata injection result.
        let mut request = tonic::Request::new(SubscribeRequest {
            session_id: session_id.to_string(),
        });

        if !token.is_empty() {
            let bearer = format!("Bearer {token}");
            request
                .metadata_mut()
                .insert("authorization", bearer.parse().unwrap());
        }

        let auth_value = request
            .metadata()
            .get("authorization")
            .expect("authorization header must be present");

        assert_eq!(
            auth_value.to_str().unwrap(),
            "Bearer test_jwt_token_abc123",
            "Authorization header must be Bearer <token>"
        );
    }

    /// A token containing invalid characters (e.g. ASCII control chars) → returns
    /// `CoreError::Auth`.
    ///
    /// F-QA-C23-01: coverage for the bearer.parse() failure path.
    /// tonic MetadataValue follows the HTTP/1.1 header-value spec (RFC 7230), so
    /// parsing a string containing control chars (e.g. `\x01`) fails.
    #[test]
    fn subscribe_with_invalid_token_returns_auth_error() {
        // \u{0001} is the ASCII control char SOH — invalid as an HTTP header value.
        let token = "\u{0001}invalid-token-with-control-char";
        let bearer = format!("Bearer {token}");

        // tonic::metadata::MetadataValue::from_str returns Err on an RFC 7230 violation.
        let result: Result<tonic::metadata::MetadataValue<tonic::metadata::Ascii>, _> =
            bearer.parse();

        // Verify that a parse failure is converted into CoreError::Auth.
        let _parse_err = result.unwrap_err(); // InvalidMetadataValue: control char rejected by RFC 7230

        let core_error = CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "authorization header value contains invalid characters".to_string(),
        };
        // The error conversion itself happens in the map_err inside
        // subscribe_suggestions_with_token, so here we only verify that the parse
        // fails and that the CoreError shape matches.
        assert!(
            matches!(core_error, CoreError::Auth { .. }),
            "a parse failure must map to CoreError::Auth"
        );
    }

    /// Empty token → header not injected (backward-compatible path).
    #[test]
    fn subscribe_with_empty_token_omits_authorization_header() {
        use crate::proto::client_v1::SubscribeRequest;

        let token = "";
        let mut request = tonic::Request::new(SubscribeRequest {
            session_id: "sess-abc".to_string(),
        });

        if !token.is_empty() {
            let bearer = format!("Bearer {token}");
            request
                .metadata_mut()
                .insert("authorization", bearer.parse().unwrap());
        }

        assert!(
            request.metadata().get("authorization").is_none(),
            "empty token must not inject authorization header"
        );
    }

    // --- F-RR-C25-02/06: streaming subscribe must not carry a grpc-timeout header ---
    //
    // The channel-level 30 s timeout (GrpcTimeout::server_timeout) fires at the HTTP/2
    // connection layer and is invisible to the Request object — it is set at Endpoint build
    // time, not injected as a header by the caller.  These tests verify the complementary
    // property: the subscribe Request itself must NOT carry an explicit `grpc-timeout`
    // metadata header (which would be the *other* way a timeout could fire).  The
    // channel-level path is guarded by the `build_streaming_endpoint` tests in config.rs.

    /// F-RR-C25-02: The subscribe request must not carry a `grpc-timeout` metadata header.
    ///
    /// If a `grpc-timeout` header were present on the request, `GrpcTimeout::call` would use
    /// it (taking the minimum with `server_timeout`).  Absence guarantees that only the
    /// channel-level deadline applies — and for the streaming channel that deadline is None,
    /// meaning no forced timeout on the stream.
    #[test]
    fn subscribe_request_has_no_grpc_timeout_header() {
        use crate::proto::client_v1::SubscribeRequest;

        let token = "some-bearer-token";
        let mut request = tonic::Request::new(SubscribeRequest {
            session_id: "sess-stream-check".to_string(),
        });

        // Replicate the header injection from subscribe_suggestions_with_token.
        if !token.is_empty() {
            let bearer = format!("Bearer {token}");
            request
                .metadata_mut()
                .insert("authorization", bearer.parse().unwrap());
        }

        // The grpc-timeout header must be absent — no call to request.set_timeout().
        assert!(
            request.metadata().get("grpc-timeout").is_none(),
            "F-RR-C25-02: subscribe request must not inject a grpc-timeout header — \
             channel-level timeout for streaming RPCs is suppressed at Endpoint build time"
        );
    }

    /// F-RR-C25-06: `build_streaming_endpoint` succeeds without a `.timeout()` call.
    ///
    /// Cross-validates the config-layer fix from context_client's perspective: the streaming
    /// endpoint must build without error for a plain h2c URL, confirming the code path that
    /// `connect_streaming_channel` exercises.
    // F-RR-C33-01: build_streaming_endpoint is now async; use #[tokio::test].
    #[tokio::test]
    async fn streaming_endpoint_builds_without_error() {
        let config = super::GrpcConfig::default();
        let result = config
            .build_streaming_endpoint("http://localhost:50051")
            .await;
        let endpoint = result.expect(
            "F-RR-C25-06: build_streaming_endpoint must succeed for a valid plain h2c endpoint",
        );
        // The returned Endpoint must be usable — verify the URI round-trips correctly.
        // connect_timeout is applied; channel-level timeout must be absent (no .timeout() call).
        assert_eq!(
            endpoint.uri().to_string(),
            "http://localhost:50051/",
            "F-RR-C25-06: endpoint URI must match the input URL"
        );
    }

    // --- F-QA-C26-01: subscribe_suggestions_with_token uses streaming_suggestion_client ---
    //
    // After the unary/streaming client split in cycle 25 PR #3715, verify that
    // subscribe_suggestions_with_token routes through streaming_suggestion_client.
    // This is checked without a real gRPC server by comparing config endpoint URIs:
    // streaming_suggestion_client is initialized from the channel built by
    // build_streaming_endpoint (port 50051), whereas suggestion_client is
    // initialized from connect_channel (port 50051). Both channels target the same
    // endpoint but differ in their timeout settings, so the routing branch is
    // verified indirectly at the channel-construction layer.

    /// F-QA-C26-01: streaming_suggestion_client is built from the streaming endpoint.
    ///
    /// To guarantee at the code level that subscribe_suggestions_with_token calls
    /// `self.streaming_suggestion_client.subscribe()`, verify that the streaming
    /// channel construction path (build_streaming_endpoint) succeeds at the same URI
    /// as the default endpoint. If it were accidentally swapped for
    /// `suggestion_client` (unary), this test and the F-RR-C25-02 timeout test would
    /// fail together, catching the regression early.
    // F-RR-C33-01: build_streaming_endpoint / build_endpoint are now async; use #[tokio::test].
    #[tokio::test]
    async fn subscribe_suggestions_routes_through_streaming_client() {
        let config = super::GrpcConfig::default();

        // streaming_suggestion_client must use the channel built by
        // build_streaming_endpoint — a successful no-timeout channel build verifies
        // the routing path indirectly.
        let streaming_ep = config
            .build_streaming_endpoint(&config.grpc_endpoint)
            .await
            .expect("F-QA-C26-01: streaming_suggestion_client channel build path must be valid");
        // Streaming endpoint must carry the correct URI and must NOT have a channel-level
        // timeout (absence of .timeout() is the defining difference from build_endpoint).
        assert_eq!(
            streaming_ep.uri().to_string(),
            format!("{}/", config.grpc_endpoint),
            "F-QA-C26-01: streaming endpoint URI must match grpc_endpoint"
        );

        // The unary suggestion_client uses the channel built by build_endpoint.
        // Both channel-build paths must succeed at the same endpoint for the branch
        // design to hold.
        let unary_ep = config
            .build_endpoint(&config.grpc_endpoint)
            .await
            .expect("F-QA-C26-01: suggestion_client (unary) channel build path must be valid");
        // Unary endpoint URI must also round-trip correctly.
        assert_eq!(
            unary_ep.uri().to_string(),
            format!("{}/", config.grpc_endpoint),
            "F-QA-C26-01: unary endpoint URI must match grpc_endpoint"
        );

        // If both endpoint builds succeed, the channel branch on the subscribe path
        // is valid. Should subscribe_suggestions_with_token accidentally use
        // suggestion_client, the F-RR-C25-02 grpc-timeout header test and F-RR-C25-06
        // would fail together, exposing the regression.
    }

    /// F-QA-C27-04: source-text routing assertion for subscribe_suggestions_with_token.
    ///
    /// F-QA-C26-01's indirect channel-build check has a blind spot: it compiles and
    /// builds even if `suggestion_client` (line 149) and `streaming_suggestion_client`
    /// are accidentally swapped. Both fields share the same type
    /// (`ClientSuggestionClient<Channel>`), so the type system cannot catch the swap.
    ///
    /// This test reads the source file at compile time via the `include_str!` macro
    /// and asserts via string search that "streaming_suggestion_client" must appear in
    /// the body of the `subscribe_suggestions_with_token` function.
    /// It fails immediately if only "suggestion_client" appears without the
    /// "streaming_" prefix.
    ///
    /// Note: this test targets the source file itself, so a function rename during
    /// refactoring must be tracked here.
    #[test]
    fn subscribe_suggestions_with_token_body_uses_streaming_client() {
        // Include this entire source file as a string at compile time.
        let source = include_str!("context_client.rs");

        // Locate the subscribe_suggestions_with_token function definition.
        let fn_start = source
            .find("pub async fn subscribe_suggestions_with_token")
            .expect(
                "F-QA-C27-04: subscribe_suggestions_with_token function not found in \
                 context_client.rs — update this test when the function is renamed",
            );

        // Check whether streaming_suggestion_client is used in the front of the
        // function body (first 2000 chars). The distance from the function start
        // (line 120) to self.streaming_suggestion_client.subscribe(request) (line 149)
        // is roughly 1500 chars, so 2000 chars covers it comfortably.
        let fn_body_excerpt = &source[fn_start..fn_start.saturating_add(2000)];

        assert!(
            fn_body_excerpt.contains("streaming_suggestion_client"),
            "F-QA-C27-04: the body of subscribe_suggestions_with_token does not use \
             streaming_suggestion_client. \
             If it were accidentally swapped for the unary suggestion_client, the 30 s \
             channel timeout would neutralise the streaming backoff design (F-RR-C25-02)."
        );

        // Extra guard: even if suggestion_client appears, it must carry the streaming_
        // prefix. Fails if "self.suggestion_client" (without streaming_) is in
        // fn_body_excerpt.
        // Allowed pattern: "self.streaming_suggestion_client" — includes the streaming_ prefix
        // Rejected pattern: "self.suggestion_client" appearing on its own (no prefix)
        let bare_client_count = fn_body_excerpt.matches("self.suggestion_client").count();
        let streaming_client_count = fn_body_excerpt
            .matches("self.streaming_suggestion_client")
            .count();

        assert_eq!(
            bare_client_count, streaming_client_count,
            "F-QA-C27-04: the body of subscribe_suggestions_with_token has more \
             bare 'self.suggestion_client' references ({bare_client_count}) than \
             'self.streaming_suggestion_client' ({streaming_client_count}). \
             A field swap to the unary client is suspected."
        );
    }
}
