use tracing::{debug, warn};

use maekon_core::error::CoreError;
use maekon_core::models::sync::ChangeSet;
use maekon_core::sync::Hlc;

use crate::sync::lan_discovery::LanPeerInfo;
use crate::sync::sync_crypto;

use super::LanSyncTransport;

impl LanSyncTransport {
    ///
    /// Authenticates first (from cache or fresh handshake), then pushes.
    /// If push gets 401, invalidates the cached token and retries once.
    pub(super) async fn push_to_peer(
        &self,
        peer_id: &str,
        peer: &LanPeerInfo,
        encrypted: &[u8],
    ) -> Result<bool, CoreError> {
        let token = match self.get_session_token_with_retry(peer_id, peer).await {
            Ok(t) => t,
            Err(e) => {
                warn!(peer_id, error = %e, "failed to authenticate with peer for push");
                return Ok(false);
            }
        };

        // get_session_token_with_retry drives authenticate_with_peer on a cache
        // miss, which populates peer_clients with the TOFU-pinned TLS client.
        let client = self
            .cached_peer_client(peer_id)
            .ok_or_else(|| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("no TLS client cached for peer {peer_id}"),
            })?;

        let url = Self::peer_url(peer, "/sync/push");

        let resp = client
            .post(&url)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/octet-stream")
            .body(encrypted.to_vec())
            .send()
            .await;

        match resp {
            Ok(r) if r.status().is_success() => {
                debug!(peer_id, "push to LAN peer succeeded");
                Ok(true)
            }
            Ok(r) if r.status().as_u16() == 401 => {
                // Token rejected -- invalidate cache and retry once
                debug!(peer_id, "push 401, re-authenticating");
                self.token_cache.invalidate(peer_id);
                let new_token = match self.authenticate_with_peer(peer_id, peer).await {
                    Ok(t) => {
                        self.token_cache.put(peer_id, t.clone());
                        t
                    }
                    Err(e) => {
                        warn!(peer_id, error = %e, "re-authentication failed");
                        return Ok(false);
                    }
                };

                // authenticate_with_peer has refreshed the per-peer TLS client.
                let retry_client =
                    self.cached_peer_client(peer_id)
                        .ok_or_else(|| CoreError::Network {
                            code: maekon_core::error_codes::NetworkCode::Generic,
                            message: format!(
                                "no TLS client cached for peer {peer_id} after re-auth"
                            ),
                        })?;

                let retry = retry_client
                    .post(&url)
                    .header("authorization", format!("Bearer {new_token}"))
                    .header("content-type", "application/octet-stream")
                    .body(encrypted.to_vec())
                    .send()
                    .await;

                match retry {
                    Ok(r) if r.status().is_success() => {
                        debug!(peer_id, "push succeeded after re-auth");
                        Ok(true)
                    }
                    Ok(r) => {
                        let status = r.status();
                        warn!(peer_id, %status, "push failed after re-auth");
                        Ok(false)
                    }
                    Err(e) => {
                        warn!(peer_id, error = %e, "push retry failed");
                        Ok(false)
                    }
                }
            }
            Ok(r) => {
                let status = r.status();
                // #6923: cap the peer error body (same OOM class as the pull path).
                let body = crate::sync::http_body::read_text_capped(
                    r,
                    crate::sync::http_body::MAX_CONTROL_RESPONSE_BYTES,
                )
                .await
                .unwrap_or_default();
                // Log response length only — peer error bodies may contain
                // echoed request fragments or internal server details (#6006).
                warn!(peer_id, %status, response_len = body.len(), "push to LAN peer rejected");
                Ok(false)
            }
            Err(e) => {
                warn!(peer_id, error = %e, "push to LAN peer failed");
                Ok(false)
            }
        }
    }

    /// Pull encrypted changesets from a single peer. Returns decrypted changeset(s).
    ///
    /// Authenticates first, then pulls. Retries once on 401.
    pub(super) async fn pull_from_peer(
        &self,
        peer_id: &str,
        peer: &LanPeerInfo,
        since: &Hlc,
    ) -> Result<Option<ChangeSet>, CoreError> {
        let token = match self.get_session_token_with_retry(peer_id, peer).await {
            Ok(t) => t,
            Err(e) => {
                warn!(peer_id, error = %e, "failed to authenticate with peer for pull");
                return Ok(None);
            }
        };

        // get_session_token_with_retry drives authenticate_with_peer on a cache
        // miss, which populates peer_clients with the TOFU-pinned TLS client.
        let client = self
            .cached_peer_client(peer_id)
            .ok_or_else(|| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("no TLS client cached for peer {peer_id}"),
            })?;

        let url = format!(
            "{}?since_wall_ms={}&since_counter={}&device_id={}",
            Self::peer_url(peer, "/sync/pull"),
            since.wall_ms,
            since.counter,
            self.local_device_id,
        );

        let resp = client
            .get(&url)
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await;

        match resp {
            Ok(r) if r.status().as_u16() == 204 => {
                debug!(peer_id, "peer has no new data");
                Ok(None)
            }
            Ok(r) if r.status().as_u16() == 401 => {
                // Token rejected -- invalidate cache and retry once
                debug!(peer_id, "pull 401, re-authenticating");
                self.token_cache.invalidate(peer_id);
                let new_token = match self.authenticate_with_peer(peer_id, peer).await {
                    Ok(t) => {
                        self.token_cache.put(peer_id, t.clone());
                        t
                    }
                    Err(e) => {
                        warn!(peer_id, error = %e, "re-authentication for pull failed");
                        return Ok(None);
                    }
                };

                // authenticate_with_peer has refreshed the per-peer TLS client.
                let retry_client =
                    self.cached_peer_client(peer_id)
                        .ok_or_else(|| CoreError::Network {
                            code: maekon_core::error_codes::NetworkCode::Generic,
                            message: format!(
                                "no TLS client cached for peer {peer_id} after re-auth"
                            ),
                        })?;

                let retry = retry_client
                    .get(&url)
                    .header("authorization", format!("Bearer {new_token}"))
                    .send()
                    .await;

                match retry {
                    Ok(r) if r.status().is_success() => self.decode_pull_response(peer_id, r).await,
                    Ok(r) if r.status().as_u16() == 204 => Ok(None),
                    Ok(r) => {
                        warn!(peer_id, status = %r.status(), "pull failed after re-auth");
                        Ok(None)
                    }
                    Err(e) => {
                        warn!(peer_id, error = %e, "pull retry failed");
                        Ok(None)
                    }
                }
            }
            Ok(r) if r.status().is_success() => self.decode_pull_response(peer_id, r).await,
            Ok(r) => {
                let status = r.status();
                warn!(peer_id, %status, "pull from LAN peer returned unexpected status");
                Ok(None)
            }
            Err(e) => {
                warn!(peer_id, error = %e, "pull from LAN peer failed");
                Ok(None)
            }
        }
    }

    /// Decode and decrypt a successful pull response.
    async fn decode_pull_response(
        &self,
        peer_id: &str,
        resp: reqwest::Response,
    ) -> Result<Option<ChangeSet>, CoreError> {
        // #6923: cap the peer pull body before buffering. #6917 hardened the
        // remote_transport sibling but left this LAN path uncapped — a compromised/
        // misbehaving paired peer could stream an unbounded body and OOM the agent
        // (the buffer happens BEFORE the decrypt/auth gate). Shared helper, 64 MiB.
        let bytes = crate::sync::http_body::read_body_capped(
            resp,
            crate::sync::http_body::MAX_PULL_RESPONSE_BYTES,
        )
        .await?;

        if bytes.is_empty() {
            return Ok(None);
        }

        let plaintext = sync_crypto::decrypt(&self.passphrase, &bytes)?;
        let changesets: Vec<ChangeSet> =
            serde_json::from_slice(&plaintext).map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("deserialize pull response: {e}"),
            })?;

        let merged = match merge_pulled_changesets(changesets) {
            Some(m) => m,
            None => return Ok(None),
        };
        debug!(
            peer_id,
            origin = %merged.origin_device_id,
            rows = merged.row_count(),
            "pulled from LAN peer"
        );
        Ok(Some(merged))
    }
}

