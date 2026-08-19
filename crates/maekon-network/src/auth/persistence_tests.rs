//! ONESHIM login-session persistence tests for `TokenManager` (#9459).
//!
//! Split out of the sibling `tests.rs` when that file crossed the 900-line
//! ADR-013 new-giant threshold (`cargo test -p maekon-lint --test
//! adr013_loc_growth_gate`). The persistence suite is the natural seam: it is
//! self-contained (its own `SecretStore` fakes, its own round-trip fixtures)
//! and shares no helper with the retry/backoff suite that stayed behind.

#[cfg(test)]
#[allow(deprecated)]
#[allow(clippy::module_inception)]
mod tests {
    use chrono::{Duration, Utc};
    use maekon_core::error::CoreError;

    use crate::auth::tokens::{TokenManager, TokenState};

    /// Local copy of `tests.rs`'s fixture: a runtime-built password, because a
    /// string literal at a `login()` call site trips CodeQL
    /// `rust/hard-coded-cryptographic-value`. Duplicated rather than shared —
    /// exposing the sibling test module's private helpers would mean widening
    /// `mod tests` (and its parent) beyond test scope for three lines.
    fn primary_password() -> String {
        String::from_utf8(vec![b'x'; 16]).expect("password fixture bytes must be UTF-8")
    }

    /// In-memory SecretStore fake for persistence round-trip tests.
    struct MemorySecretStore(
        tokio::sync::Mutex<std::collections::HashMap<(String, String), String>>,
    );

    impl MemorySecretStore {
        fn new() -> Self {
            Self(tokio::sync::Mutex::new(std::collections::HashMap::new()))
        }
    }

    #[async_trait::async_trait]
    impl maekon_core::ports::secret_store::SecretStore for MemorySecretStore {
        async fn store(&self, ns: &str, key: &str, value: &str) -> Result<(), CoreError> {
            self.0
                .lock()
                .await
                .insert((ns.to_string(), key.to_string()), value.to_string());
            Ok(())
        }
        async fn retrieve(&self, ns: &str, key: &str) -> Result<Option<String>, CoreError> {
            let entry = (ns.to_string(), key.to_string());
            Ok(self.0.lock().await.get(&entry).cloned())
        }
        async fn delete(&self, ns: &str, key: &str) -> Result<(), CoreError> {
            let entry = (ns.to_string(), key.to_string());
            self.0.lock().await.remove(&entry);
            Ok(())
        }
        async fn delete_namespace(&self, ns: &str) -> Result<(), CoreError> {
            self.0.lock().await.retain(|(n, _), _| n != ns);
            Ok(())
        }
    }

