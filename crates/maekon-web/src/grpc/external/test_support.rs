//! Shared test helpers for external gRPC integration tests.
//!
//! Gated on `#[cfg(any(test, feature = "test-support"))]` — the `test-support`
//! feature is strictly opt-in (NEVER enabled by default or transitively via
//! `grpc-dashboard-external`). Integration tests must invoke with
//! `--features grpc-dashboard-external,external-grpc-tools,test-support`.
//!
//! **ES256 note**: the `ring` crypto backend (default for rcgen) does NOT support
//! RSA key generation at runtime (RSA key-gen requires platform PRNG that ring
//! intentionally omits). All JWT helpers here use ES256 (P-256 ECDSA) which ring
//! fully supports.
//!
//! **rcgen 0.14 signed_by API**: `CertificateParams::signed_by` takes two arguments:
//! `(self, public_key: &impl PublicKeyData, issuer: &Issuer<'_, impl SigningKey>)`.
//! Create the `Issuer` using `Issuer::from_ca_cert_pem`.
//!
//! # Phase 0-9 test fixture API (CR4)
//!
//! - [`fixture_bridge`] / [`fixture_metrics`] — Tasks 0.6, 3.1
//! - [`InnerEcho`] — Tasks 0.6, 3.1 (trailer-status simulation)
//! - [`AuthContext::fixture`] / [`PeerInfo::fixture`] — Tasks 0.6, 3.1, 6.1
//! - [`PassthroughInner`] — Task 6.1
//! - [`connect_loopback`] / [`req_with_valid_auth`] — Task 9.4 G3 test dependencies

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};
use tempfile::TempDir;

// ── rustls crypto provider ────────────────────────────────────────────────────

static RUSTLS_INIT: Once = Once::new();

