//! Unit + integration tests for `TokenManager`.
//!
//! All tests live here so that `mod.rs` / `tokens.rs` / `refresh.rs` stay
//! under the 300-LOC ADR-013 target.

#[cfg(test)]
#[allow(deprecated)]
#[allow(clippy::module_inception)]
mod tests {
    use std::time::Duration as StdDuration;

    use chrono::{Duration, Utc};
    use maekon_core::error::CoreError;

    use crate::auth::refresh::{
        parse_retry_after, REFRESH_INITIAL_BACKOFF_MS, REFRESH_MAX_BACKOFF_MS, REFRESH_MAX_RETRIES,
    };
    use crate::auth::tokens::{TokenManager, TokenState, MAX_TOKEN_TTL_SECS};
    use maekon_http_core::resilience::jittered_backoff_delay;

    // ── Password fixtures ─────────────────────────────────────────────────────

    fn password_fixture(byte: u8, len: usize) -> String {
        String::from_utf8(vec![byte; len]).expect("password fixture bytes must be UTF-8")
    }

    fn primary_password() -> String {
        password_fixture(b'x', 16)
    }

    fn alternate_password() -> String {
        password_fixture(b'y', 16)
    }

    fn short_password() -> String {
        password_fixture(b'z', 1)
    }

    // ── parse_retry_after ─────────────────────────────────────────────────────

