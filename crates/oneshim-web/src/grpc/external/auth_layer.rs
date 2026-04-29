//! tower::Layer verifying JWT header and/or mTLS cert. On success it
//! inserts AuthContext into the request extensions and passes through
//! to the downstream service. On failure: uniform Status::unauthenticated
//! (detail is in audit log only — no oracle signals to the client).

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use oneshim_core::models::audit::AuditStatus;
use tonic::Status;
use tower::{Layer, Service};
use ulid::Ulid;

use oneshim_core::config::AuthMode;

use super::audit_bridge::AuditBridge;
use super::conn_info::{AuthContext, AuthType, PeerInfo};
use super::ip_ban::IpBan;
use super::jwt_verifier::JwtVerifier;
use super::metrics::ExternalMetrics;
use super::mtls_verifier::MtlsVerifier;

#[derive(Clone)]
pub(crate) struct AuthLayer {
    pub auth_mode: AuthMode,
    pub jwt_verifier: Option<Arc<JwtVerifier>>,
    pub mtls_verifier: Option<Arc<MtlsVerifier>>,
    pub ip_ban: Arc<IpBan>,
    pub metrics: Arc<ExternalMetrics>,
    pub audit_bridge: Arc<AuditBridge>,
}

impl<S: Clone> Layer<S> for AuthLayer {
    type Service = AuthService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            inner,
            auth_mode: self.auth_mode,
            jwt_verifier: self.jwt_verifier.clone(),
            mtls_verifier: self.mtls_verifier.clone(),
            ip_ban: self.ip_ban.clone(),
            metrics: self.metrics.clone(),
            audit_bridge: self.audit_bridge.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct AuthService<S> {
    inner: S,
    auth_mode: AuthMode,
    jwt_verifier: Option<Arc<JwtVerifier>>,
    mtls_verifier: Option<Arc<MtlsVerifier>>,
    ip_ban: Arc<IpBan>,
    metrics: Arc<ExternalMetrics>,
    audit_bridge: Arc<AuditBridge>,
}

impl<S, B, RespBody> Service<http::Request<B>> for AuthService<S>
where
    S: Service<
            http::Request<B>,
            Response = http::Response<RespBody>,
            Error = std::convert::Infallible,
        > + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    B: Send + 'static,
    RespBody: Default + Send + 'static,
{
    type Response = http::Response<RespBody>;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: http::Request<B>) -> Self::Future {
        let jwt_verifier = self.jwt_verifier.clone();
        let mtls_verifier = self.mtls_verifier.clone();
        let auth_mode = self.auth_mode;
        let ip_ban = self.ip_ban.clone();
        let metrics = self.metrics.clone();
        let audit_bridge = self.audit_bridge.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let peer: Option<PeerInfo> = req.extensions().get::<PeerInfo>().cloned();
            let peer = match peer {
                Some(p) => p,
                None => {
                    return Ok(status_response(Status::unauthenticated("unauthenticated")));
                }
            };

            // Read RequestId extension once; moved into each spawn that needs it.
            let request_id: Option<String> = req
                .extensions()
                .get::<super::request_id_layer::RequestId>()
                .map(|r| r.0.clone());

            let mut client_id: Option<String> = None;
            let mut jti: Option<String> = None;

            // JWT gate
            if auth_mode.includes_jwt() {
                let token = req
                    .headers()
                    .get("authorization")
                    .and_then(|h| h.to_str().ok())
                    .and_then(|s| s.strip_prefix("Bearer "));
                match (token, &jwt_verifier) {
                    (Some(tok), Some(v)) => match v.verify(tok) {
                        Ok(claims) => {
                            client_id = Some(claims.sub.clone());
                            jti = claims.jti.clone();
                        }
                        Err(_) => {
                            ip_ban.record_failure(peer.remote_addr);
                            metrics.auth_failure_bump("invalid_jwt");
                            // Build a stub context for the audit record (auth failed, no real client_id).
                            let stub_ctx = AuthContext {
                                auth_type: AuthType::Jwt,
                                client_id: "unknown".into(),
                                jti: None,
                                command_id: Ulid::new().to_string(),
                            };
                            let bridge = audit_bridge.clone();
                            let remote = peer.remote_addr.to_string();
                            let cmd_id = request_id.clone();
                            tokio::spawn(async move {
                                bridge
                                    .record(
                                        &stub_ctx,
                                        remote,
                                        "external_grpc",
                                        "auth_failed",
                                        AuditStatus::Failed,
                                        Duration::ZERO,
                                        None,
                                        None,
                                        Some("invalid_jwt"),
                                        cmd_id,
                                    )
                                    .await;
                            });
                            return Ok(status_response(Status::unauthenticated("unauthenticated")));
                        }
                    },
                    _ => {
                        ip_ban.record_failure(peer.remote_addr);
                        metrics.auth_failure_bump("missing_token");
                        let stub_ctx = AuthContext {
                            auth_type: AuthType::Jwt,
                            client_id: "unknown".into(),
                            jti: None,
                            command_id: Ulid::new().to_string(),
                        };
                        let bridge = audit_bridge.clone();
                        let remote = peer.remote_addr.to_string();
                        let cmd_id = request_id.clone();
                        tokio::spawn(async move {
                            bridge
                                .record(
                                    &stub_ctx,
                                    remote,
                                    "external_grpc",
                                    "auth_failed",
                                    AuditStatus::Failed,
                                    Duration::ZERO,
                                    None,
                                    None,
                                    Some("missing_token"),
                                    cmd_id,
                                )
                                .await;
                        });
                        return Ok(status_response(Status::unauthenticated("unauthenticated")));
                    }
                }
            }

            // mTLS gate (cert already CA-validated at TLS layer; verifier enforces lifetime + allowlist)
            if auth_mode.includes_mtls() {
                let (cert_der, mtls_v) = (peer.peer_cert_der.as_ref(), mtls_verifier.as_ref());
                match (cert_der, mtls_v) {
                    (Some(der), Some(v)) => match v.verify(der) {
                        Ok(verified) => {
                            // If JWT did not provide client_id, use CN; else prefer JWT sub.
                            if client_id.is_none() {
                                client_id = Some(verified.subject_cn.clone());
                            }
                        }
                        Err(_) => {
                            ip_ban.record_failure(peer.remote_addr);
                            metrics.auth_failure_bump("fingerprint_mismatch");
                            let stub_ctx = AuthContext {
                                auth_type: AuthType::Mtls,
                                client_id: "unknown".into(),
                                jti: None,
                                command_id: Ulid::new().to_string(),
                            };
                            let bridge = audit_bridge.clone();
                            let remote = peer.remote_addr.to_string();
                            let cmd_id = request_id.clone();
                            tokio::spawn(async move {
                                bridge
                                    .record(
                                        &stub_ctx,
                                        remote,
                                        "external_grpc",
                                        "auth_failed",
                                        AuditStatus::Failed,
                                        Duration::ZERO,
                                        None,
                                        None,
                                        Some("fingerprint_mismatch"),
                                        cmd_id,
                                    )
                                    .await;
                            });
                            return Ok(status_response(Status::unauthenticated("unauthenticated")));
                        }
                    },
                    _ => {
                        ip_ban.record_failure(peer.remote_addr);
                        metrics.auth_failure_bump("missing_cert");
                        let stub_ctx = AuthContext {
                            auth_type: AuthType::Mtls,
                            client_id: "unknown".into(),
                            jti: None,
                            command_id: Ulid::new().to_string(),
                        };
                        let bridge = audit_bridge.clone();
                        let remote = peer.remote_addr.to_string();
                        let cmd_id = request_id.clone();
                        tokio::spawn(async move {
                            bridge
                                .record(
                                    &stub_ctx,
                                    remote,
                                    "external_grpc",
                                    "auth_failed",
                                    AuditStatus::Failed,
                                    Duration::ZERO,
                                    None,
                                    None,
                                    Some("missing_cert"),
                                    cmd_id,
                                )
                                .await;
                        });
                        return Ok(status_response(Status::unauthenticated("unauthenticated")));
                    }
                }
            }

            let auth_type = auth_mode_to_type(auth_mode);
            let ctx = AuthContext {
                auth_type,
                client_id: client_id.unwrap_or_else(|| "unknown".into()),
                jti,
                command_id: Ulid::new().to_string(),
            };

            // AuditLayer now owns Started + Completed recording (Task 13 spec §2.2).
            // AuthLayer still records `Failed` on auth rejection (the 4 spawn
            // blocks above); the success path simply forwards to the inner
            // service after inserting AuthContext.
            metrics.request_bump("external", auth_type.as_str(), "ok");

            req.extensions_mut().insert(ctx);
            inner.call(req).await
        })
    }
}