/// Composite-merge pulled changesets into one, concatenating every row vec and keeping
/// the latest watermark. Returns `None` for an empty pull.
///
/// The GDPR Art.17 erasure `tombstones` (#5174) are concatenated like any other row vec —
/// the post-erase changeset is pulled AFTER the data changeset, so OMITTING them here
/// would silently drop the erasure and break cross-device convergence over LAN.
fn merge_pulled_changesets(changesets: Vec<ChangeSet>) -> Option<ChangeSet> {
    let mut iter = changesets.into_iter();
    let mut merged = iter.next()?;
    for cs in iter {
        merged.segments.extend(cs.segments);
        merged.regimes.extend(cs.regimes);
        merged.overrides.extend(cs.overrides);
        merged.embeddings.extend(cs.embeddings);
        merged.suggestions.extend(cs.suggestions);
        merged.param_snapshots.extend(cs.param_snapshots);
        merged.preferences.extend(cs.preferences);
        merged.tombstones.extend(cs.tombstones);
        // Keep the latest watermark.
        if cs.watermark.wall_ms > merged.watermark.wall_ms
            || (cs.watermark.wall_ms == merged.watermark.wall_ms
                && cs.watermark.counter > merged.watermark.counter)
        {
            merged.watermark = cs.watermark;
        }
    }
    Some(merged)
}

#[cfg(test)]
mod composite_merge_tests {
    use super::*;
    use maekon_core::models::sync::Tombstone;

    fn cs_with_tombstone(row_id: &str, wall: u64) -> ChangeSet {
        ChangeSet {
            watermark: maekon_core::sync::Hlc {
                wall_ms: wall,
                counter: 0,
                device_id: "dev-a".to_string(),
            },
            tombstones: vec![Tombstone {
                table_name: "activity_segments".to_string(),
                row_id: row_id.to_string(),
                origin_device_id: "dev-a".to_string(),
                hlc_wall_ms: wall,
                hlc_counter: 0,
                deleted_at: "2026-01-01 00:00:00".to_string(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn empty_pull_merges_to_none() {
        assert!(merge_pulled_changesets(vec![]).is_none());
    }

    #[test]
    fn tombstones_from_every_changeset_survive_composite_merge() {
        // #5174 regression: a peer pulling [data_cs, post-erase tombstone_cs] must NOT
        // drop the erasure tombstone carried by the second changeset.
        let data_cs = ChangeSet {
            segments: vec![serde_json::json!({"id": "seg-1"})],
            watermark: maekon_core::sync::Hlc {
                wall_ms: 100,
                counter: 0,
                device_id: "dev-a".to_string(),
            },
            ..Default::default()
        };
        let merged = merge_pulled_changesets(vec![data_cs, cs_with_tombstone("seg-1", 200)])
            .expect("non-empty");
        assert_eq!(
            merged.tombstones.len(),
            1,
            "later changeset's tombstone preserved"
        );
        assert_eq!(merged.tombstones[0].row_id, "seg-1");
        assert_eq!(merged.segments.len(), 1, "data also preserved");
        assert_eq!(merged.watermark.wall_ms, 200, "latest watermark kept");
    }
}