    #[test]
    fn parse_retry_after_valid_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "30".parse().unwrap());
        let duration = parse_retry_after(&headers);
        assert_eq!(duration, Some(StdDuration::from_secs(30)));
    }

    #[test]
    fn parse_retry_after_caps_at_60() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "120".parse().unwrap());
        let duration = parse_retry_after(&headers);
        assert_eq!(duration, Some(StdDuration::from_secs(60)));
    }

    #[test]
    fn parse_retry_after_missing_header() {
        let headers = reqwest::header::HeaderMap::new();
        assert!(parse_retry_after(&headers).is_none());
    }

    #[test]
    fn parse_retry_after_non_integer_ignored() {
        let mut headers = reqwest::header::HeaderMap::new();
        // HTTP-date format is not supported — returns None
        headers.insert(
            "retry-after",
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        assert!(parse_retry_after(&headers).is_none());
    }

    // ── refresh() backoff envelope (#7725: adopted `resilience::jittered_backoff_delay`,
    // replacing the hand-rolled `(INITIAL_BACKOFF_MS * 2u64.pow(attempt)).min(MAX_BACKOFF_MS)`
    // doubling — found during the #7725 fix-class completeness sweep) ──────────

    #[test]
    fn refresh_backoff_constants_match_pre_migration_tuning() {
        assert_eq!(REFRESH_MAX_RETRIES, 3);
        assert_eq!(REFRESH_INITIAL_BACKOFF_MS, 500);
        assert_eq!(REFRESH_MAX_BACKOFF_MS, 8_000);
    }

    #[test]
    fn refresh_backoff_delay_envelope_matches_pre_migration_doubling_schedule() {
        let base = StdDuration::from_millis(REFRESH_INITIAL_BACKOFF_MS);
        let max = StdDuration::from_millis(REFRESH_MAX_BACKOFF_MS);
        // Without jitter the pre-migration schedule doubled 500ms -> 1000ms ->
        // 2000ms -> 4000ms; pin the envelope (floor = bare exponential,
        // ceiling = exponential + 25% jitter) rather than an exact value since
        // the delay is now randomized.
        for (attempt, exp_ms) in [(0u32, 500u64), (1, 1000), (2, 2000)] {
            let floor = StdDuration::from_millis(exp_ms);
            let ceiling = StdDuration::from_millis(exp_ms + exp_ms / 4);
            for _ in 0..50 {
                let delay = jittered_backoff_delay(attempt, base, max);
                assert!(
                    delay >= floor,
                    "attempt {attempt}: delay {delay:?} must be >= exponential floor {floor:?}"
                );
                assert!(
                    delay <= ceiling,
                    "attempt {attempt}: delay {delay:?} must be <= exp + 25% ({ceiling:?})"
                );
            }
        }
    }

    #[test]
    fn refresh_backoff_delay_never_exceeds_max_for_full_retry_range() {
        let base = StdDuration::from_millis(REFRESH_INITIAL_BACKOFF_MS);
        let max = StdDuration::from_millis(REFRESH_MAX_BACKOFF_MS);
        for attempt in 0..REFRESH_MAX_RETRIES {
            let delay = jittered_backoff_delay(attempt, base, max);
            assert!(
                delay <= max,
                "attempt {attempt}: delay {delay:?} must be <= {max:?}"
            );
        }
    }

    // ── Constructor smoke tests ───────────────────────────────────────────────

    #[test]
    fn token_manager_creation() {
        let tm = TokenManager::new("http://localhost:8000");
        assert_eq!(tm.base_url, "http://localhost:8000");
    }

    #[test]
    fn token_manager_trailing_slash() {
        let tm = TokenManager::new("http://localhost:8000/");
        assert_eq!(tm.base_url, "http://localhost:8000");
    }

    #[test]
    fn new_with_client_strips_trailing_slash() {
        let client = reqwest::Client::new();
        let tm = TokenManager::new_with_client("http://localhost:8000/", client);
        assert_eq!(tm.base_url, "http://localhost:8000");
    }

    #[test]
    fn new_with_tls_tls_disabled_succeeds() {
        let tls = maekon_core::config::TlsConfig {
            enabled: false,
            allow_self_signed: false,
        };
        let tm = TokenManager::new_with_tls(
            "http://localhost:8000",
            &tls,
            Some(std::time::Duration::from_secs(5)),
        )
        .expect("TLS disabled must succeed for http:// URL");
        assert_eq!(
            tm.base_url, "http://localhost:8000",
            "base_url must be stored verbatim (no trailing slash)"
        );
    }

    #[test]
    fn new_with_tls_tls_enabled_succeeds() {
        let tls = maekon_core::config::TlsConfig::default(); // enabled=true, allow_self_signed=false
        let tm = TokenManager::new_with_tls(
            "https://api.example.com",
            &tls,
            Some(std::time::Duration::from_secs(5)),
        )
        .expect("TLS enabled + https:// URL must construct without error");
        assert_eq!(
            tm.base_url, "https://api.example.com",
            "base_url must be stored verbatim with trailing slash stripped"
        );
    }

    // ── Unauthenticated state ─────────────────────────────────────────────────

    #[tokio::test]
    async fn unauthenticated_get_token_fails() {
        let tm = TokenManager::new("http://localhost:8000");
        let err = tm.get_token().await.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    ..
                }
            ),
            "unauthenticated get_token must return CoreError::Auth::Failed, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn unauthenticated_state() {
        let tm = TokenManager::new("http://localhost:8000");
        assert!(!tm.is_authenticated().await);
    }

    // ── Login ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn login_success() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"jwt_abc","refresh_token":"ref_xyz","expires_in":3600}"#)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        tm.login("user@test.com", &password)
            .await
            .expect("login must succeed on 200 with valid token JSON");
        // After successful login the manager must report authenticated.
        assert!(
            tm.is_authenticated().await,
            "is_authenticated must return true after successful login"
        );
        mock.assert_async().await;
    }

    /// #6201 regression: a server-controlled `expires_in` of `i64::MAX` must
    /// not panic the login path (`Duration::seconds()` overflow / `Utc::now() +
    /// Duration` overflow) and must clamp the stored expiry to `MAX_TOKEN_TTL_SECS`.
    #[tokio::test]
    async fn login_clamps_adversarial_expires_in() {
        let mut server = mockito::Server::new_async().await;
        let body = format!(
            r#"{{"access_token":"jwt_abc","refresh_token":"ref_xyz","expires_in":{}}}"#,
            i64::MAX
        );
        let mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        let before = Utc::now();
        // Must not panic on the overflowing TTL.
        tm.login("user@test.com", &password)
            .await
            .expect("login must succeed even with an adversarial expires_in");

        // Expiry must be clamped to the 30-day cap, not the overflowing value.
        let expires_at = {
            let state = tm.state.read().await;
            state
                .as_ref()
                .expect("state populated after login")
                .expires_at
        };
        let upper = before + Duration::seconds(MAX_TOKEN_TTL_SECS) + Duration::seconds(5);
        assert!(
            expires_at <= upper,
            "expires_at {expires_at} must be clamped to the {MAX_TOKEN_TTL_SECS}s cap"
        );
        assert!(
            tm.is_authenticated().await,
            "clamped token must still be valid"
        );
        mock.assert_async().await;
    }

    /// #6201 regression: the refresh path must apply the same TTL clamp as login
    /// so an adversarial `expires_in` cannot panic `Duration::seconds()`.
    #[tokio::test]
    async fn refresh_clamps_adversarial_expires_in() {
        let mut server = mockito::Server::new_async().await;
        let body = format!(
            r#"{{"access_token":"new_jwt","refresh_token":"new_ref","expires_in":{}}}"#,
            i64::MAX
        );
        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        // Seed state directly so we don't need a login round-trip.
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: None,
                organization_id: None,
            });
        }

        let before = Utc::now();
        // Must not panic on the overflowing TTL.
        tm.refresh()
            .await
            .expect("refresh must succeed even with an adversarial expires_in");

        let expires_at = {
            let state = tm.state.read().await;
            state
                .as_ref()
                .expect("state populated after refresh")
                .expires_at
        };
        let upper = before + Duration::seconds(MAX_TOKEN_TTL_SECS) + Duration::seconds(5);
        assert!(
            expires_at <= upper,
            "refresh expires_at {expires_at} must be clamped to the {MAX_TOKEN_TTL_SECS}s cap"
        );
        refresh_mock.assert_async().await;
    }

    #[tokio::test]
    async fn login_failure_401() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = alternate_password();
        let result = tm.login("bad@test.com", &password).await;
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("login failure"));
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn login_failure_network() {
        let tm = TokenManager::new("http://127.0.0.1:1");
        let password = primary_password();
        let result = tm.login("user@test.com", &password).await;
        assert!(result.is_err()); // lint:allow-is-err-hedge — OS-level connection refusal maps to platform-dependent network error
    }

    // ── Logout ────────────────────────────────────────────────────────────────

    // OOS-N15 (2026-05-05): logout + logout_all_sessions tests
    #[tokio::test]
    async fn logout_clears_state_and_calls_server() {
        let mut server = mockito::Server::new_async().await;
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"jwt_abc","refresh_token":"ref_xyz","expires_in":3600}"#)
            .create_async()
            .await;
        let logout_mock = server
            .mock("DELETE", "/api/v1/auth/tokens")
            .match_header("authorization", "Bearer jwt_abc")
            .with_status(200)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        tm.login("user@test.com", &password).await.unwrap();
        assert!(tm.is_authenticated().await);

        tm.logout().await.unwrap();
        assert!(!tm.is_authenticated().await);

        login_mock.assert_async().await;
        logout_mock.assert_async().await;
    }

    #[tokio::test]
    async fn logout_all_sessions_calls_tokens_all_and_clears_state() {
        let mut server = mockito::Server::new_async().await;
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"jwt_abc","refresh_token":"ref_xyz","expires_in":3600}"#)
            .create_async()
            .await;
        let logout_all_mock = server
            .mock("DELETE", "/api/v1/auth/tokens/all")
            .match_header("authorization", "Bearer jwt_abc")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"success":true,"message":"all logged out","sessions_terminated":3}"#)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        tm.login("user@test.com", &password).await.unwrap();
        assert!(tm.is_authenticated().await);

        tm.logout_all_sessions().await.unwrap();
        assert!(!tm.is_authenticated().await);

        login_mock.assert_async().await;
        logout_all_mock.assert_async().await;
    }

    #[tokio::test]
    async fn logout_all_sessions_clears_state_even_when_server_fails() {
        let mut server = mockito::Server::new_async().await;
        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"jwt_abc","refresh_token":"ref_xyz","expires_in":3600}"#)
            .create_async()
            .await;
        // server returns 500 — local state still cleared (UX consistency, matches logout policy)
        let logout_all_mock = server
            .mock("DELETE", "/api/v1/auth/tokens/all")
            .match_header("authorization", "Bearer jwt_abc")
            .with_status(500)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        tm.login("user@test.com", &password).await.unwrap();
        assert!(tm.is_authenticated().await);

        // Server failure still clears local state + returns Ok (same policy as logout)
        tm.logout_all_sessions().await.unwrap();
        assert!(!tm.is_authenticated().await);

        login_mock.assert_async().await;
        logout_all_mock.assert_async().await;
    }

    // ── Refresh ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn refresh_without_auth_fails() {
        let tm = TokenManager::new("http://localhost:9999");
        let result = tm.refresh().await;
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("Not authenticated"));
    }

    #[tokio::test]
    async fn refresh_success() {
        let mut server = mockito::Server::new_async().await;

        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"old_jwt","refresh_token":"ref_tok","expires_in":3600}"#)
            .create_async()
            .await;

        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new_jwt","refresh_token":"new_ref","expires_in":7200}"#)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        tm.login("user@test.com", &password).await.unwrap();
        login_mock.assert_async().await;

        tm.refresh()
            .await
            .expect("refresh must succeed on 200 with valid token JSON");
        // Verify the token was actually updated to the refreshed value.
        let token = tm
            .get_token()
            .await
            .expect("get_token must succeed after successful refresh");
        assert_eq!(
            token, "new_jwt",
            "access token must be updated to refreshed value"
        );
        refresh_mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_token_propagates_refresh_failure() {
        let mut server = mockito::Server::new_async().await;

        let login_mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"old_jwt","refresh_token":"ref_tok","expires_in":1}"#)
            .create_async()
            .await;

        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(401)
            .with_body("Unauthorized")
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        let password = primary_password();
        tm.login("user@test.com", &password).await.unwrap();
        login_mock.assert_async().await;

        let result = tm.get_token().await;
        assert!(format!("{}", result.unwrap_err()).contains("Automatic token refresh failed"));
        refresh_mock.assert_async().await;
    }

    /// Net-7 regression: concurrent `get_token` calls near expiry must collapse
    /// into a single `/refresh` round-trip (single-flight).  Without the
    /// `refresh_lock` guard each caller races to refresh with the same refresh
    /// token, which a rotating server rejects on the second use.
    #[tokio::test]
    async fn concurrent_get_token_single_flights_refresh() {
        let mut server = mockito::Server::new_async().await;

        // expect(1): exactly one refresh must fire even under a concurrent herd.
        let refresh_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new_jwt","refresh_token":"new_ref","expires_in":7200}"#)
            .expect(1)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        // Seed a near-expiry token directly (inside the 5-minute refresh window).
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::minutes(1),
                identifier: None,
                organization_id: None,
            });
        }

        // Fire many concurrent get_token() calls; all should observe the same
        // freshly-refreshed token without each triggering its own refresh.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let tm = tm.clone();
            handles.push(tokio::spawn(async move { tm.get_token().await }));
        }
        for handle in handles {
            let token = handle.await.expect("task join").expect("get_token");
            assert_eq!(token, "new_jwt");
        }

        refresh_mock.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_retries_on_transient_failure() {
        let mut server = mockito::Server::new_async().await;

        // mockito prioritises mocks with missing hits (below their expect count),
        // then falls back to the last matching mock. Register the failure mock
        // first with expect(2) so it absorbs the first two requests, then the
        // success mock (registered second = last) handles the third.
        let failure_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(503)
            .with_body("Service Unavailable")
            .expect(2)
            .create_async()
            .await;

        let success_mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new_jwt","refresh_token":"new_ref","expires_in":7200}"#)
            .expect(1)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        // Seed state directly so we don't need a login mock
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: None,
                organization_id: None,
            });
        }

        tm.refresh()
            .await
            .expect("refresh must succeed after retrying past two transient 503s");
        // Pin that the access token was actually updated by the successful retry.
        let token = tm
            .get_token()
            .await
            .expect("get_token must succeed after successful retry-based refresh");
        assert_eq!(
            token, "new_jwt",
            "access token must be updated to the value from the successful (3rd) attempt"
        );
        failure_mock.assert_async().await;
        success_mock.assert_async().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_gives_up_after_max_retries() {
        let mut server = mockito::Server::new_async().await;

        // Always return 503 — should exhaust all 4 attempts (initial + 3 retries)
        let mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(503)
            .with_body("Service Unavailable")
            .expect(4)
            .create_async()
            .await;

        let tm = TokenManager::new(&server.url());
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: None,
                organization_id: None,
            });
        }

        let result = tm.refresh().await;
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("token refresh failure"));
        mock.assert_async().await;
    }

    // ── Login HTTP-status mapping (iter-70 regression guards) ─────────────────

    // iter-70 regression guards for iter-61a semantic HTTP status mapping
    // in login(). Shared helper pattern matches iter-67..69.
    async fn run_login_status_test(status: u16) -> maekon_core::error::CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;
        let tm = TokenManager::new(&server.url());
        let password = short_password();
        tm.login("u@test.com", &password).await.unwrap_err()
    }

    #[tokio::test]
    async fn login_429_maps_to_rate_limit() {
        let err = run_login_status_test(429).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn login_503_maps_to_service_unavailable() {
        let err = run_login_status_test(503).await;
        assert!(
            matches!(
                err,
                maekon_core::error::CoreError::ServiceUnavailable { .. }
            ),
            "503 → ServiceUnavailable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn login_504_maps_to_timeout() {
        let err = run_login_status_test(504).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::RequestTimeout { .. }),
            "504 → RequestTimeout, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn login_401_stays_as_auth() {
        // Sanity check: 401 (the "normal" login failure) still maps to
        // CoreError::Auth so iter-61a didn't regress the common case.
        let err = run_login_status_test(401).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::Auth { .. }),
            "401 → Auth, got: {err:?}"
        );
    }

    /// iter-79: domain-fallback guard for login. 500 (not in specific arms)
    /// falls back to CoreError::Auth/Failed — the login-endpoint-appropriate
    /// wildcard. Catches regressions that broaden the transient-class arms
    /// (429/502/503/504) into the 5xx space.
    #[tokio::test]
    async fn login_500_falls_back_to_auth() {
        let err = run_login_status_test(500).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::Auth { .. }),
            "500 should fall back to Auth (domain-appropriate for login endpoint), got: {err:?}"
        );
    }

    // ── Refresh HTTP-status mapping (iter-98 regression guards) ──────────────

    // iter-98 regression guards: refresh() must apply canonical HTTP status
    // mapping, not blanket-label every error as CoreError::Auth.
    async fn run_refresh_status_test(status: u16) -> maekon_core::error::CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/v1/auth/tokens/refresh")
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .expect_at_least(1)
            .create_async()
            .await;
        let tm = TokenManager::new(&server.url());
        {
            let mut state = tm.state.write().await;
            *state = Some(TokenState {
                access_token: "old_jwt".to_string(),
                refresh_token: Some("ref_tok".to_string()),
                expires_at: Utc::now() + Duration::hours(1),
                identifier: None,
                organization_id: None,
            });
        }
        tm.refresh().await.unwrap_err()
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_503_maps_to_service_unavailable() {
        let err = run_refresh_status_test(503).await;
        assert_eq!(err.code(), "service.unavailable");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_429_maps_to_rate_limit() {
        let err = run_refresh_status_test(429).await;
        assert_eq!(err.code(), "network.rate_limit");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_504_maps_to_timeout() {
        let err = run_refresh_status_test(504).await;
        assert_eq!(err.code(), "network.timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_401_maps_to_auth() {
        let err = run_refresh_status_test(401).await;
        assert_eq!(err.code(), "auth.failed");
    }

    /// Domain-fallback: 500 falls back to Auth (refresh is auth-domain).
    #[tokio::test(start_paused = true)]
    async fn refresh_500_falls_back_to_auth() {
        let err = run_refresh_status_test(500).await;
        assert_eq!(err.code(), "auth.failed");
    }
}