fn auth_mode_to_type(mode: AuthMode) -> AuthType {
    match mode {
        AuthMode::Jwt => AuthType::Jwt,
        AuthMode::Mtls => AuthType::Mtls,
        AuthMode::JwtAndMtls => AuthType::JwtAndMtls,
    }
}

/// Convert a tonic `Status` into an `http::Response<B>`.
///
/// tonic 0.14 exposes `Status::into_http::<B>()` (owned, consumes `self`) which
/// encodes grpc-status + grpc-message into the response trailers — the correct
/// wire format for a gRPC error response over HTTP/2.
fn status_response<B: Default>(status: Status) -> http::Response<B> {
    status.into_http::<B>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::{Request, Response};
    use std::convert::Infallible;
    use tower::{service_fn, ServiceExt};

    use crate::grpc::external::conn_info::PeerInfo;

    fn echo_service() -> impl Service<
        Request<Vec<u8>>,
        Response = Response<Vec<u8>>,
        Error = Infallible,
        Future = impl std::future::Future<Output = Result<Response<Vec<u8>>, Infallible>> + Send,
    > + Clone {
        service_fn(|req: Request<Vec<u8>>| async move {
            // Echo whether AuthContext was injected into extensions.
            let has_ctx = req.extensions().get::<AuthContext>().is_some();
            let body = if has_ctx {
                b"ok".to_vec()
            } else {
                b"no-ctx".to_vec()
            };
            Ok::<_, Infallible>(Response::new(body))
        })
    }

    fn mk_peer(peer_cert: Option<Vec<u8>>) -> PeerInfo {
        PeerInfo {
            remote_addr: "127.0.0.1:5001".parse().unwrap(),
            peer_cert_der: peer_cert,
            cert_subject_cn: None,
            tls_version: "TLSv1.3".into(),
        }
    }

    /// Build a no-op `AuditBridge` backed by a minimal `AuditLogPort` stub.
    fn noop_audit_bridge() -> Arc<AuditBridge> {
        use async_trait::async_trait;
        use oneshim_core::models::ai_session::SessionAuditEntry;
        use oneshim_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
        use oneshim_core::ports::audit_log::AuditLogPort;

        struct NoopAudit;
        #[async_trait]
        impl AuditLogPort for NoopAudit {
            async fn pending_count(&self) -> usize {
                0
            }
            async fn recent_entries(&self, _: usize) -> Vec<AuditEntry> {
                vec![]
            }
            async fn entries_by_status(&self, _: &AuditStatus, _: usize) -> Vec<AuditEntry> {
                vec![]
            }
            async fn entries_by_action_prefix(&self, _: &str, _: usize) -> Vec<AuditEntry> {
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
            async fn log_event(&self, _: &str, _: &str, _: &str) {}
            async fn log_start_if(&self, _: AuditLevel, _: &str, _: &str, _: &str) {}
            async fn log_complete_with_time(
                &self,
                _: AuditLevel,
                _: &str,
                _: &str,
                _: &str,
                _: u64,
            ) {
            }
            async fn drain_batch(&self) -> Vec<AuditEntry> {
                vec![]
            }
            async fn drain_all(&self) -> Vec<AuditEntry> {
                vec![]
            }
            async fn record_session_event(&self, _: SessionAuditEntry) {}
        }
        Arc::new(AuditBridge::new(Arc::new(NoopAudit)))
    }

    #[tokio::test]
    async fn missing_peer_info_returns_unauthenticated() {
        let layer = AuthLayer {
            auth_mode: AuthMode::Jwt,
            jwt_verifier: None,
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: Arc::new(ExternalMetrics::new()),
            audit_bridge: noop_audit_bridge(),
        };
        let mut svc = layer.layer(echo_service());
        // Request with NO PeerInfo extension inserted.
        let req = Request::builder().uri("/").body(vec![]).unwrap();
        let resp: Response<Vec<u8>> = svc.ready().await.unwrap().call(req).await.unwrap();
        // tonic grpc-status "16" = UNAUTHENTICATED (uniform auth error path, no oracle via status code)
        let grpc_status = resp
            .headers()
            .get("grpc-status")
            .map(|v| v.to_str().unwrap().to_string());
        assert_eq!(grpc_status.as_deref(), Some("16"), "UNAUTHENTICATED = 16");
    }

    #[tokio::test]
    async fn jwt_valid_builds_context_with_sub_as_client_id() {
        use crate::grpc::external::jwt_verifier::tests::rsa_keypair_pem;
        use oneshim_core::config::JwtAlgorithm;

        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let verifier =
            Arc::new(JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "iss-1", "aud-1").unwrap());
        let enc = jsonwebtoken::EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = serde_json::json!({
            "sub": "user-A",
            "iss": "iss-1",
            "aud": "aud-1",
            "exp": now + 3600,
            "iat": now,
        });
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &claims,
            &enc,
        )
        .unwrap();

        let layer = AuthLayer {
            auth_mode: AuthMode::Jwt,
            jwt_verifier: Some(verifier),
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: Arc::new(ExternalMetrics::new()),
            audit_bridge: noop_audit_bridge(),
        };
        let mut svc = layer.layer(echo_service());
        let mut req = Request::builder().uri("/").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_peer(None));
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let resp = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(resp.body(), b"ok", "inner service saw AuthContext");
    }

    #[tokio::test]
    async fn jwt_invalid_returns_unauthenticated_and_records_ban() {
        use crate::grpc::external::jwt_verifier::tests::rsa_keypair_pem;
        use oneshim_core::config::JwtAlgorithm;

        let (_, pub_pem) = rsa_keypair_pem();
        let verifier =
            Arc::new(JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "iss-1", "aud-1").unwrap());
        let ban = Arc::new(IpBan::new());
        let layer = AuthLayer {
            auth_mode: AuthMode::Jwt,
            jwt_verifier: Some(verifier),
            mtls_verifier: None,
            ip_ban: ban.clone(),
            metrics: Arc::new(ExternalMetrics::new()),
            audit_bridge: noop_audit_bridge(),
        };
        let mut svc = layer.layer(echo_service());
        let mut req = Request::builder().uri("/").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_peer(None));
        req.headers_mut()
            .insert("authorization", "Bearer not.a.valid.token".parse().unwrap());
        let resp = svc.ready().await.unwrap().call(req).await.unwrap();
        let grpc_status = resp
            .headers()
            .get("grpc-status")
            .and_then(|v| v.to_str().ok());
        assert_eq!(grpc_status, Some("16"), "UNAUTHENTICATED = 16");
        // One failure is not enough to trigger a ban (threshold is 5).
        assert_eq!(ban.active_ban_count(), 0, "one failure not enough for ban");
    }

    #[tokio::test]
    async fn mtls_valid_falls_back_to_cn_when_jwt_absent() {
        use crate::grpc::external::mtls_verifier::tests::gen_cert_with_cn;

        let der = gen_cert_with_cn("client-X", 24);
        let mtls = Arc::new(MtlsVerifier::new(48, &[]).unwrap());
        let layer = AuthLayer {
            auth_mode: AuthMode::Mtls,
            jwt_verifier: None,
            mtls_verifier: Some(mtls),
            ip_ban: Arc::new(IpBan::new()),
            metrics: Arc::new(ExternalMetrics::new()),
            audit_bridge: noop_audit_bridge(),
        };
        let mut svc = layer.layer(echo_service());
        let mut req = Request::builder().uri("/").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_peer(Some(der)));
        let resp = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(
            resp.body(),
            b"ok",
            "mTLS-only path accepted and AuthContext inserted"
        );
    }

    /// Auth failure bumps `metrics.auth_failures_total["invalid_jwt"]`.
    #[tokio::test]
    async fn jwt_invalid_bumps_auth_failure_metric() {
        use crate::grpc::external::jwt_verifier::tests::rsa_keypair_pem;
        use oneshim_core::config::JwtAlgorithm;

        let (_, pub_pem) = rsa_keypair_pem();
        let verifier =
            Arc::new(JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "iss-1", "aud-1").unwrap());
        let metrics = Arc::new(ExternalMetrics::new());
        let layer = AuthLayer {
            auth_mode: AuthMode::Jwt,
            jwt_verifier: Some(verifier),
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: metrics.clone(),
            audit_bridge: noop_audit_bridge(),
        };
        let mut svc = layer.layer(echo_service());
        let mut req = Request::builder().uri("/").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_peer(None));
        req.headers_mut()
            .insert("authorization", "Bearer bad.token".parse().unwrap());
        let _ = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(
            metrics.get_auth_failure_count("invalid_jwt"),
            1,
            "invalid JWT must bump auth_failure_count[invalid_jwt]"
        );
    }

    /// Successful JWT auth bumps `metrics.requests_total["external|jwt|ok"]`.
    #[tokio::test]
    async fn jwt_valid_bumps_request_metric() {
        use crate::grpc::external::jwt_verifier::tests::rsa_keypair_pem;
        use oneshim_core::config::JwtAlgorithm;

        let (priv_pem, pub_pem) = rsa_keypair_pem();
        let verifier =
            Arc::new(JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "iss-1", "aud-1").unwrap());
        let enc = jsonwebtoken::EncodingKey::from_rsa_pem(&priv_pem).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = serde_json::json!({
            "sub": "user-B",
            "iss": "iss-1",
            "aud": "aud-1",
            "exp": now + 3600,
            "iat": now,
        });
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
            &claims,
            &enc,
        )
        .unwrap();

        let metrics = Arc::new(ExternalMetrics::new());
        let layer = AuthLayer {
            auth_mode: AuthMode::Jwt,
            jwt_verifier: Some(verifier),
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: metrics.clone(),
            audit_bridge: noop_audit_bridge(),
        };
        let mut svc = layer.layer(echo_service());
        let mut req = Request::builder().uri("/").body(vec![]).unwrap();
        req.extensions_mut().insert(mk_peer(None));
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        let _ = svc.ready().await.unwrap().call(req).await.unwrap();
        assert_eq!(
            metrics.get_request_count("external|jwt|ok"),
            1,
            "successful JWT auth must bump requests_total[external|jwt|ok]"
        );
    }

    /// Failed audit rows carry the client's x-request-id as command_id.
    ///
    /// Spec §5.2 / U5 / D14 revised: RequestIdLayer inserts the extension before
    /// AuthLayer runs. Auth-rejected audit entries must include the client's
    /// x-request-id so security dashboards can correlate rejections to the
    /// originating client call.
    #[tokio::test]
    async fn failed_audit_reads_request_id_from_extensions() {
        use crate::grpc::external::jwt_verifier::tests::rsa_keypair_pem;
        use crate::grpc::external::request_id_layer::RequestId;
        use crate::grpc::external::test_support::{fixture_bridge, fixture_metrics};
        use oneshim_core::config::JwtAlgorithm;
        use oneshim_core::models::audit::AuditStatus;

        // Build a verifier with a fresh key pair; we will send a token signed
        // by a *different* key so verify() fails → invalid_jwt → AuditStatus::Failed.
        let (_, pub_pem) = rsa_keypair_pem();
        let verifier =
            Arc::new(JwtVerifier::new(JwtAlgorithm::Rs256, &pub_pem, "iss-1", "aud-1").unwrap());

        let (bridge, recorder) = fixture_bridge();
        let layer = AuthLayer {
            auth_mode: AuthMode::Jwt,
            jwt_verifier: Some(verifier),
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: fixture_metrics(),
            audit_bridge: Arc::new(bridge),
        };
        let mut svc = layer.layer(echo_service());

        let mut req = Request::builder()
            .uri("/Service/Method")
            .body(vec![])
            .unwrap();
        // Manually insert RequestId extension (in production, RequestIdLayer does this).
        req.extensions_mut()
            .insert(RequestId("req-auth-fail".into()));
        req.extensions_mut().insert(mk_peer(None));
        // Invalid token → JwtVerifier::verify fails → invalid_jwt failure path.
        req.headers_mut()
            .insert("authorization", "Bearer invalid-token".parse().unwrap());

        let _ = svc.ready().await.unwrap().call(req).await.unwrap();

        // Give the spawned audit record time to complete.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let entries = recorder.snapshot();
        let failed = entries
            .iter()
            .find(|e| e.status == AuditStatus::Failed)
            .expect("auth-rejected audit row must be present");
        assert_eq!(
            failed.command_id, "req-auth-fail",
            "auth-rejected audit row must carry client's x-request-id"
        );
    }
}