    /// Contract fixture: current server `AuthenticatedLoginResponse` shape
    /// (outcome discriminator + extra fields). Client parsing must tolerate
    /// unknown fields and read top-level access_token/refresh_token/expires_in.
    const AUTHENTICATED_LOGIN_FIXTURE: &str = r#"{"outcome":"authenticated","success":true,"message":"ok",
                "timestamp":"2026-07-29T00:00:00Z","request_id":"r-1","metadata":{},
                "user_id":"u-1","username":"mingyu_song","email":"m@example.com",
                "organization_id":"org-e2e-futurepac","organization_slug":"futurepac",
                "access_token":"at-1","refresh_token":"rt-1","token_type":"bearer",
                "expires_in":900,"session_id":"s-1","requires_mfa":false,
                "mfa_methods":[],"user_permissions":[],"user_roles":[],"organizations":[]}"#;

    #[tokio::test]
    async fn login_persists_session_and_restore_rehydrates() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(AUTHENTICATED_LOGIN_FIXTURE)
            .create_async()
            .await;

        let store = std::sync::Arc::new(MemorySecretStore::new());
        let tm = TokenManager::new(&server.url()).with_persistence(store.clone());
        tm.login_with_org("mingyu_song", &primary_password(), "org-e2e-futurepac")
            .await
            .expect("login must succeed against contract fixture");

        use maekon_core::ports::secret_store::*;
        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            Some("at-1".into())
        );
        assert_eq!(
            store
                .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, ONESHIM_IDENTIFIER_SECRET_KEY)
                .await
                .unwrap(),
            Some("mingyu_song".into())
        );

        // Fresh manager + same store => restore rehydrates without network.
        let tm2 = TokenManager::new(&server.url()).with_persistence(store.clone());
        assert!(
            tm2.restore_persisted_session().await,
            "restore must succeed"
        );
        assert!(tm2.is_authenticated().await);
        let info = tm2
            .session_info()
            .await
            .expect("session info after restore");
        assert_eq!(info.identifier.as_deref(), Some("mingyu_song"));
        assert_eq!(info.organization_id.as_deref(), Some("org-e2e-futurepac"));
    }

    #[tokio::test]
    async fn logout_clears_persisted_session() {
        use maekon_core::ports::secret_store::*;

        let mut server = mockito::Server::new_async().await;
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(AUTHENTICATED_LOGIN_FIXTURE)
            .create_async()
            .await;
        let logout_mock = server
            .mock("DELETE", "/api/v1/auth/tokens")
            .match_header("authorization", "Bearer at-1")
            .with_status(200)
            .create_async()
            .await;

        let store = std::sync::Arc::new(MemorySecretStore::new());
        let tm = TokenManager::new(&server.url()).with_persistence(store.clone());
        tm.login_with_org("mingyu_song", &primary_password(), "org-e2e-futurepac")
            .await
            .expect("login must succeed against contract fixture");
        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            Some("at-1".into()),
            "precondition: login must have persisted the access token"
        );

        tm.logout().await.expect("logout must succeed");

        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            None,
            "logout must clear the persisted access token"
        );
        assert_eq!(
            store
                .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, ONESHIM_IDENTIFIER_SECRET_KEY)
                .await
                .unwrap(),
            None,
            "logout must clear the persisted identifier"
        );

        let tm2 = TokenManager::new(&server.url()).with_persistence(store.clone());
        assert!(
            !tm2.restore_persisted_session().await,
            "a cleared session must not restore"
        );
        assert!(!tm2.is_authenticated().await);

        login_mock.assert_async().await;
        logout_mock.assert_async().await;
    }

    #[tokio::test]
    async fn restore_rejects_expired_or_corrupt_material() {
        use maekon_core::ports::secret_store::*;

        let store = std::sync::Arc::new(MemorySecretStore::new());
        // Expired access token with no refresh token — nothing left to salvage.
        store
            .store(
                ONESHIM_AUTH_SECRET_NAMESPACE,
                ONESHIM_ACCESS_TOKEN_SECRET_KEY,
                "at-expired",
            )
            .await
            .unwrap();
        store
            .store(
                ONESHIM_AUTH_SECRET_NAMESPACE,
                ONESHIM_EXPIRES_AT_SECRET_KEY,
                &(Utc::now() - Duration::hours(1)).to_rfc3339(),
            )
            .await
            .unwrap();

        let tm = TokenManager::new("http://localhost:9999").with_persistence(store.clone());
        assert!(
            !tm.restore_persisted_session().await,
            "expired material with no refresh token must not restore"
        );
        assert!(!tm.is_authenticated().await);
        assert!(
            tm.session_info().await.is_none(),
            "a rejected restore must leave the session empty"
        );

        // Corrupt expiry — must be ignored rather than panic the bootstrap.
        store
            .store(
                ONESHIM_AUTH_SECRET_NAMESPACE,
                ONESHIM_EXPIRES_AT_SECRET_KEY,
                "not-an-rfc3339-timestamp",
            )
            .await
            .unwrap();

        let tm2 = TokenManager::new("http://localhost:9999").with_persistence(store.clone());
        assert!(
            !tm2.restore_persisted_session().await,
            "unparsable expires_at must not restore"
        );
        assert!(!tm2.is_authenticated().await);
    }

    /// The refresh response carries token material only, so `identifier` /
    /// `organization_id` must survive the state swap and be re-persisted
    /// alongside the rotated tokens.
    #[tokio::test]
    async fn refresh_persists_rotated_tokens_and_carries_session_metadata() {
        use maekon_core::ports::secret_store::*;

        let mut server = mockito::Server::new_async().await;
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(AUTHENTICATED_LOGIN_FIXTURE)
            .create_async()
            .await;
        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"at-2","refresh_token":"rt-2","expires_in":900}"#)
            .create_async()
            .await;

        let store = std::sync::Arc::new(MemorySecretStore::new());
        let tm = TokenManager::new(&server.url()).with_persistence(store.clone());
        tm.login_with_org("mingyu_song", &primary_password(), "org-e2e-futurepac")
            .await
            .expect("login must succeed against contract fixture");

        tm.refresh().await.expect("refresh must succeed");

        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            Some("at-2".into()),
            "refresh must re-persist the rotated access token"
        );
        assert_eq!(
            store
                .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, ONESHIM_IDENTIFIER_SECRET_KEY)
                .await
                .unwrap(),
            Some("mingyu_song".into()),
            "refresh must not drop the persisted identifier"
        );
        let info = tm.session_info().await.expect("session info after refresh");
        assert_eq!(info.identifier.as_deref(), Some("mingyu_song"));
        assert_eq!(info.organization_id.as_deref(), Some("org-e2e-futurepac"));

        login_mock.assert_async().await;
        refresh_mock.assert_async().await;
    }

    /// SecretStore fake that parks the *first* `store()` write until the test
    /// opens a gate, so one persist can be frozen mid-write while another runs.
    /// Records an operation log so the test can observe whether the second
    /// persist interleaved.
    struct GatedSecretStore {
        entries: tokio::sync::Mutex<std::collections::HashMap<(String, String), String>>,
        ops: tokio::sync::Mutex<Vec<String>>,
        entered: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        gate: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl GatedSecretStore {
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

        async fn ops(&self) -> Vec<String> {
            self.ops.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl maekon_core::ports::secret_store::SecretStore for GatedSecretStore {
        async fn store(&self, ns: &str, key: &str, value: &str) -> Result<(), CoreError> {
            self.ops.lock().await.push(format!("store:{key}"));
            // Signal the test that a persist reached its first write, then park
            // until the test releases the gate. Both are one-shot, so every
            // later write in the same persist proceeds without blocking.
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
            let entry = (ns.to_string(), key.to_string());
            Ok(self.entries.lock().await.get(&entry).cloned())
        }
        async fn delete(&self, ns: &str, key: &str) -> Result<(), CoreError> {
            self.ops.lock().await.push(format!("delete:{key}"));
            let entry = (ns.to_string(), key.to_string());
            self.entries.lock().await.remove(&entry);
            Ok(())
        }
        async fn delete_namespace(&self, ns: &str) -> Result<(), CoreError> {
            self.ops.lock().await.push("delete_namespace".to_string());
            self.entries.lock().await.retain(|(n, _), _| n != ns);
            Ok(())
        }
    }

    /// Fix round 1 regression (#9459): a persist that is already in flight must
    /// not be able to write its snapshot after a concurrent logout cleared the
    /// namespace. Without `persist_lock`, the refresh-side persist resumes after
    /// `delete_namespace` and resurrects a session the user already logged out
    /// of — the next launch would then restore revoked credentials.
    #[tokio::test]
    async fn concurrent_persist_cannot_resurrect_a_logged_out_session() {
        use maekon_core::ports::secret_store::*;

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
        let store = std::sync::Arc::new(GatedSecretStore::new(entered_tx, gate_rx));

        let tm = TokenManager::new("http://localhost:9999").with_persistence(store.clone());
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "at-live".to_string(),
                refresh_token: Some("rt-live".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: Some("mingyu_song".to_string()),
                organization_id: Some("org-e2e-futurepac".to_string()),
            });
        }

        // A refresh-side persist gets as far as its first keychain write, then parks.
        let refresh_side = tm.clone();
        let refresh_persist =
            tokio::spawn(async move { refresh_side.persist_current_state().await });
        entered_rx
            .await
            .expect("the refresh-side persist must reach its first store() write");

        // The user logs out while that write is still in flight.
        {
            let mut state = tm.state.write().await;
            *state = None;
        }
        let logout_side = tm.clone();
        let logout_persist = tokio::spawn(async move { logout_side.persist_current_state().await });
        // Give the logout persist every chance to run. It must not: the
        // in-flight persist still holds `persist_lock`.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let ops_mid_flight = store.ops().await;
        assert!(
            !ops_mid_flight.iter().any(|op| op == "delete_namespace"),
            "the logout persist must be serialized behind the in-flight persist, \
             but it already ran: {ops_mid_flight:?}"
        );

        // Release the frozen write and let both persists finish.
        gate_tx.send(()).expect("gate receiver must still be alive");
        refresh_persist.await.expect("refresh-side persist task");
        logout_persist.await.expect("logout-side persist task");

        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            None,
            "a logged-out session must not survive an interleaved persist"
        );
        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_REFRESH_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            None,
            "the refresh token must not survive either"
        );
        assert!(
            !tm.restore_persisted_session().await,
            "nothing must be left for a later launch to restore"
        );
    }

    /// Account-B token material the gated stub below serves on `POST
    /// /api/v1/auth/tokens`, so a re-login can complete while an account-A
    /// refresh is still parked mid-flight (#9491).
    const RELOGIN_LOGIN_FIXTURE: &str =
        r#"{"access_token":"at-b","refresh_token":"rt-b","expires_in":900}"#;

    /// Rotated token material the gated stub below serves once `gate` fires.
    const GATED_REFRESH_FIXTURE: &str =
        r#"{"access_token":"at-rotated","refresh_token":"rt-rotated","expires_in":900}"#;

    /// Stub HTTP/1.1 server that parks its `/api/v1/auth/tokens/refresh` reply
    /// until `gate` fires, while answering every other request immediately: the
    /// logout `DELETE` gets an empty body, and a `POST /api/v1/auth/tokens`
    /// login gets [`RELOGIN_LOGIN_FIXTURE`] (account B). Returns the bound base
    /// URL.
    ///
    /// Hand-rolled rather than `mockito` because the interleaving under test
    /// needs a response held open *across* another request: mockito matches and
    /// replies in one step, with no seam to freeze. Each connection is handled
    /// in its own task, so the parked refresh cannot block the logout or the
    /// re-login that follows it.
    async fn spawn_gated_refresh_server(
        refresh_seen: tokio::sync::oneshot::Sender<()>,
        gate: tokio::sync::oneshot::Receiver<()>,
    ) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("stub server must bind a loopback port");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("bound stub server address")
        );

        // One-shot halves shared across per-connection tasks, mirroring
        // `GatedSecretStore`: whichever handler sees the refresh first takes them.
        let refresh_seen = std::sync::Arc::new(tokio::sync::Mutex::new(Some(refresh_seen)));
        let gate = std::sync::Arc::new(tokio::sync::Mutex::new(Some(gate)));

        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let refresh_seen = refresh_seen.clone();
                let gate = gate.clone();
                tokio::spawn(async move {
                    // Read just far enough to route on the request line; the
                    // body is irrelevant to this test.
                    let mut head = Vec::new();
                    let mut buf = [0u8; 512];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }

                    // Route on the request line alone: `POST /api/v1/auth/tokens`
                    // (login) and `DELETE /api/v1/auth/tokens` (logout) share a
                    // path, and the refresh path is a prefix-extension of both,
                    // so method + exact path is the only unambiguous key.
                    let head_text = String::from_utf8_lossy(&head).into_owned();
                    let request_line = head_text.lines().next().unwrap_or_default().to_string();

                    let body = if request_line.contains("/auth/tokens/refresh") {
                        if let Some(tx) = refresh_seen.lock().await.take() {
                            let _ = tx.send(());
                        }
                        if let Some(rx) = gate.lock().await.take() {
                            let _ = rx.await;
                        }
                        GATED_REFRESH_FIXTURE
                    } else if request_line.starts_with("POST /api/v1/auth/tokens ") {
                        RELOGIN_LOGIN_FIXTURE
                    } else {
                        "{}"
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        base_url
    }

    /// Fix round 2 regression (#9459): a refresh whose response lands *after* a
    /// logout must not resurrect the session. Neither `logout()` nor
    /// `logout_all_sessions()` holds `refresh_lock`, so an in-flight refresh can
    /// return once the state is already cleared and the namespace already wiped
    /// — and the unconditional `*state = Some(rotated)` then repopulated memory
    /// and re-persisted revoked tokens plus the identifier, so the next launch
    /// greeted a logged-out user with "Signed in as …".
    ///
    /// Runs the real `refresh()` and `logout()` against the gated stub above, so
    /// the ordering is deterministic rather than timing-dependent.
    #[tokio::test]
    async fn refresh_completing_after_logout_does_not_resurrect_the_session() {
        use maekon_core::ports::secret_store::*;

        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
        let base_url = spawn_gated_refresh_server(seen_tx, gate_rx).await;

        let store = std::sync::Arc::new(MemorySecretStore::new());
        let tm = TokenManager::new(&base_url).with_persistence(store.clone());
        {
            let mut state = tm.state.write().await;
            // An hour of headroom keeps `logout()`'s own `get_token()` from
            // starting a second, nested refresh.
            *state = Some(TokenState {
                access_token: "at-live".to_string(),
                refresh_token: Some("rt-live".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: Some("mingyu_song".to_string()),
                organization_id: Some("org-e2e-futurepac".to_string()),
            });
        }
        tm.persist_current_state().await;
        assert_eq!(
            store
                .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, ONESHIM_IDENTIFIER_SECRET_KEY)
                .await
                .unwrap(),
            Some("mingyu_song".into()),
            "precondition: the seeded session must be persisted"
        );

        // Background refresh reaches the server and parks there.
        let refresh_side = tm.clone();
        let refresh_task = tokio::spawn(async move { refresh_side.refresh().await });
        seen_rx
            .await
            .expect("the refresh must reach the stub server before the logout");

        // The user logs out while the refresh response is still outstanding.
        tm.logout().await.expect("logout must succeed");
        assert_eq!(
            store
                .retrieve(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY
                )
                .await
                .unwrap(),
            None,
            "precondition: logout must have cleared the namespace"
        );

        // Release the rotated-token response into the logged-out client.
        gate_tx.send(()).expect("gate receiver must still be alive");
        let refresh_result = refresh_task.await.expect("refresh task must not panic");
        let refresh_error = refresh_result.expect_err(
            "a refresh that resolves into a logged-out session must not report success",
        );
        assert!(
            matches!(&refresh_error, CoreError::Auth { .. }),
            "post-logout refresh must fail as an auth error, got: {refresh_error}"
        );
        assert!(
            refresh_error
                .to_string()
                .contains("session ended during refresh"),
            "the error must name the logout race, got: {refresh_error}"
        );

        assert!(
            !tm.is_authenticated().await,
            "the rotated tokens must not repopulate the in-memory session"
        );
        assert!(
            tm.session_info().await.is_none(),
            "the logged-out session must stay empty"
        );
        for key in [
            ONESHIM_ACCESS_TOKEN_SECRET_KEY,
            ONESHIM_REFRESH_TOKEN_SECRET_KEY,
            ONESHIM_IDENTIFIER_SECRET_KEY,
            ONESHIM_ORGANIZATION_ID_SECRET_KEY,
        ] {
            assert_eq!(
                store
                    .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, key)
                    .await
                    .unwrap(),
                None,
                "'{key}' must not be re-persisted by a post-logout refresh"
            );
        }
        assert!(
            !tm.restore_persisted_session().await,
            "the next launch must not restore a session the user logged out of"
        );
    }

    /// #9491: the post-logout guard above only covers the logout -> `None`
    /// case. The logout -> re-login interleaving walks straight past a bare
    /// `state.is_none()` check — account A's refresh lands *after* the user has
    /// already signed back in as account B, `is_none()` is false, and A's
    /// rotated tokens plus A's `identifier`/`organization_id` overwrite B's
    /// fresh session in memory and in the keychain. The user believes they are
    /// signed in as B while the session has silently reverted to A.
    ///
    /// The session-generation counter closes it: any state transition between
    /// a refresh's start and its completion invalidates the rotation, so the
    /// re-login (not just the logout) is enough to discard A's response.
    #[tokio::test]
    async fn refresh_completing_after_relogin_does_not_overwrite_the_new_session() {
        use maekon_core::ports::secret_store::*;

        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel();
        let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
        let base_url = spawn_gated_refresh_server(seen_tx, gate_rx).await;

        let store = std::sync::Arc::new(MemorySecretStore::new());
        let tm = TokenManager::new(&base_url).with_persistence(store.clone());
        {
            let mut state = tm.state.write().await;
            // An hour of headroom keeps `logout()`'s own `get_token()` from
            // starting a second, nested refresh.
            *state = Some(TokenState {
                access_token: "at-a".to_string(),
                refresh_token: Some("rt-a".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: Some("account_a".to_string()),
                organization_id: Some("org-a".to_string()),
            });
        }
        tm.persist_current_state().await;

        // Account A's background refresh reaches the server and parks there.
        let refresh_side = tm.clone();
        let refresh_task = tokio::spawn(async move { refresh_side.refresh().await });
        seen_rx
            .await
            .expect("the refresh must reach the stub server before the logout");

        // The user signs out of A and immediately signs back in as B, all while
        // A's rotated-token response is still outstanding.
        tm.logout().await.expect("logout must succeed");
        tm.login_with_org("account_b", &primary_password(), "org-b")
            .await
            .expect("the re-login as account B must succeed");
        assert_eq!(
            tm.session_info().await.and_then(|info| info.identifier),
            Some("account_b".to_string()),
            "precondition: the live session must be account B before A's refresh lands"
        );

        // Release account A's rotated-token response into account B's session.
        gate_tx.send(()).expect("gate receiver must still be alive");
        let refresh_error = refresh_task
            .await
            .expect("refresh task must not panic")
            .expect_err("a refresh whose session was replaced must not report success");
        assert!(
            matches!(&refresh_error, CoreError::Auth { .. }),
            "a superseded refresh must fail as an auth error, got: {refresh_error}"
        );
        assert!(
            refresh_error
                .to_string()
                .contains("session ended during refresh"),
            "the error must name the session-change race, got: {refresh_error}"
        );

        // In-memory session is untouched: still B, never A.
        let info = tm
            .session_info()
            .await
            .expect("account B's session must survive the superseded refresh");
        assert_eq!(info.identifier.as_deref(), Some("account_b"));
        assert_eq!(info.organization_id.as_deref(), Some("org-b"));
        assert_eq!(
            tm.get_token()
                .await
                .expect("account B's access token must still be served"),
            "at-b",
            "A's rotated access token must not have replaced B's"
        );

        // The keychain namespace holds account B's material only.
        for (key, expected) in [
            (ONESHIM_ACCESS_TOKEN_SECRET_KEY, "at-b"),
            (ONESHIM_REFRESH_TOKEN_SECRET_KEY, "rt-b"),
            (ONESHIM_IDENTIFIER_SECRET_KEY, "account_b"),
            (ONESHIM_ORGANIZATION_ID_SECRET_KEY, "org-b"),
        ] {
            assert_eq!(
                store
                    .retrieve(ONESHIM_AUTH_SECRET_NAMESPACE, key)
                    .await
                    .unwrap(),
                Some(expected.to_string()),
                "'{key}' must stay account B's, not be overwritten by A's stale refresh"
            );
        }

        // A later launch restores B, not A.
        let tm2 = TokenManager::new(&base_url).with_persistence(store.clone());
        assert!(
            tm2.restore_persisted_session().await,
            "account B's persisted session must restore"
        );
        let restored = tm2
            .session_info()
            .await
            .expect("session info after restoring account B");
        assert_eq!(restored.identifier.as_deref(), Some("account_b"));
        assert_eq!(restored.organization_id.as_deref(), Some("org-b"));
    }
}
