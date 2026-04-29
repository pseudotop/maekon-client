use tracing::{debug, warn};

use oneshim_core::error::CoreError;
use oneshim_core::models::sync::ChangeSet;
use oneshim_core::sync::Hlc;

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

        let url = Self::peer_url(peer, "/sync/push");

        let resp = self
            .http_client
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

                let retry = self
                    .http_client
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
                let body = r.text().await.unwrap_or_default();
                warn!(peer_id, %status, body, "push to LAN peer rejected");
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

        let url = format!(
            "{}?since_wall_ms={}&since_counter={}&device_id={}",
            Self::peer_url(peer, "/sync/pull"),
            since.wall_ms,
            since.counter,
            self.local_device_id,
        );

        let resp = self
            .http_client
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

                let retry = self
                    .http_client
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
        let bytes = resp.bytes().await.map_err(|e| CoreError::Network {
            code: oneshim_core::error_codes::NetworkCode::Generic,
            message: format!("read pull body: {e}"),
        })?;

        if bytes.is_empty() {
            return Ok(None);
        }

        let plaintext = sync_crypto::decrypt(&self.passphrase, &bytes)?;
        let changesets: Vec<ChangeSet> =
            serde_json::from_slice(&plaintext).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("deserialize pull response: {e}"),
            })?;

        if changesets.is_empty() {
            return Ok(None);
        }

        // Merge all pulled changesets into a single composite changeset
        // by concatenating their Vec fields and keeping the latest watermark.
        let mut iter = changesets.into_iter();
        // Safe: checked `changesets.is_empty()` above, so at least one element exists.
        let mut merged = iter.next().expect("non-empty checked above");
        for cs in iter {
            merged.segments.extend(cs.segments);
            merged.regimes.extend(cs.regimes);
            merged.overrides.extend(cs.overrides);
            merged.embeddings.extend(cs.embeddings);
            merged.suggestions.extend(cs.suggestions);
            merged.param_snapshots.extend(cs.param_snapshots);
            merged.preferences.extend(cs.preferences);
            // Keep the latest watermark
            if cs.watermark.wall_ms > merged.watermark.wall_ms
                || (cs.watermark.wall_ms == merged.watermark.wall_ms
                    && cs.watermark.counter > merged.watermark.counter)
            {
                merged.watermark = cs.watermark;
            }
        }
        debug!(
            peer_id,
            origin = %merged.origin_device_id,
            rows = merged.row_count(),
            "pulled from LAN peer"
        );
        Ok(Some(merged))
    }
}