/// Install the aws-lc-rs CryptoProvider as the process-level default for rustls.
///
/// rustls 0.23 requires an explicit provider when both `aws-lc-rs` and `ring`
/// are present in the dependency graph. Tests that call
/// `rustls::ServerConfig::builder()` or `WebPkiClientVerifier::builder()`
/// must call this function first — those paths consult the process-level
/// default, which is unset unless installed explicitly.
///
/// Idempotent: the `Once` guard ensures the install runs at most once per
/// process, regardless of how many tests call this function.
pub fn install_rustls_crypto_provider() {
    RUSTLS_INIT.call_once(|| {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

// ── Server TLS cert pair ─────────────────────────────────────────────────────

/// Cached (TempDir, cert_pem_path, key_pem_path). The `TempDir` must be kept
/// alive for the lifetime of the process so the files remain on disk.
static CERT_CACHE: OnceLock<(TempDir, PathBuf, PathBuf)> = OnceLock::new();

/// Return cached (cert_pem_path, key_pem_path) for a self-signed server cert.
///
/// The cert has SANs `localhost` + `127.0.0.1`. Files are written once and
/// re-used across all tests in the same process invocation.
pub fn test_cert_pair() -> (PathBuf, PathBuf) {
    let (_, cp, kp) = CERT_CACHE.get_or_init(|| {
        use rcgen::{CertificateParams, KeyPair, SanType};
        let dir = TempDir::new().expect("TempDir for server cert");
        let kp = KeyPair::generate().expect("server keypair");
        let mut params =
            CertificateParams::new(vec!["localhost".to_string()]).expect("cert params");
        params
            .subject_alt_names
            .push(SanType::IpAddress(IpAddr::from([127, 0, 0, 1])));
        let cert = params.self_signed(&kp).expect("self-signed server cert");
        let cp = dir.path().join("cert.pem");
        let kp_p = dir.path().join("key.pem");
        std::fs::write(&cp, cert.pem()).expect("write cert.pem");
        std::fs::write(&kp_p, kp.serialize_pem()).expect("write key.pem");
        (dir, cp, kp_p)
    });
    (cp.clone(), kp.clone())
}

// ── JWT key pair ─────────────────────────────────────────────────────────────

/// JWT test key pair — public key path on disk + encoding key in memory.
pub struct TestJwt {
    /// Path to the EC public key PEM (used to configure `JwtVerifier`).
    pub pub_pem_path: PathBuf,
    /// Encoding key for minting tokens inside tests.
    pub enc_key: jsonwebtoken::EncodingKey,
    /// Keep-alive for the temp directory that holds the public key file.
    pub _dir: TempDir,
}

/// Generate an ES256 key pair and write the public key to a temp file.
///
/// ES256 is used instead of RS256 because the `ring` backend (default rcgen
/// feature) does not support RSA key generation. ES256 uses P-256 ECDSA which
/// ring fully supports.
pub fn test_jwt_keypair() -> TestJwt {
    use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
    let dir = TempDir::new().expect("TempDir for JWT keypair");
    let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("EC P-256 keypair");
    let pub_pem = kp.public_key_pem();
    let pub_pem_path = dir.path().join("jwt_pub.pem");
    std::fs::write(&pub_pem_path, &pub_pem).expect("write jwt_pub.pem");
    // The encoding key needs the private key in EC PEM format.
    let enc_key = jsonwebtoken::EncodingKey::from_ec_pem(kp.serialize_pem().as_bytes())
        .expect("EncodingKey from EC PEM");
    TestJwt {
        pub_pem_path,
        enc_key,
        _dir: dir,
    }
}

/// Mint an ES256 JWT with the given claims.
///
/// - `exp_offset_secs`: added to `now()` for the `exp` claim. Use a negative
///   value to produce an already-expired token.
pub fn test_mint_jwt(
    enc: &jsonwebtoken::EncodingKey,
    sub: &str,
    iss: &str,
    aud: &str,
    exp_offset_secs: i64,
) -> String {
    use jsonwebtoken::{encode, Algorithm, Header};
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after epoch")
        .as_secs() as i64;
    let claims = serde_json::json!({
        "sub": sub,
        "iss": iss,
        "aud": aud,
        "exp": now + exp_offset_secs,
        "iat": now,
    });
    encode(&Header::new(Algorithm::ES256), &claims, enc).expect("encode JWT")
}

// ── CA + client cert ─────────────────────────────────────────────────────────

/// CA + client certificate issued by that CA.
pub struct TestCaAndClient {
    /// Path to the CA cert PEM (used as `mtls_ca_path`).
    pub ca_pem_path: PathBuf,
    /// Path to the client cert PEM.
    pub client_cert_pem_path: PathBuf,
    /// Path to the client private key PEM.
    pub client_key_pem_path: PathBuf,
    /// Client cert DER bytes — convenience for `MtlsVerifier` tests.
    pub client_cert_der: Vec<u8>,
    /// Keep-alive for the temp directory.
    pub _dir: TempDir,
}

/// Generate a CA cert + a client cert signed by that CA.
///
/// `lifetime_hours` controls the client cert validity window. Use values
/// ≤ 48 for "accepted" tests and > 48 for "rejected" tests (the default
/// `mtls_max_cert_lifetime_hours` cap is 48).
///
/// **rcgen 0.14 API note**: `Issuer::new(params, key)` does not require the
/// `x509-parser` feature (unlike `from_ca_cert_pem`/`from_ca_cert_der`). We
/// build the CA cert via `self_signed` and simultaneously construct an `Issuer`
/// from a duplicate CA params set (rcgen 0.14 `Issuer::new` consumes params).
pub fn test_ca_and_client_cert(lifetime_hours: i64) -> TestCaAndClient {
    use chrono::{Datelike, Duration as ChronoDuration, Utc};
    use rcgen::{BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair};

    let dir = TempDir::new().expect("TempDir for CA+client certs");

    // ── CA ────────────────────────────────────────────────────────────────────
    let ca_kp = KeyPair::generate().expect("CA keypair");

    // Build CA params — we need two independent sets because both self_signed
    // (for the cert file) and Issuer::new (for signing the client cert) consume
    // CertificateParams. We construct the cert for the CA PEM file first, then
    // build the Issuer from a second params set.
    let make_ca_params = || -> CertificateParams {
        let mut p = CertificateParams::new(vec!["test-ca".to_string()]).expect("CA params");
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.distinguished_name
            .push(rcgen::DnType::CommonName, "test-ca");
        p
    };

    // CA cert — written as PEM for `mtls_ca_path`.
    let ca_cert = make_ca_params()
        .self_signed(&ca_kp)
        .expect("CA self-signed cert");
    let ca_pem_path = dir.path().join("ca.pem");
    std::fs::write(&ca_pem_path, ca_cert.pem()).expect("write ca.pem");

    // Build an Issuer using a fresh CA params copy + the CA keypair.
    // Issuer::new does NOT need x509-parser (unlike from_ca_cert_pem).
    let issuer: Issuer<'_, KeyPair> = Issuer::new(make_ca_params(), ca_kp);

    // ── Client cert ───────────────────────────────────────────────────────────
    let client_kp = KeyPair::generate().expect("client keypair");
    let now = Utc::now();
    let expiry = now + ChronoDuration::hours(lifetime_hours);
    let mut client_params =
        CertificateParams::new(vec!["test-client".to_string()]).expect("client params");
    client_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "test-client");
    // rcgen 0.14 uses date_time_ymd helper for not_before / not_after.
    client_params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    client_params.not_after =
        rcgen::date_time_ymd(expiry.year(), expiry.month() as u8, expiry.day() as u8);
    // rcgen 0.14: signed_by(public_key, &issuer) — public_key is the subject's key.
    let client_cert = client_params
        .signed_by(&client_kp, &issuer)
        .expect("client cert signed by CA");

    let client_cert_pem_path = dir.path().join("client_cert.pem");
    let client_key_pem_path = dir.path().join("client_key.pem");
    std::fs::write(&client_cert_pem_path, client_cert.pem()).expect("write client_cert.pem");
    std::fs::write(&client_key_pem_path, client_kp.serialize_pem()).expect("write client_key.pem");

    let client_cert_der = client_cert.der().to_vec();

    TestCaAndClient {
        ca_pem_path,
        client_cert_pem_path,
        client_key_pem_path,
        client_cert_der,
        _dir: dir,
    }
}

// ── AuthContext::fixture / PeerInfo::fixture ─────────────────────────────────

use super::conn_info::{AuthContext, AuthType, PeerInfo};

impl AuthContext {
    /// Canonical fixture used by Tasks 0.6, 3.1, 6.1, 9.4.
    ///
    /// Returns a deterministic `AuthContext` with `AuthType::Jwt`, a fixed
    /// `client_id`, a fixed `jti`, and a stable ULID-shaped `command_id`.
    pub fn fixture() -> Self {
        Self {
            auth_type: AuthType::Jwt,
            client_id: "test-client".into(),
            jti: Some("test-jti".into()),
            command_id: "01HXFIXTURE0000000000000000".into(),
        }
    }
}

impl PeerInfo {
    /// Canonical fixture used by Tasks 0.6, 3.1, 6.1, 9.4.
    ///
    /// Returns `127.0.0.1:50001` with no mTLS cert and TLS 1.3 version string.
    pub fn fixture() -> Self {
        Self {
            remote_addr: "127.0.0.1:50001".parse().expect("fixture addr"),
            peer_cert_der: None,
            cert_subject_cn: None,
            tls_version: "TLSv1.3".into(),
        }
    }
}

// ── InnerEcho — minimal tower::Service returning preset HTTP responses ────────

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use http::HeaderMap;
use http_body::{Body, Frame};

/// `http_body::Body` impl used by [`InnerEcho`] so `AuditLayer::call` can
/// wrap the response body with `TrailerCapturingBody<RespBody>` (which
/// requires `RespBody: http_body::Body`).
///
/// Two shapes:
/// - **Body + trailer**: `data = Some(...)`, `trailers = Some(...)` — the
///   `poll_frame` returns one `Frame::data` then one `Frame::trailers`.
/// - **Trailers-only (empty body)**: `data = None`, `trailers = None` — the
///   `poll_frame` immediately returns `Ready(None)`. The grpc-status lives
///   in INITIAL headers per the tonic `Err(Status)` trailers-only convention.
///
/// Fields are `pub` so tests can construct bespoke bodies directly (e.g.
/// `SlowInner` in audit_layer tests that need a known delay before the
/// trailer frame is emitted).
pub struct EchoBody {
    pub data: Option<Bytes>,
    pub trailers: Option<HeaderMap>,
}

impl Body for EchoBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(d) = self.data.take() {
            return Poll::Ready(Some(Ok(Frame::data(d))));
        }
        if let Some(t) = self.trailers.take() {
            return Poll::Ready(Some(Ok(Frame::trailers(t))));
        }
        Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.data.is_none() && self.trailers.is_none()
    }
}

