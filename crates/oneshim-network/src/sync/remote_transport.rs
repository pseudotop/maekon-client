//! RemoteSyncTransport -- REST-based cloud sync relay.
//!
//! Pushes/pulls AES-256-GCM encrypted ChangeSet payloads to a configurable
//! REST endpoint. Authentication via Bearer token or API key.

use async_trait::async_trait;
use std::time::Duration;
use tracing::{debug, warn};

use oneshim_core::config::RemoteSyncAuth;
use oneshim_core::error::CoreError;
use oneshim_core::models::sync::{ChangeSet, PeerInfo};
use oneshim_core::ports::sync_transport::SyncTransport;
use oneshim_core::sync::Hlc;

use super::sync_crypto;
use crate::resilience::{extract_retry_after, jittered_backoff_delay};

const MAX_RETRIES: u32 = 3;
const REQUEST_TIMEOUT_SECS: u64 = 30;

/// Remote sync transport -- push/pull changesets via REST to a cloud endpoint.
pub struct RemoteSyncTransport {
    client: reqwest::Client,
    endpoint: String,
    local_device_id: String,
    passphrase: String,
    auth_mode: RemoteSyncAuth,
    /// Credential value: Bearer token or API key (retrieved from OS keychain).
    auth_credential: String,
    max_retries: u32,
    timeout_ms: u64,
}

impl RemoteSyncTransport {
    pub fn new(
        endpoint: String,
        local_device_id: String,
        passphrase: String,
        auth_mode: RemoteSyncAuth,
        auth_credential: String,
    ) -> Result<Self, CoreError> {
        let timeout = Duration::from_secs(REQUEST_TIMEOUT_SECS);
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("Failed to build HTTP client: {e}"),
            })?;

        Ok(Self {
            client,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            local_device_id,
            passphrase,
            auth_mode,
            auth_credential,
            max_retries: MAX_RETRIES,
            timeout_ms: timeout.as_millis() as u64,
        })
    }

    fn auth_header(&self) -> (&str, String) {
        match self.auth_mode {
            RemoteSyncAuth::BearerToken => {
                ("Authorization", format!("Bearer {}", self.auth_credential))
            }
            RemoteSyncAuth::ApiKey => ("X-Api-Key", self.auth_credential.clone()),
        }
    }

    fn map_error(&self, e: reqwest::Error, context: &str) -> CoreError {
        if e.is_timeout() {
            CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout_ms,
            }
        } else {
            CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("{context}: {e}"),
            }
        }
    }

    fn check_response_status(status: reqwest::StatusCode, body: &str) -> Result<(), CoreError> {
        match status.as_u16() {
            200 | 204 => Ok(()),
            401 | 403 => Err(CoreError::Auth {
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: format!("Sync auth failed: {body}"),
            }),
            404 => Err(CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "SyncEndpoint".to_string(),
                id: body.to_string(),
            }),
            409 => {
                // Conflict -- stale watermark; SyncEngine will re-pull
                debug!("sync push conflict (409), will re-pull");
                Ok(())
            }
            // 408/504 are timeout-class — wire code `network.timeout` (iter-55)
            408 | 504 => Err(CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: 0, // sentinel: server-side timeout, unknown budget
            }),
            429 => {
                let retry_secs = 60u64; // Default; actual parsing in retry loop
                Err(CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs: retry_secs,
                })
            }
            // 502 Bad Gateway is a transient upstream failure (iter-55)
            502 | 503 => Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: body.to_string(),
            }),
            _ => Err(CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("Sync API error ({status}): {body}"),
            }),
        }
    }

    fn is_retryable(error: &CoreError) -> bool {
        matches!(
            error,
            CoreError::Network { .. }
                | CoreError::RequestTimeout {
                    code: oneshim_core::error_codes::NetworkCode::Timeout,
                    ..
                }
                | CoreError::ServiceUnavailable { .. }
                | CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    ..
                }
        )
    }
}

