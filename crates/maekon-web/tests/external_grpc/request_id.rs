// D14 request-id correlation integration tests — `RequestIdLayer` header
// preservation/generation/replacement, plus the auth-rejection boundary.
// Split from `external_grpc_integration.rs` by scenario family (#7730).

use std::sync::Arc;
use std::time::Duration;

use maekon_core::models::audit::AuditStatus;
use maekon_core::ports::audit_log::AuditLogPort;
use tonic::Code;

use maekon_web::grpc::external::test_support::{
    server_cert_pem, test_jwt_keypair, test_mint_jwt, CapturingAudit,
};
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use maekon_web::proto::dashboard::v1::GetAgentInfoRequest;

use crate::common::{make_jwt_config, make_tls_channel, spawn_server};

/// R1 — Test 10: RequestIdLayer preserves a valid client-supplied x-request-id.
///
/// Per spec §5.2 / D31: when the client sends a valid `x-request-id` header
/// (ASCII graphic, 1..=128 chars), `RequestIdLayer` echoes that EXACT value in
/// the response — it does NOT overwrite a matching value.
///
/// Assertion: the response metadata carries `x-request-id: test-req-123`
/// exactly as supplied, proving the conditional-overwrite path (D31) works
/// end-to-end through the real `serve_external` stack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_header_returned() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Attach both authorization AND a valid x-request-id header.
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "x-request-id",
        tonic::metadata::MetadataValue::try_from("test-req-123").expect("valid x-request-id value"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth should succeed and yield AgentInfoResponse");

    // The x-request-id that the server echoed must be the exact client value.
    let returned_id = resp
        .metadata()
        .get("x-request-id")
        .expect("x-request-id must be present in response metadata")
        .to_str()
        .expect("x-request-id must be valid ASCII");
    assert_eq!(
        returned_id, "test-req-123",
        "RequestIdLayer (D31) must preserve the client-supplied x-request-id unchanged"
    );

    // Also verify the handler returned real business data (smoke).
    let info = resp.into_inner();
    assert!(
        !info.build_profile.is_empty(),
        "AgentInfoResponse.build_profile must be populated"
    );

    handle.abort();
    let _ = handle.await;
}