/// Minimal `tower::Service` test double that echoes a preset HTTP response
/// with a `grpc-status` code encoded per one of two tonic conventions:
///
/// - [`InnerEcho::with_trailer_status`] — non-empty body + a **trailer frame**
///   carrying `grpc-status`. Simulates an Ok unary response or streaming
///   response terminating cleanly. `AuditLayer` observes the code via
///   `TrailerCapturingBody::poll_frame` when the body is polled.
///
/// - [`InnerEcho::trailers_only_with_status`] — empty body + `grpc-status`
///   in **initial response headers**, no body trailer frame. Simulates
///   `tonic::Status::from` (handler `Err(Status)`) which constructs a
///   trailers-only HTTP/2 response. `AuditLayer` observes the code via
///   `response.headers().get("grpc-status")` BEFORE wrapping the body
///   (spec §5.5 D28 header-first path).
#[derive(Clone)]
pub struct InnerEcho {
    grpc_status: i32,
    body_bytes: &'static [u8],
    /// When `true`, emit `grpc-status` in INITIAL headers + empty body (no
    /// trailer frame). When `false`, emit `grpc-status` as a body trailer.
    trailers_only: bool,
}

impl InnerEcho {
    /// Non-empty body + a `grpc-status` **trailer frame** (normal-trailers path).
    pub fn with_trailer_status(grpc_status: i32) -> Self {
        Self {
            grpc_status,
            body_bytes: b"body",
            trailers_only: false,
        }
    }

