//! #9504: revoke ↔ in-flight refresh race regression tests.
//!
//! `OAuthClient::revoke` must serialize behind the per-provider refresh lock.
//! Before the fix, `revoke` deleted the namespace WITHOUT taking the lock, so
//! a refresh mid-roundtrip (which holds the lock through its `store_tokens`
//! commit) landed afterwards and re-persisted the just-revoked credentials —
//! the #9481/#9491 resurrection shape that #9499 already fixed in
//! `TokenManager`. The gated-store technique mirrors
//! `auth/persistence_tests.rs`.

use std::sync::Arc;

use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::secret_store::SecretStore;

use super::provider_config::OAuthProviderConfig;
use super::{token_exchange, OAuthClient, KEY_ACCESS_TOKEN, KEY_EXPIRES_AT, KEY_REFRESH_TOKEN};

/// SecretStore fake that parks the *first* `store()` write until the test
/// opens a gate, and records an operation log so ordering can be asserted.
/// Seeding bypasses the trait (`seed`) so it never consumes the gate.
struct GatedOAuthStore {
    entries: tokio::sync::Mutex<std::collections::HashMap<(String, String), String>>,
    ops: tokio::sync::Mutex<Vec<String>>,
    entered: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    gate: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl GatedOAuthStore {
    fn new(
        entered: tokio::sync::oneshot::Sender<()>,
        gate: tokio::sync::oneshot::Receiver<()>,
    ) -> Self {
        Self {
            entries: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            ops: tokio::sync::Mutex::new(Vec::new()),
            entered: tokio::sync::Mutex::new(Some(entered)),
            gate: tokio::sync::Mutex::new(Some(gate)),
        }
    }

    /// A store with no gate — every write proceeds immediately.
    fn ungated() -> Self {
        Self {
            entries: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            ops: tokio::sync::Mutex::new(Vec::new()),
            entered: tokio::sync::Mutex::new(None),
            gate: tokio::sync::Mutex::new(None),
        }
    }

    /// Direct write into the backing map — never touches the gate.
    async fn seed(&self, ns: &str, key: &str, value: &str) {
        self.entries
            .lock()
            .await
            .insert((ns.to_string(), key.to_string()), value.to_string());
    }

    async fn ops(&self) -> Vec<String> {
        self.ops.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl SecretStore for GatedOAuthStore {
    async fn store(&self, ns: &str, key: &str, value: &str) -> Result<(), CoreError> {
        self.ops.lock().await.push(format!("store:{key}"));
        if let Some(tx) = self.entered.lock().await.take() {
            let _ = tx.send(());
        }
        let gate = self.gate.lock().await.take();
        if let Some(rx) = gate {
            let _ = rx.await;
        }
        self.entries
            .lock()
            .await
            .insert((ns.to_string(), key.to_string()), value.to_string());
        Ok(())
    }

    async fn retrieve(&self, ns: &str, key: &str) -> Result<Option<String>, CoreError> {
        Ok(self
            .entries
            .lock()
            .await
            .get(&(ns.to_string(), key.to_string()))
            .cloned())
    }

    async fn delete(&self, ns: &str, key: &str) -> Result<(), CoreError> {
        self.ops.lock().await.push(format!("delete:{key}"));
        self.entries
            .lock()
            .await
            .remove(&(ns.to_string(), key.to_string()));
        Ok(())
    }

    async fn delete_namespace(&self, ns: &str) -> Result<(), CoreError> {
        self.ops.lock().await.push("delete_namespace".to_string());
        self.entries.lock().await.retain(|(n, _), _| n != ns);
        Ok(())
    }
}

fn client_against(server_url: &str, store: Arc<GatedOAuthStore>) -> OAuthClient {
    let mut config = OAuthProviderConfig::openai_codex();
    config.token_endpoint = format!("{server_url}/token");
    OAuthClient::new(store, vec![config])
}

/// The core #9504 interleaving, made deterministic with a gated store:
/// a refresh reaches its commit (holding the per-provider lock) and parks;
/// a concurrent revoke must WAIT — not delete around it — so once both
/// complete, the revoked provider's namespace is empty. Before the fix the
/// revoke ran immediately and the parked refresh then re-persisted the
/// rotated (revoked) credentials.
#[tokio::test]
async fn revoke_waits_out_inflight_refresh_and_revoked_credentials_do_not_survive() {
    let mut server = mockito::Server::new_async().await;
    let refresh_http = server
        .mock("POST", "/token")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"access_token":"at-rotated","refresh_token":"rt-rotated","expires_in":3600}"#,
        )
        .create_async()
        .await;

    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let store = Arc::new(GatedOAuthStore::new(entered_tx, gate_rx));
    store.seed("openai", KEY_ACCESS_TOKEN, "at-live").await;
    store.seed("openai", KEY_REFRESH_TOKEN, "rt-live").await;

    let client = Arc::new(client_against(&server.url(), store.clone()));

    // 1. Refresh completes its roundtrip and parks at its FIRST store() write,
    //    still holding the per-provider refresh lock.
    let refresh_side = client.clone();
    let refresh_task = tokio::spawn(async move { refresh_side.try_refresh("openai").await });
    entered_rx
        .await
        .expect("refresh must reach its store_tokens commit");

    // 2. Revoke arrives mid-commit. With the fix it blocks on the refresh
    //    lock; without it, it deleted the namespace right here.
    let revoke_side = client.clone();
    let revoke_task = tokio::spawn(async move { revoke_side.revoke("openai").await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !revoke_task.is_finished(),
        "revoke must wait for the in-flight refresh, not delete around it"
    );

    // 3. Release the commit; refresh lands first, then revoke sweeps.
    gate_tx.send(()).expect("open the store gate");
    let refresh_outcome = refresh_task
        .await
        .expect("refresh task join")
        .expect("refresh must succeed");
    assert!(matches!(refresh_outcome, super::RefreshOutcome::Refreshed));
    revoke_task
        .await
        .expect("revoke task join")
        .expect("revoke must succeed");

    // Revoked means revoked: neither the old nor the rotated credentials survive.
    let access = store
        .retrieve("openai", KEY_ACCESS_TOKEN)
        .await
        .expect("retrieve access");
    let refresh = store
        .retrieve("openai", KEY_REFRESH_TOKEN)
        .await
        .expect("retrieve refresh");
    assert!(
        access.is_none(),
        "rotated access token resurrected: {access:?}"
    );
    assert!(
        refresh.is_none(),
        "rotated refresh token resurrected: {refresh:?}"
    );

    // Ordering proof: the namespace sweep happened strictly after every
    // refresh-side write.
    let ops = store.ops().await;
    let del = ops
        .iter()
        .position(|o| o == "delete_namespace")
        .expect("revoke must delete the namespace");
    let last_store = ops
        .iter()
        .rposition(|o| o.starts_with("store:"))
        .expect("refresh must have committed writes");
    assert!(
        del > last_store,
        "delete_namespace must be ordered after the refresh commit, got {ops:?}"
    );
    refresh_http.assert_async().await;
}

/// #9491 reconnect shape: after revoke + reconnecting a DIFFERENT account,
/// a refresh must use the new account's refresh token read from the store —
/// the revoked account's token must never be sent again.
#[tokio::test]
async fn refresh_after_revoke_and_reconnect_uses_only_the_new_accounts_token() {
    let mut server = mockito::Server::new_async().await;
    // The old account's refresh token must never reach the wire again.
    let stale = server
        .mock("POST", "/token")
        .match_body(mockito::Matcher::UrlEncoded(
            "refresh_token".into(),
            "rt-account-a".into(),
        ))
        .expect(0)
        .create_async()
        .await;
    let fresh = server
        .mock("POST", "/token")
        .match_body(mockito::Matcher::UrlEncoded(
            "refresh_token".into(),
            "rt-account-b".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"access_token":"at-b-rotated","refresh_token":"rt-b-rotated","expires_in":3600}"#,
        )
        .create_async()
        .await;

    let store = Arc::new(GatedOAuthStore::ungated());
    store.seed("openai", KEY_ACCESS_TOKEN, "at-account-a").await;
    store
        .seed("openai", KEY_REFRESH_TOKEN, "rt-account-a")
        .await;

    let client = client_against(&server.url(), store.clone());
    client.revoke("openai").await.expect("revoke account A");

    // Reconnect as account B — what the auth-code exchange completion does.
    client
        .store_tokens(
            "openai",
            &token_exchange::TokenExchangeResult {
                access_token: "at-account-b".into(),
                refresh_token: Some("rt-account-b".into()),
                expires_in: Some(0),
                scope: None,
                token_type: None,
            },
        )
        .await
        .expect("store account B tokens");
    // Expired on purpose so the next ensure path genuinely refreshes.
    store
        .seed(
            "openai",
            KEY_EXPIRES_AT,
            &(Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
        )
        .await;

    let outcome = client
        .try_refresh("openai")
        .await
        .expect("refresh must not error");
    assert!(matches!(outcome, super::RefreshOutcome::Refreshed));
    let access = store
        .retrieve("openai", KEY_ACCESS_TOKEN)
        .await
        .expect("retrieve access")
        .expect("access token must exist after refresh");
    assert_eq!(access, "at-b-rotated");

    stale.assert_async().await;
    fresh.assert_async().await;
}

/// #9504 review (A1): the CONNECT flow is the third writer of the provider
/// namespace. Its commit (`commit_connect_tokens`, the exact seam `start_flow`
/// Phase 2 calls) must serialize behind the same per-provider lock, so a
/// revoke arriving mid-commit waits and then sweeps — never deletes around an
/// in-flight 4-write store sequence (partial credential set) and never lets
/// the commit land after the sweep (resurrection).
#[tokio::test]
async fn revoke_waits_out_inflight_connect_commit_and_connect_tokens_do_not_survive() {
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let store = Arc::new(GatedOAuthStore::new(entered_tx, gate_rx));

    let client = Arc::new(client_against("http://127.0.0.1:9", store.clone()));
    let lock = client.refresh_lock_for("openai").await;

    // 1. The connect commit reaches its FIRST store() write and parks,
    //    holding the per-provider lock — exactly the Phase-2 state after a
    //    successful token exchange.
    let commit_store = store.clone();
    let commit_lock = lock.clone();
    let commit_task = tokio::spawn(async move {
        super::commit_connect_tokens(
            &commit_lock,
            &*commit_store,
            "openai",
            &token_exchange::TokenExchangeResult {
                access_token: "at-connect".into(),
                refresh_token: Some("rt-connect".into()),
                expires_in: Some(3600),
                scope: None,
                token_type: None,
            },
        )
        .await
    });
    entered_rx
        .await
        .expect("connect commit must reach its first store write");

    // 2. Revoke arrives mid-commit and must block on the shared lock.
    let revoke_side = client.clone();
    let revoke_task = tokio::spawn(async move { revoke_side.revoke("openai").await });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        !revoke_task.is_finished(),
        "revoke must wait for the in-flight connect commit, not delete around it"
    );

    // 3. Release: the commit lands all 4 writes, then the revoke sweeps.
    gate_tx.send(()).expect("open the store gate");
    commit_task
        .await
        .expect("commit task join")
        .expect("connect commit must succeed");
    revoke_task
        .await
        .expect("revoke task join")
        .expect("revoke must succeed");

    let access = store
        .retrieve("openai", KEY_ACCESS_TOKEN)
        .await
        .expect("retrieve access");
    assert!(
        access.is_none(),
        "connect-flow tokens resurrected past revoke: {access:?}"
    );
    let ops = store.ops().await;
    let del = ops
        .iter()
        .position(|o| o == "delete_namespace")
        .expect("revoke must delete the namespace");
    let last_store = ops
        .iter()
        .rposition(|o| o.starts_with("store:"))
        .expect("connect commit must have written");
    assert!(
        del > last_store,
        "delete_namespace must be ordered after the connect commit, got {ops:?}"
    );
}