/// N1 — RequestIdLayer generates an ADR-022 prefix+ULID when the client omits x-request-id.
///
/// When no `x-request-id` header is sent, `RequestIdLayer` (spec §5.2 / None
/// branch) generates a fresh prefix+ULID ("req_<26>") and inserts it into the response.
/// The CapturingAudit's Completed row carries that same ID as `command_id` via
/// AuditLayer's request_id override (U5), proving end-to-end propagation.
///
/// Assertions:
/// 1. Response metadata has `x-request-id` (server-generated).
/// 2. The value is a valid ADR-022 prefix+ULID (starts with "req_", 30 chars total).
/// 3. The CapturingAudit Completed row's `command_id` matches the response header.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_generated_when_missing() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-gen-id",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // No x-request-id header — RequestIdLayer must generate an ADR-022 prefix+ULID.
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth + real handler → Ok");

    // 1. Response must carry a server-generated x-request-id.
    let generated_id = resp
        .metadata()
        .get("x-request-id")
        .expect("server must insert x-request-id when client omits it")
        .to_str()
        .expect("x-request-id must be valid ASCII");

    // 2. Must be an ADR-022 prefix+ULID (req_<26-char ULID>).
    assert!(
        generated_id.starts_with("req_"),
        "generated x-request-id must start with 'req_'; got {generated_id:?}"
    );
    assert_eq!(
        generated_id.len(),
        "req_".len() + 26,
        "generated x-request-id must be req_ + 26-char ULID; got {generated_id:?}"
    );

    // Give AuditLayer's deferred task time to flush to the mock.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. CapturingAudit's Completed row must carry the same ID as command_id.
    // Drop the lock before any `.await`.
    let (audit_cmd_id, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let completed = entries
            .iter()
            .find(|e| matches!(e.status, AuditStatus::Completed) && e.grpc_status_code.is_some())
            .map(|e| e.command_id.clone())
            .unwrap_or_default();
        let dbg = format!("{entries:?}");
        (completed, dbg)
    };
    assert_eq!(
        audit_cmd_id, generated_id,
        "AuditLayer command_id must equal the x-request-id echoed in the response; \
         entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

/// N2 — RequestIdLayer discards a malformed client x-request-id and substitutes
/// a fresh ADR-022 prefix+ULID.
///
/// Per spec §5.2 / L307: when the client sends an `x-request-id` that fails
/// `is_valid()` (ASCII graphic 0x21..=0x7E, 1..=128 chars), `RequestIdLayer`
/// emits a `tracing::warn!` and generates a fresh prefix+ULID.  The warn+regenerate
/// path proves that a malicious / malformed client cannot inject arbitrary
/// bytes into the response-header / downstream audit trail.
///
/// The malformed payload used here is `"bad\tid"` — the tab byte (0x09) is a
/// valid HeaderValue byte (HTAB is permitted by `http::HeaderValue::from_str`)
/// but falls outside the `is_valid()` 0x21..=0x7E range, so the server-side
/// validator will reject it and substitute an ADR-022 ID.  Mirrors the in-crate
/// `rejects_invalid_characters_generates_new` unit test (request_id_layer.rs L189).
///
/// Assertions:
/// 1. Response metadata carries a valid ADR-022 prefix+ULID ("req_" + 26-char ULID).
/// 2. Response's `x-request-id` does NOT equal the malformed client input.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_invalid_replaced() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-bad-reqid",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Malformed x-request-id: tab (0x09) is valid as an http HeaderValue byte
    // but fails the is_valid(0x21..=0x7E) range, forcing the warn+regenerate path.
    let malformed_id = "bad\tid";
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "x-request-id",
        tonic::metadata::MetadataValue::try_from(malformed_id)
            .expect("tab (0x09) is a valid HeaderValue byte"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth + real handler → Ok");

    // 1. Response must carry a server-substituted x-request-id.
    let returned_id = resp
        .metadata()
        .get("x-request-id")
        .expect("x-request-id must be present (server substitutes on invalid input)")
        .to_str()
        .expect("substituted x-request-id must be valid ASCII");

    // 2. Value must NOT be the malformed client input.
    assert_ne!(
        returned_id, malformed_id,
        "server must discard malformed x-request-id and substitute an ADR-022 ID"
    );

    // 3. Value must be an ADR-022 prefix+ULID (req_<26-char ULID>).
    assert!(
        returned_id.starts_with("req_"),
        "substituted x-request-id must start with 'req_'; got {returned_id:?}"
    );
    assert_eq!(
        returned_id.len(),
        "req_".len() + 26,
        "substituted x-request-id must be req_ + 26-char ULID; got {returned_id:?}"
    );

    handle.abort();
    let _ = handle.await;
}

/// N3 — x-request-id is preserved across the auth-rejection boundary (U5 / D14).
///
/// Per spec §5.2 / §9.2 L1393: `RequestIdLayer` is the outermost layer and runs
/// BEFORE `AuthLayer`, so it inserts the `RequestId` extension with the client's
/// header value before any auth gate fires.  When `AuthLayer` subsequently
/// rejects the request (invalid JWT → Unauthenticated), its Failed-path
/// `bridge.record(...)` reads the extension and passes it as `command_id`
/// (commit `7bd7c944`, Task 6.1).  This closes the correlation gap at the
/// security boundary — security dashboards can still trace which client call
/// produced each auth rejection.
///
/// Flow:
/// 1. Client sends `x-request-id: req-abc-123` + a JWT signed with a wrong issuer.
/// 2. Server's `RequestIdLayer` validates the header (passes) and inserts
///    `RequestId("req-abc-123")` into request extensions.
/// 3. `AuthLayer`'s JWT gate calls `verifier.verify(tok)`, which fails (wrong
///    issuer), and takes the `invalid_jwt` Failed-path.
/// 4. The Failed-path reads the `RequestId` extension and calls
///    `bridge.record(..., Some("req-abc-123"))`, which persists the Failed
///    audit row with `command_id = "req-abc-123"`.
///
/// Assertions:
/// 1. RPC returns `Err(Status)` with code `Unauthenticated` (16).
/// 2. CapturingAudit captures ≥1 Failed audit row.
/// 3. That Failed row's `command_id` equals `"req-abc-123"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_preserved_across_auth_reject() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    // Invalid JWT — wrong issuer → JwtVerifier::verify() fails → invalid_jwt path.
    let bad_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-auth-reject",
        "wrong-issuer", // mismatch with verifier's "test-issuer" → verify fails
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Valid x-request-id — passes is_valid() (all ASCII graphic chars).
    let client_req_id = "req-abc-123";
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {bad_token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "x-request-id",
        tonic::metadata::MetadataValue::try_from(client_req_id).expect("valid x-request-id value"),
    );

    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;

    // 1. RPC must fail with Unauthenticated (invalid_jwt path).
    let status = result.expect_err("wrong-issuer JWT must yield Err");
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "invalid JWT must yield Unauthenticated (code 16); got {status:?}"
    );

    // Give the tokio::spawn'd AuthLayer Failed-path record() time to flush.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 2 + 3. The auth-rejection audit row must carry the client's x-request-id
    // as command_id. AuthLayer's Failed-path calls both `log_complete_with_time`
    // (writes command_id + details JSON) and `log_event("external_grpc_failed")`
    // (prefix-queryable marker).  We locate the authoritative auth-rejection
    // row by its details payload (`"result":"auth_failed"` + `"failure_reason":
    // "invalid_jwt"`) — this is the row whose command_id must equal the client's
    // x-request-id per U5/D14.
    //
    // We filter by details content rather than `AuditStatus::Failed` because
    // `Failed` also covers "error"/"failed" results from non-auth paths, while
    // the substring match pins down the exact `failure_reason: "invalid_jwt"`
    // path under test.
    //
    // The `!e.command_id.is_empty()` predicate disambiguates the two audit rows
    // that share the same details JSON: `log_complete_with_time` (L1657 in
    // CapturingAudit) populates `command_id` from the forwarded request-id,
    // whereas `log_event` (L1615) hard-codes `String::new()`.  If
    // CapturingAudit is ever refactored so that `log_event` also populates
    // `command_id`, this filter will match both rows and `auth_failed.first()`
    // will non-deterministically return either one.  In that case tighten the
    // predicate (e.g., match on the event type string) or assert
    // `auth_failed_count == 1` to catch the ambiguity at test time.
    //
    // Drop the lock before any `.await`.
    let (auth_failed_count, auth_failed_cmd_id, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let auth_failed: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.details
                    .as_deref()
                    .map(|d| {
                        d.contains("\"result\":\"auth_failed\"")
                            && d.contains("\"failure_reason\":\"invalid_jwt\"")
                    })
                    .unwrap_or(false)
                    && !e.command_id.is_empty()
            })
            .collect();
        let cmd_id = auth_failed
            .first()
            .map(|e| e.command_id.clone())
            .unwrap_or_default();
        let dbg = format!("{entries:?}");
        (auth_failed.len(), cmd_id, dbg)
    };
    assert!(
        auth_failed_count >= 1,
        "expected ≥1 auth-rejection audit row with populated command_id + \
         details.result='auth_failed' + failure_reason='invalid_jwt'; \
         got {auth_failed_count} (entries: {entries_debug})"
    );
    assert_eq!(
        auth_failed_cmd_id, client_req_id,
        "auth-rejection audit row's command_id must equal the client's x-request-id \
         (U5/D14 correlation preserved at security boundary); entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}