    /// Empty body + `grpc-status` in **initial headers** (tonic Err(Status) path).
    pub fn trailers_only_with_status(grpc_status: i32) -> Self {
        Self {
            grpc_status,
            body_bytes: b"",
            trailers_only: true,
        }
    }
}

impl<B> tower::Service<http::Request<B>> for InnerEcho
where
    B: Send + 'static,
{
    type Response = http::Response<EchoBody>;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: http::Request<B>) -> Self::Future {
        let grpc_status = self.grpc_status;
        let body_bytes = self.body_bytes;
        let trailers_only = self.trailers_only;
        Box::pin(async move {
            if trailers_only {
                // Tonic `Err(Status)` trailers-only convention: grpc-status
                // in INITIAL headers, empty body, no trailer frame.
                let body = EchoBody {
                    data: None,
                    trailers: None,
                };
                let resp = http::Response::builder()
                    .status(200)
                    .header("content-type", "application/grpc")
                    .header("grpc-status", grpc_status.to_string())
                    .body(body)
                    .expect("InnerEcho trailers-only response");
                Ok(resp)
            } else {
                // Normal-trailers path: non-empty body frame + trailer frame
                // carrying grpc-status. No grpc-status header.
                let mut trailers = HeaderMap::new();
                trailers.insert("grpc-status", http::HeaderValue::from(grpc_status));
                let body = EchoBody {
                    data: Some(Bytes::copy_from_slice(body_bytes)),
                    trailers: Some(trailers),
                };
                let resp = http::Response::builder()
                    .status(200)
                    .header("content-type", "application/grpc")
                    .body(body)
                    .expect("InnerEcho trailer-status response");
                Ok(resp)
            }
        })
    }
}

// ── PassthroughInner — transparent Service that records call counts ───────────

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Transparent `tower::Service` that delegates to the inner service and counts
/// how many times `call` was invoked. Used by Task 6.1 to verify that
/// the audit or auth layer does (or does not) forward requests to the handler.
#[derive(Clone)]
pub struct PassthroughInner {
    /// Number of times `call` has been invoked.
    pub call_count: Arc<AtomicUsize>,
}

impl PassthroughInner {
    /// Create a new `PassthroughInner` with the counter at zero.
    pub fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Snapshot the call count at this instant.
    pub fn count(&self) -> usize {
        self.call_count.load(Ordering::Relaxed)
    }
}

impl Default for PassthroughInner {
    fn default() -> Self {
        Self::new()
    }
}

impl<B> tower::Service<http::Request<B>> for PassthroughInner
where
    B: Send + 'static,
{
    type Response = http::Response<Vec<u8>>;
    type Error = Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: http::Request<B>) -> Self::Future {
        self.call_count.fetch_add(1, Ordering::Relaxed);
        Box::pin(async move {
            Ok(http::Response::builder()
                .status(200)
                .body(vec![])
                .expect("PassthroughInner response"))
        })
    }
}

// ── Noop audit / server cert PEM ────────────────────────────────────────────
//
// #7730: shared test-binary helpers, de-duplicated from verbatim copies that
// previously lived in both `tests/external_grpc_integration.rs` and
// `tests/external_grpc_stress.rs`. NOTE: `in_memory_storage` (the third
// duplicated helper named in #7730) deliberately does NOT live here — it
// needs `maekon-storage`, an adapter crate, and `scripts/check-crate-
// boundaries.sh` forbids `maekon-web` from taking `maekon-storage` as a
// normal (even optional/feature-gated) dependency — only `dev-dependencies`
// are exempt (see that script's comment). It is instead shared via
// `tests/support/in_memory_storage.rs`, `#[path]`-included independently by
// both integration-test crate roots (mirrors the pre-existing
// `tests/support/failing_storage.rs` pattern).