#[async_trait]
impl SyncTransport for RemoteSyncTransport {
    async fn push(&self, changes: &ChangeSet) -> Result<(), CoreError> {
        let json = serde_json::to_vec(changes).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("serialize changeset: {e}"),
        })?;
        let encrypted = sync_crypto::encrypt(&self.passphrase, &json)?;
        let (header_name, header_value) = self.auth_header();

        let mut last_error = CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "push failed".to_string(),
        };
        for attempt in 0..=self.max_retries {
            let result = self
                .client
                .post(format!("{}/sync/push", self.endpoint))
                .header(header_name, &header_value)
                .header("Content-Type", "application/octet-stream")
                .body(encrypted.clone())
                .send()
                .await;

            match result {
                Ok(resp) => {
                    let status = resp.status();
                    let retry_after = extract_retry_after(&resp);
                    let body = resp.text().await.unwrap_or_default();
                    match status.as_u16() {
                        200 | 204 => {
                            debug!(bytes = encrypted.len(), "remote push succeeded");
                            return Ok(());
                        }
                        409 => {
                            debug!("remote push conflict (409), re-pull needed");
                            return Ok(()); // SyncEngine handles re-pull
                        }
                        429 => {
                            last_error = CoreError::RateLimit {
                                code: oneshim_core::error_codes::NetworkCode::RateLimit,
                                retry_after_secs: retry_after,
                            };
                        }
                        _ => {
                            let err = Self::check_response_status(status, &body);
                            if let Err(e) = err {
                                last_error = e;
                            }
                        }
                    }
                }
                Err(e) => {
                    last_error = self.map_error(e, "remote push");
                }
            }

            if !Self::is_retryable(&last_error) || attempt == self.max_retries {
                return Err(last_error);
            }

            let delay = match &last_error {
                CoreError::RateLimit {
                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                    retry_after_secs,
                } => Duration::from_secs(*retry_after_secs),
                _ => {
                    jittered_backoff_delay(attempt, Duration::from_secs(1), Duration::from_secs(30))
                }
            };
            warn!(
                attempt = attempt + 1,
                delay_ms = delay.as_millis() as u64,
                "remote push retry"
            );
            tokio::time::sleep(delay).await;
        }
        Err(last_error)
    }

    async fn pull(&self, since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
        let (header_name, header_value) = self.auth_header();
        let url = format!(
            "{}/sync/pull?since_wall_ms={}&since_counter={}&device_id={}",
            self.endpoint, since.wall_ms, since.counter, self.local_device_id
        );

        let mut last_error = CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "pull failed".to_string(),
        };
        for attempt in 0..=self.max_retries {
            let result = self
                .client
                .get(&url)
                .header(header_name, &header_value)
                .send()
                .await;

            match result {
                Ok(resp) => {
                    let status = resp.status();
                    match status.as_u16() {
                        204 => return Ok(None),
                        200 => {
                            let bytes = resp.bytes().await.map_err(|e| CoreError::Network {
                                code: oneshim_core::error_codes::NetworkCode::Generic,
                                message: format!("read pull response: {e}"),
                            })?;
                            if bytes.is_empty() {
                                return Ok(None);
                            }
                            let plaintext = sync_crypto::decrypt(&self.passphrase, &bytes)?;
                            let cs: ChangeSet =
                                serde_json::from_slice(&plaintext).map_err(|e| {
                                    CoreError::Internal {
                                        code: oneshim_core::error_codes::InternalCode::Generic,
                                        message: format!("deserialize changeset: {e}"),
                                    }
                                })?;
                            debug!(
                                origin = %cs.origin_device_id,
                                rows = cs.row_count(),
                                "remote pull succeeded"
                            );
                            return Ok(Some(cs));
                        }
                        _ => {
                            let retry_after = extract_retry_after(&resp);
                            let body = resp.text().await.unwrap_or_default();
                            last_error = match status.as_u16() {
                                429 => CoreError::RateLimit {
                                    code: oneshim_core::error_codes::NetworkCode::RateLimit,
                                    retry_after_secs: retry_after,
                                },
                                _ => Self::check_response_status(status, &body)
                                    .err()
                                    .unwrap_or_else(|| CoreError::Internal {
                                        code: oneshim_core::error_codes::InternalCode::Generic,
                                        message: "unexpected".into(),
                                    }),
                            };
                        }
                    }
                }
                Err(e) => {
                    last_error = self.map_error(e, "remote pull");
                }
            }

            if !Self::is_retryable(&last_error) || attempt == self.max_retries {
                return Err(last_error);
            }

            let delay =
                jittered_backoff_delay(attempt, Duration::from_secs(1), Duration::from_secs(30));
            warn!(attempt = attempt + 1, "remote pull retry");
            tokio::time::sleep(delay).await;
        }
        Err(last_error)
    }

    async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
        let (header_name, header_value) = self.auth_header();
        let resp = self
            .client
            .get(format!("{}/sync/peers", self.endpoint))
            .header(header_name, &header_value)
            .send()
            .await
            .map_err(|e| self.map_error(e, "discover peers"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Self::check_response_status(status, &body)
                .err()
                .unwrap_or_else(|| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: "unexpected".into(),
                }));
        }

        let peers: Vec<PeerInfo> = resp.json().await.map_err(|e| {
            // Match sister module integration/http_transport/connect.rs pattern:
            // wrap as CoreError::Serialization so wire code is `internal.serialization`
            // (indicating parse failure) rather than generic `internal.generic`.
            CoreError::Serialization(serde_json::Error::io(std::io::Error::other(format!(
                "parse peers response: {e}"
            ))))
        })?;
        debug!(count = peers.len(), "discovered remote peers");
        Ok(peers)
    }

    async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        let (header_name, header_value) = self.auth_header();
        let resp = self
            .client
            .delete(format!("{}/sync/peers/{}", self.endpoint, device_id))
            .header(header_name, &header_value)
            .send()
            .await
            .map_err(|e| self.map_error(e, "forget peer"))?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 404 {
            debug!(device_id, "remote peer forgotten");
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Self::check_response_status(status, &body)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::sync::ChangeSetKind;

    fn test_transport(endpoint: &str) -> RemoteSyncTransport {
        RemoteSyncTransport::new(
            endpoint.to_string(),
            "test-device".to_string(),
            "test-passphrase".to_string(),
            RemoteSyncAuth::BearerToken,
            "test-token".to_string(),
        )
        .unwrap()
    }

    fn test_changeset() -> ChangeSet {
        ChangeSet {
            kind: ChangeSetKind::Data,
            origin_device_id: "test-device".to_string(),
            origin_device_name: "Test".to_string(),
            watermark: Hlc {
                wall_ms: 100,
                counter: 1,
                device_id: "test-device".to_string(),
            },
            segments: vec![serde_json::json!({"id": "seg-1"})],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn push_success_200() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/sync/push")
            .match_header("Authorization", "Bearer test-token")
            .with_status(200)
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let result = transport.push(&test_changeset()).await;
        assert!(result.is_ok());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn push_conflict_409_returns_ok() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/sync/push")
            .with_status(409)
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let result = transport.push(&test_changeset()).await;
        assert!(result.is_ok()); // 409 is not an error -- triggers re-pull
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn push_auth_failure_401_no_retry() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/sync/push")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let result = transport.push(&test_changeset()).await;
        assert!(matches!(result, Err(CoreError::Auth { .. })));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn pull_204_returns_none() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/sync/pull\?.*".to_string()),
            )
            .with_status(204)
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let result = transport.pull(&Hlc::default()).await;
        assert!(result.unwrap().is_none());
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn pull_200_decrypts_changeset() {
        let passphrase = "test-passphrase";
        let cs = test_changeset();
        let json = serde_json::to_vec(&cs).unwrap();
        let encrypted = sync_crypto::encrypt(passphrase, &json).unwrap();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/sync/pull\?.*".to_string()),
            )
            .with_status(200)
            .with_body(encrypted)
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let pulled = transport.pull(&Hlc::default()).await.unwrap();
        assert!(pulled.is_some());
        assert_eq!(pulled.unwrap().origin_device_id, "test-device");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn discover_peers_success() {
        let peers_json = serde_json::json!([
            {
                "device_id": "peer-1",
                "device_name": "Work MacBook",
                "last_sync_at": "2026-03-19T12:00:00Z",
                "watermark": { "wall_ms": 100, "counter": 1, "device_id": "peer-1" }
            }
        ]);

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/sync/peers")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(peers_json.to_string())
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let peers = transport.discover_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].device_id, "peer-1");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn api_key_auth_mode() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/sync/peers")
            .match_header("X-Api-Key", "my-api-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("[]")
            .create_async()
            .await;

        let transport = RemoteSyncTransport::new(
            server.url(),
            "dev".to_string(),
            "pass".to_string(),
            RemoteSyncAuth::ApiKey,
            "my-api-key".to_string(),
        )
        .unwrap();

        transport.discover_peers().await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn push_pull_roundtrip_integration() {
        let mut server = mockito::Server::new_async().await;

        // Mock push endpoint
        let push_mock = server
            .mock("POST", "/sync/push")
            .with_status(200)
            .create_async()
            .await;

        // Device A pushes
        let transport_a = test_transport(&server.url());
        let original = test_changeset();
        transport_a.push(&original).await.unwrap();
        push_mock.assert_async().await;

        // For the roundtrip, we encrypt manually and serve it on pull
        let json = serde_json::to_vec(&original).unwrap();
        let encrypted = sync_crypto::encrypt("test-passphrase", &json).unwrap();

        let pull_mock = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"/sync/pull\?.*".to_string()),
            )
            .with_status(200)
            .with_body(encrypted)
            .create_async()
            .await;

        // Device B pulls
        let transport_b = test_transport(&server.url());
        let pulled = transport_b.pull(&Hlc::default()).await.unwrap();
        assert!(pulled.is_some());
        let pulled_cs = pulled.unwrap();
        assert_eq!(pulled_cs.origin_device_id, original.origin_device_id);
        assert_eq!(pulled_cs.segments, original.segments);
        pull_mock.assert_async().await;
    }

    #[tokio::test]
    async fn forget_peer_sends_delete_with_auth_header() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("DELETE", "/sync/peers/device-123")
            .match_header("Authorization", "Bearer test-token")
            .with_status(204)
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        transport.forget_peer("device-123").await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn forget_peer_treats_404_as_success() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/unknown")
            .with_status(404)
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        assert!(transport.forget_peer("unknown").await.is_ok());
    }

    #[tokio::test]
    async fn forget_peer_bubbles_server_error() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/oops")
            .with_status(500)
            .with_body("internal")
            .create_async()
            .await;

        let transport = test_transport(&server.url());
        let err = transport.forget_peer("oops").await.unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("500") || msg.to_lowercase().contains("internal"),
            "unexpected error: {msg}"
        );
    }

    /// iter-66 regression guards for iter-55a semantic HTTP status mapping
    /// on sync/remote_transport::check_response_status. Each test asserts
    /// the typed CoreError variant for a specific status code.
    #[tokio::test]
    async fn forget_peer_403_maps_to_auth() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/x")
            .with_status(403)
            .with_body("forbidden")
            .create_async()
            .await;
        let transport = test_transport(&server.url());
        let err = transport.forget_peer("x").await.unwrap_err();
        assert!(
            matches!(err, CoreError::Auth { .. }),
            "403 must map to CoreError::Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn forget_peer_408_maps_to_timeout() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/x")
            .with_status(408)
            .with_body("request timeout")
            .create_async()
            .await;
        let transport = test_transport(&server.url());
        let err = transport.forget_peer("x").await.unwrap_err();
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "408 must map to CoreError::RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn forget_peer_502_maps_to_service_unavailable() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/x")
            .with_status(502)
            .with_body("bad gateway")
            .create_async()
            .await;
        let transport = test_transport(&server.url());
        let err = transport.forget_peer("x").await.unwrap_err();
        assert!(
            matches!(err, CoreError::ServiceUnavailable { .. }),
            "502 must map to CoreError::ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn forget_peer_504_maps_to_timeout() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/x")
            .with_status(504)
            .with_body("gateway timeout")
            .create_async()
            .await;
        let transport = test_transport(&server.url());
        let err = transport.forget_peer("x").await.unwrap_err();
        assert!(
            matches!(err, CoreError::RequestTimeout { .. }),
            "504 must map to CoreError::RequestTimeout, got: {err:?}"
        );
    }

    /// iter-78: domain fallback. 500 (not in specific arms) stays as Network.
    /// Complements `forget_peer_bubbles_server_error` (which asserts only the
    /// error message) with a variant-level assertion.
    #[tokio::test]
    async fn forget_peer_500_falls_back_to_network() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("DELETE", "/sync/peers/x")
            .with_status(500)
            .with_body("internal server error")
            .create_async()
            .await;
        let transport = test_transport(&server.url());
        let err = transport.forget_peer("x").await.unwrap_err();
        assert!(
            matches!(err, CoreError::Network { .. }),
            "500 should fall back to Network, got: {err:?}"
        );
    }
}
