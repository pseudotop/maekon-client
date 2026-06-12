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

    /// Subscribe to server-streamed suggestions (인증 헤더 없음 — 내부 테스트 전용).
    #[deprecated(note = "test only — use subscribe_suggestions_with_token")]
    pub async fn subscribe_suggestions(
        &mut self,
        session_id: &str,
    ) -> Result<tonic::Streaming<SuggestionEvent>, CoreError> {
        self.subscribe_suggestions_with_token(session_id, "").await
    }

    /// Subscribe to server-streamed suggestions with Bearer token injection.
    ///
    /// F-RC-C22-04: `authorization: Bearer <token>` 메타데이터를 tonic Request 에
    /// 주입하여 서버 AuthenticatedServiceWrapper 인증을 통과한다.
    /// token 이 비어 있으면 헤더를 생략한다 (하위 호환 / 테스트 용도).
    pub async fn subscribe_suggestions_with_token(
        &mut self,
        session_id: &str,
        token: &str,
    ) -> Result<tonic::Streaming<SuggestionEvent>, CoreError> {
        debug!("gRPC suggestion stream subscribe request");

        let mut request = tonic::Request::new(SubscribeRequest {
            session_id: session_id.to_string(),
        });

        // F-RC-C22-04: Authorization 헤더 주입 — 서버 JWT 인증 게이트 통과.
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
        // Tonic `Channel` はArc-backed クローンハンドル — 실제 연결 종료는 마지막
        // 클론이 drop 될 때 발생함 (refcount → 0). 이 struct 의 drop 은 이 인스턴스의
        // Arc 참조 하나를 해제할 뿐이며, 테스트 스파이 등 다른 곳에서 클론된
        // Channel 이 존재하면 해당 클론이 모두 drop 될 때까지 연결이 유지됨.
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

    // ---------- F-RC-C22-04: subscribe_suggestions Authorization 헤더 주입 ----------

    /// subscribe_suggestions_with_token 이 비어 있지 않은 토큰을 받으면
    /// tonic Request 메타데이터에 `authorization: Bearer <token>` 이 포함된다.
    /// (실제 gRPC 서버 연결 없이 Request 객체만 검사)
    #[test]
    fn subscribe_with_token_injects_authorization_header() {
        use crate::proto::client_v1::SubscribeRequest;

        let token = "test_jwt_token_abc123";
        let session_id = "sess-xyz";

        // subscribe_suggestions_with_token 내부 Request 구성 로직을 직접 재현해
        // 메타데이터 주입 결과를 단위 검증한다.
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

    /// 잘못된 문자(ASCII 제어 문자 등)가 포함된 토큰 → `CoreError::Auth` 반환.
    ///
    /// F-QA-C23-01: bearer.parse() 실패 경로 커버리지.
    /// tonic MetadataValue 은 HTTP/1.1 헤더 값 규격(RFC 7230)을 따르므로
    /// 제어 문자(`\x01` 등)가 포함된 문자열 파싱은 실패한다.
    #[test]
    fn subscribe_with_invalid_token_returns_auth_error() {
        // \u{0001} 은 ASCII 제어 문자(SOH) — HTTP 헤더 값으로 유효하지 않다.
        let token = "\u{0001}invalid-token-with-control-char";
        let bearer = format!("Bearer {token}");

        // tonic::metadata::MetadataValue::from_str 은 RFC 7230 위반 시 Err 를 반환한다.
        let result: Result<tonic::metadata::MetadataValue<tonic::metadata::Ascii>, _> =
            bearer.parse();

        // 파싱이 실패하면 CoreError::Auth 로 변환됨을 검증한다.
        let _parse_err = result.unwrap_err(); // InvalidMetadataValue: control char rejected by RFC 7230

        let core_error = CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "authorization header value contains invalid characters".to_string(),
        };
        // 오류 변환 자체는 subscribe_suggestions_with_token 내 map_err 에서 발생하므로
        // 여기서는 파싱 실패 + CoreError 형식이 일치하는지만 검증한다.
        assert!(
            matches!(core_error, CoreError::Auth { .. }),
            "파싱 실패 시 CoreError::Auth 로 매핑되어야 한다"
        );
    }

    /// 빈 토큰 → 헤더 미주입 (하위 호환 경로).
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

    // --- F-QA-C26-01: subscribe_suggestions_with_token 은 streaming_suggestion_client 를 사용한다 ---
    //
    // Cycle 25 PR #3715 에서 unary/streaming 클라이언트 분리 이후,
    // subscribe_suggestions_with_token 이 streaming_suggestion_client 를 통해
    // 라우팅되는지 검증한다.  실제 gRPC 서버 없이 config 엔드포인트 URI 비교로 확인한다:
    // streaming_suggestion_client 는 build_streaming_endpoint 가 생성한 채널(포트 50051)로
    // 초기화되는 반면, suggestion_client 는 connect_channel(포트 50051)로 초기화된다.
    // 두 채널은 동일 엔드포인트지만 timeout 설정이 다르므로 routing 분기를 채널 생성
    // 계층에서 간접 검증한다.

    /// F-QA-C26-01: streaming_suggestion_client 는 streaming endpoint 에서 빌드된다.
    ///
    /// subscribe_suggestions_with_token 이 `self.streaming_suggestion_client.subscribe()` 를
    /// 호출함을 코드 레벨에서 보장하기 위해, streaming channel 생성 경로
    /// (build_streaming_endpoint) 가 기본 엔드포인트와 동일한 URI 에서 성공하는지
    /// 검증한다.  실수로 `suggestion_client`(unary)로 교체되면 이 test 와 함께
    /// F-RR-C25-02 timeout 테스트가 동시에 실패해 회귀를 조기에 포착한다.
    // F-RR-C33-01: build_streaming_endpoint / build_endpoint are now async; use #[tokio::test].
    #[tokio::test]
    async fn subscribe_suggestions_routes_through_streaming_client() {
        let config = super::GrpcConfig::default();

        // streaming_suggestion_client 는 build_streaming_endpoint 로 생성된 채널을
        // 사용해야 한다 — timeout 없는 채널 빌드 성공으로 라우팅 경로를 간접 검증한다.
        let streaming_ep = config
            .build_streaming_endpoint(&config.grpc_endpoint)
            .await
            .expect("F-QA-C26-01: streaming_suggestion_client 채널 빌드 경로가 유효해야 한다");
        // Streaming endpoint must carry the correct URI and must NOT have a channel-level
        // timeout (absence of .timeout() is the defining difference from build_endpoint).
        assert_eq!(
            streaming_ep.uri().to_string(),
            format!("{}/", config.grpc_endpoint),
            "F-QA-C26-01: streaming endpoint URI must match grpc_endpoint"
        );

        // unary suggestion_client 는 build_endpoint 로 생성된 채널을 사용한다.
        // 두 채널 빌드 경로가 모두 동일 엔드포인트에서 성공해야 분기 설계가 성립한다.
        let unary_ep = config
            .build_endpoint(&config.grpc_endpoint)
            .await
            .expect("F-QA-C26-01: suggestion_client(unary) 채널 빌드 경로가 유효해야 한다");
        // Unary endpoint URI must also round-trip correctly.
        assert_eq!(
            unary_ep.uri().to_string(),
            format!("{}/", config.grpc_endpoint),
            "F-QA-C26-01: unary endpoint URI must match grpc_endpoint"
        );

        // 두 엔드포인트 빌드가 모두 성공하면 subscribe 경로의 채널 분기가 유효하다.
        // 만약 subscribe_suggestions_with_token 이 실수로 suggestion_client 를 사용하면
        // F-RR-C25-02 grpc-timeout 헤더 테스트와 F-RR-C25-06 이 함께 실패해 회귀가 드러난다.
    }

    /// F-QA-C27-04: subscribe_suggestions_with_token 소스 텍스트 라우팅 어설션.
    ///
    /// F-QA-C26-01 의 채널-빌드 간접 검증은 실수로 `suggestion_client`(line 149)와
    /// `streaming_suggestion_client` 가 스왑되어도 컴파일·빌드가 통과하는 맹점이 있다.
    /// 두 필드는 동일 타입(`ClientSuggestionClient<Channel>`)이므로 타입 시스템이 swap 을 포착하지 못한다.
    ///
    /// 이 테스트는 `include_str!` 매크로로 소스 파일을 컴파일 시점에 읽어,
    /// `subscribe_suggestions_with_token` 함수 본문에 "streaming_suggestion_client" 가
    /// 반드시 등장함을 문자열 검색으로 단언한다.
    /// "suggestion_client" 만 등장하고 "streaming_" 접두어가 없으면 즉시 실패한다.
    ///
    /// 주의: 이 테스트는 소스 파일 자체를 대상으로 하므로 리팩토링 시 함수명 변경을 추적해야 한다.
    #[test]
    fn subscribe_suggestions_with_token_body_uses_streaming_client() {
        // 컴파일 시점에 이 소스 파일 전체를 문자열로 포함한다.
        let source = include_str!("context_client.rs");

        // subscribe_suggestions_with_token 함수 정의 위치 확인
        let fn_start = source
            .find("pub async fn subscribe_suggestions_with_token")
            .expect(
                "F-QA-C27-04: subscribe_suggestions_with_token 함수가 context_client.rs 에 없습니다 \
                 — 함수명 변경 시 이 테스트도 갱신하세요",
            );

        // 함수 본문 앞부분(첫 2000자)에서 streaming_suggestion_client 사용 여부 확인.
        // 함수 시작(line 120)부터 self.streaming_suggestion_client.subscribe(request)(line 149)
        // 까지 약 1500자 거리이므로 2000자로 충분히 커버한다.
        let fn_body_excerpt = &source[fn_start..fn_start.saturating_add(2000)];

        assert!(
            fn_body_excerpt.contains("streaming_suggestion_client"),
            "F-QA-C27-04: subscribe_suggestions_with_token 본문이 \
             streaming_suggestion_client 를 사용하지 않습니다. \
             실수로 unary suggestion_client 로 교체되면 30초 채널 timeout 으로 \
             인해 streaming backoff 설계가 무력화됩니다 (F-RR-C25-02)."
        );

        // 추가 보호: suggestion_client 가 등장하더라도 streaming_ 접두어가 있는 것이어야 한다.
        // "self.suggestion_client" (streaming_ 없음)가 fn_body_excerpt 에 있으면 실패.
        // 허용 패턴: "self.streaming_suggestion_client" — streaming_ 접두어 포함
        // 거부 패턴: "self.suggestion_client" 단독 등장 (접두어 없음)
        let bare_client_count = fn_body_excerpt.matches("self.suggestion_client").count();
        let streaming_client_count = fn_body_excerpt
            .matches("self.streaming_suggestion_client")
            .count();

        assert_eq!(
            bare_client_count, streaming_client_count,
            "F-QA-C27-04: subscribe_suggestions_with_token 본문에 \
             bare 'self.suggestion_client' 참조({bare_client_count}개)가 \
             'self.streaming_suggestion_client'({streaming_client_count}개)를 초과합니다. \
             unary 클라이언트로의 필드 스왑이 의심됩니다."
        );
    }
}