use maekon_core::models::ai_session::SessionAuditEntry;
use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use maekon_core::ports::audit_log::AuditLogPort;

/// A no-op `AuditLogPort` impl used when tests don't need to inspect audit entries.
pub struct NoopAudit;

#[async_trait::async_trait]
impl AuditLogPort for NoopAudit {
    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _status: &AuditStatus, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _prefix: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        AuditStats::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn log_event(&self, _action_type: &str, _session_id: &str, _details: &str) {}
    async fn log_start_if(
        &self,
        _level: AuditLevel,
        _command_id: &str,
        _session_id: &str,
        _action_type: &str,
    ) {
    }
    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        _command_id: &str,
        _session_id: &str,
        _details: &str,
        _execution_time_ms: u64,
    ) {
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _entry: SessionAuditEntry) {}
}

/// Read the server cert PEM from the cached cert pair (see [`test_cert_pair`]).
pub fn server_cert_pem() -> Vec<u8> {
    let (cert_path, _) = test_cert_pair();
    std::fs::read(&cert_path).expect("read server cert PEM")
}

// ── CapturingAudit — richer capture than MockRecorder ──────────────────────────
//
// `MockRecorder` (below) captures the durable `AuditEntry` row shape used by
// `AuditBridge` unit tests (Tasks 0.6, 3.1) and only implements
// `log_complete_with_time`. `CapturingAudit` is a distinct, more capable
// double built for the `external_grpc_integration.rs` audit-family tests
// (Phase 9 / CR4): it separately tracks `log_start_if` (Started rows),
// `log_complete_with_time` (legacy Completed-only rows), AND
// `log_with_status_and_time` (#6277 — the real terminal status/action_type),
// and extracts `grpc_status_code` from the JSON details blob (D26 raw-code
// visibility) into the lighter `CapturedEntry` projection. Kept as two
// distinct doubles (#7730) rather than merged — their capture shapes and
// call-site contracts differ enough that unifying them would blur both.

/// A lightweight capture record used by Phase 9 integration tests to assert
/// on command_id, action_type, status, grpc_status_code, and execution timing.
///
/// `details` preserves the raw JSON blob from `log_complete_with_time` so that
/// tests can inspect operation names (e.g. "SubscribeEvents") without re-parsing
/// the struct.
#[derive(Clone, Debug)]
pub struct CapturedEntry {
    pub command_id: String,
    // #7719: no test in this file currently reads `action_type`/
    // `execution_time_ms` back off a captured entry (only `command_id`/
    // `status`/`grpc_status_code`/`details` are asserted on today) — kept for
    // the assertion helpers these fields were added for.
    #[allow(dead_code)]
    pub action_type: String,
    pub status: AuditStatus,
    pub grpc_status_code: Option<u32>,
    #[allow(dead_code)]
    pub execution_time_ms: u64,
    /// Raw `details` string passed by the audit bridge (JSON blob or empty).
    /// Populated by `log_complete_with_time` and `log_event`; `None` for
    /// entries that have no details context.
    pub details: Option<String>,
}

// Mock audit log that retains every `log_complete_with_time` and `log_event`
// entry so the e2e tests below can inspect what AuditLayer recorded.
//
// Structural rewrite (Task 9.0, CR4 / R2-NI1): replaces the previous
// action_type-as-command_id conflation with real command_id preservation and
// grpc_status_code JSON extraction from the details blob.  Unblocks Phase 9
// Tasks 9.1+ which assert command_id correlation and D26 raw-code visibility.
#[derive(Default)]
pub struct CapturingAudit {
    // `pub` (not private): several audit-family integration tests read
    // `capturing.entries.lock()` directly rather than going through
    // `snapshot()` — kept as a pure move from the pre-#7730 in-file struct.
    pub entries: std::sync::Mutex<Vec<CapturedEntry>>,
}

impl CapturingAudit {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: std::sync::Mutex::new(vec![]),
        })
    }

    /// Return a snapshot of all captured entries.  Used by Phase 9.1+ tests
    /// to assert command_id correlation and grpc_status_code visibility.
    // #7719: no test in this file calls it today — kept for the assertion
    // helpers it was added for.
    #[allow(dead_code)]
    pub fn snapshot(&self) -> Vec<CapturedEntry> {
        self.entries.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AuditLogPort for CapturingAudit {
    async fn log_event(&self, action_type: &str, _session_id: &str, details: &str) {
        // AuditBridge emits action_type "external_grpc_started" etc.
        // alongside log_complete_with_time; use this to capture Started rows.
        let status = match action_type {
            "external_grpc_started" => AuditStatus::Started,
            "external_grpc_completed" => AuditStatus::Completed,
            "external_grpc_failed" | "external_grpc_denied" | "external_grpc_timeout" => {
                AuditStatus::Failed
            }
            _ => AuditStatus::Completed,
        };
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: String::new(),
            action_type: action_type.to_string(),
            status,
            grpc_status_code: None,
            execution_time_ms: 0,
            details: Some(details.to_string()),
        });
    }

    async fn log_start_if(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        action_type: &str,
    ) {
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status: AuditStatus::Started,
            grpc_status_code: None,
            execution_time_ms: 0,
            details: None,
        });
    }

    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        let status = parse_status_from_details(details);
        let grpc_status_code: Option<u32> = serde_json::from_str::<serde_json::Value>(details)
            .ok()
            .and_then(|v| {
                v.get("grpc_status_code")
                    .and_then(|n| n.as_u64().map(|u| u as u32))
            });
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: String::new(),
            status,
            grpc_status_code,
            execution_time_ms,
            details: Some(details.to_string()),
        });
    }

    // #6277: AuditBridge now emits ONE durable row via log_with_status_and_time
    // carrying the REAL status + action_type (replacing log_complete_with_time +
    // a separate log_event). Capture the true status/action_type here so the mock
    // reflects the new contract — the trait default would delegate to
    // log_complete_with_time and lose the Started/Denied/Failed/Timeout status.
    async fn log_with_status_and_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: &str,
        execution_time_ms: u64,
    ) {
        let grpc_status_code: Option<u32> = serde_json::from_str::<serde_json::Value>(details)
            .ok()
            .and_then(|v| {
                v.get("grpc_status_code")
                    .and_then(|n| n.as_u64().map(|u| u as u32))
            });
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status,
            grpc_status_code,
            execution_time_ms,
            details: Some(details.to_string()),
        });
    }

    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        AuditStats::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _e: SessionAuditEntry) {}
}

/// Derive `AuditStatus` from the JSON `result` field in the details blob emitted
/// by `AuditBridge::record`.  Returns `Completed` for any unrecognized value.
fn parse_status_from_details(details: &str) -> AuditStatus {
    serde_json::from_str::<serde_json::Value>(details)
        .ok()
        .and_then(|v| v.get("result").and_then(|r| r.as_str().map(String::from)))
        .map(|s| match s.as_str() {
            "ok" => AuditStatus::Completed,
            "denied" => AuditStatus::Denied,
            "timeout" => AuditStatus::Timeout,
            "error" | "failed" | "auth_failed" => AuditStatus::Failed,
            _ => AuditStatus::Completed,
        })
        .unwrap_or(AuditStatus::Completed)
}

// ── fixture_bridge / fixture_metrics ─────────────────────────────────────────

use super::audit_bridge::AuditBridge;
use super::metrics::ExternalMetrics;

/// Capturing `AuditLogPort` impl used by [`fixture_bridge`].
///
/// Construct via [`fixture_bridge`]`() -> (AuditBridge, Arc<MockRecorder>)` — the
/// struct's `new()` constructor is crate-private to enforce this factory pattern.
/// Tests read captured entries via [`MockRecorder::snapshot`].
///
/// Used by `fixture_bridge` so that Tasks 0.6 and 3.1 can assert on the
/// exact audit entries emitted by `AuditBridge::record` /
/// `AuditBridge::record_completion`.
pub struct MockRecorder {
    /// All entries captured by `log_complete_with_time`.
    pub entries: std::sync::Mutex<Vec<AuditEntry>>,
}

impl MockRecorder {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: std::sync::Mutex::new(vec![]),
        })
    }

    /// Snapshot entries without holding the lock past the call.
    pub fn snapshot(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AuditLogPort for MockRecorder {
    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        AuditStats::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn log_event(&self, _a: &str, _s: &str, _d: &str) {}
    async fn log_start_if(&self, _l: AuditLevel, _c: &str, _s: &str, _a: &str) {}
    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        use ulid::Ulid;
        let status = serde_json::from_str::<serde_json::Value>(details)
            .ok()
            .and_then(|v| {
                v.get("result").and_then(|r| r.as_str()).map(|r| match r {
                    "ok" => AuditStatus::Completed,
                    "denied" => AuditStatus::Denied,
                    "timeout" => AuditStatus::Timeout,
                    _ => AuditStatus::Failed,
                })
            })
            .unwrap_or(AuditStatus::Completed);
        self.entries.lock().unwrap().push(AuditEntry {
            entry_id: Ulid::new().to_string(),
            timestamp: chrono::Utc::now(),
            session_id: session_id.into(),
            command_id: command_id.into(),
            action_type: "external_grpc".into(),
            status,
            details: Some(details.into()),
            execution_time_ms: Some(execution_time_ms),
        });
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _e: SessionAuditEntry) {}
}

/// Build an `(AuditBridge, Arc<MockRecorder>)` pair ready for unit tests.
///
/// The `MockRecorder` captures every `log_complete_with_time` call; the
/// `AuditBridge` delegates to it. Used by Tasks 0.6 and 3.1.
pub fn fixture_bridge() -> (AuditBridge, Arc<MockRecorder>) {
    let recorder = MockRecorder::new();
    let bridge = AuditBridge::new(recorder.clone() as Arc<dyn AuditLogPort>);
    (bridge, recorder)
}

/// Build a fresh `Arc<ExternalMetrics>` pre-zeroed. Used by Tasks 0.6 and 3.1.
pub fn fixture_metrics() -> Arc<ExternalMetrics> {
    Arc::new(ExternalMetrics::new())
}

// ── connect_loopback / req_with_valid_auth ────────────────────────────────────
// Task 9.4 G3 test dependencies — created in Task 0.0 per plan §Step 2.

/// Plaintext loopback gRPC channel connector for tests.
///
/// Returns a connected `tonic::transport::Channel` pointing at
/// `http://127.0.0.1:{port}`. Use only for in-process integration tests
/// where the server is also bound to localhost without TLS.
///
/// **Caller contract**: call only after the server has successfully bound to `port`.
/// A typical pattern is:
///   1. `tokio::spawn(async move { serve_external(...).await });`
///   2. Wait for a bind-confirmation signal (or `tokio::time::sleep(Duration::from_millis(50))`)
///   3. `let ch = connect_loopback(port).await;`
///
/// Panics with "connect to loopback server" if the server is not yet listening — the
/// panic message does not include port or timing context; add your own retry/wait if
/// flakiness appears in CI.
pub async fn connect_loopback(port: u16) -> tonic::transport::Channel {
    let addr = format!("http://127.0.0.1:{port}");
    tonic::transport::Channel::from_shared(addr)
        .expect("valid loopback URI")
        .connect()
        .await
        .expect("connect to loopback server")
}

// ── spawn_server_with_config_manager ─────────────────────────────────────────
// Task 9.4: variant of `spawn_server` that accepts a pre-built
// `Arc<ConfigManager>` in addition to the normal `ExternalGrpcSpawnConfig`.
// Spawns BOTH the tonic server AND the `ConfigReloadTask` so live-config
// mutations made via `cfg_mgr.update_with(..)` propagate to the server's
// `LiveExternalConfig` within one `watch::Receiver::changed()` round-trip.
//
// Why here (vs. the integration test file): the `ConfigReloadTask` wiring
// mirrors production (`app_runtime_launch::build_external_spawn_config`
// L1344-1358 in src-tauri). Hosting the wiring in `test_support.rs` keeps
// the integration test free of reload-task plumbing and lets future tests
// reuse the helper without duplicating the `tokio::spawn(run_config_reload(..))`
// boilerplate.
//
// The caller is responsible for:
//   - building `cfg.bind_addr` with a pre-allocated ephemeral port
//   - seeding any initial `cfg.live` snapshot (this helper will NOT rewrite it
//     — it only hooks up the reload task to observe future changes)
//   - dropping `cfg.shutdown_tx` / `handles.abort()` to tear everything down
//
// Returns `(server_handle, reload_task_handle, port)`. The two handles let
// Task 9.4 Test 6 assert that the reload task joins within 5 seconds of
// shutdown, independently of the server's own shutdown.

/// Spawn `serve_external` together with a `ConfigReloadTask` bound to the
/// provided `ConfigManager`. Mirrors the production wiring in
/// `src-tauri/src/app_runtime_launch.rs::build_external_spawn_config` (where
/// both tasks share the same `shutdown_rx` and the reload task observes
/// `cfg_mgr.subscribe()` for `AppConfig` changes).
///
/// The server and reload task run independently; the caller must either
/// `abort()` both handles or send `true` on `cfg.shutdown_tx` to stop them.
///
/// **Panics** if `cfg.bind_addr.port() == 0`: this helper does NOT allocate
/// a port for the caller — use `next_test_port()` (integration test) or bind
/// a throwaway `std::net::TcpListener` to pick one before calling.
pub async fn spawn_server_with_config_manager(
    cfg: super::spawn_config::ExternalGrpcSpawnConfig,
    cfg_mgr: std::sync::Arc<maekon_core::config_manager::ConfigManager>,
) -> (
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
    u16,
) {
    use std::time::Duration;

    install_rustls_crypto_provider();

    let port = cfg.bind_addr.port();
    assert!(
        port != 0,
        "spawn_server_with_config_manager requires a pre-allocated port; \
         got bind_addr {}",
        cfg.bind_addr
    );

    // Spawn ConfigReloadTask mirroring production (D30 — fire-and-forget
    // outside `serve_external` to avoid supervisor-respawn double-task hazard).
    // Clone the shutdown_rx so both tasks observe the same shutdown signal.
    let live_for_reload = cfg.live.clone();
    let metrics_for_reload = cfg.metrics.clone();
    let shutdown_rx_for_reload = cfg.shutdown_rx.clone();
    let config_rx = cfg_mgr.subscribe();
    let reload_handle = tokio::spawn(async move {
        super::config_reload::run_config_reload(
            live_for_reload,
            metrics_for_reload,
            config_rx,
            shutdown_rx_for_reload,
        )
        .await;
    });

    // Spawn the tonic server — same pattern as `spawn_server` in the
    // integration test. Errors are printed to stderr to surface flakes
    // without tripping panic-propagation across tests.
    let server_handle = tokio::spawn(async move {
        match super::serve_external(cfg).await {
            Ok(()) => {}
            Err(e) => eprintln!("serve_external error: {e:?}"),
        }
    });

    // Wait for the TCP listener to come up (mirrors `spawn_server`).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("external gRPC server did not start on port {port} within 5s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    (server_handle, reload_handle, port)
}

/// Build a test gRPC request with a placeholder bearer token header.
///
/// Inserts `Authorization: Bearer TEST_TOKEN_PLACEHOLDER` into the request
/// metadata.
///
/// **IMPORTANT**: The current token is a literal `"TEST_TOKEN_PLACEHOLDER"` that will
/// fail validation against any real `JwtVerifier`. This helper exists to let Task 9.4
/// G3-test scaffolding compile; the actual JWT minting belongs to Task 9.4 itself via
/// [`test_mint_jwt`] (present in `tests/external_grpc_integration.rs`).
///
/// Callers: **replace the placeholder with a real minted token** before running the
/// test against a configured server, OR use this only with a bypass-auth server setup.
pub fn req_with_valid_auth<T>(body: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(body);
    req.metadata_mut().insert(
        "authorization",
        tonic::metadata::MetadataValue::from_static("Bearer TEST_TOKEN_PLACEHOLDER"),
    );
    req
}

// ── serve_external_with_service ───────────────────────────────────────────────
// Task 9.2: variant of `serve_external` that accepts an injected
// `DashboardService` impl in place of the production `DashboardServiceImpl`.
//
// Tests need this so a fixture handler can return canned `tonic::Status`
// errors (PermissionDenied, Cancelled, Internal) and exercise the
// AuditLayer's `map_code_to_audit_status` mapping end-to-end through the
// real layer stack (request_id → auth → audit) — which is `pub(crate)` and
// not constructible from an integration-test crate. Mirrors the body of
// `serve_external` (mod.rs L112-L213) verbatim except for the service type.
//
// The fixture service replaces `DashboardServiceImpl::from_external_spawn_config`
// — `cfg.config.integration_auth_token` and other DashboardServiceImpl-specific
// state are NOT applied (the fixture has its own state).

/// Spawn the external gRPC server with an injected `DashboardService` impl.
///
/// Identical to [`super::serve_external`] but uses `service` in place of the
/// real `DashboardServiceImpl`. Layers (request_id → auth → audit), TLS,
/// accept loop, and shutdown handling are bit-for-bit the same — both this
/// helper and `serve_external` delegate to the shared
/// `super::serve_external_inner` core, so test paths stay in lock-step with
/// production lint/behavior coverage.
///
/// Errors mirror [`super::ServeExternalError`] semantics (bind, tls, tonic).
/// Test-only — gated on `#[cfg(any(test, feature = "test-support"))]` via
/// the surrounding module.
pub async fn serve_external_with_service<T>(
    cfg: super::spawn_config::ExternalGrpcSpawnConfig,
    service: T,
) -> Result<(), super::ServeExternalError>
where
    T: crate::proto::dashboard::v1::dashboard_service_server::DashboardService,
{
    super::serve_external_inner(cfg, service, "external_grpc(test)").await
}
