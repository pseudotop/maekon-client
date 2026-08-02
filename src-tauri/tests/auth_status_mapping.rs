#![cfg(feature = "server")]
#![allow(deprecated)] // TokenManager::new (non-TLS) is the test constructor
//! #9492 item 3 — `auth_status_inner` authenticated-path field mapping.
//!
//! The in-module unit tests in `src/commands/auth.rs` only ever exercise the
//! signed-out branch, where `identifier` and `organization_id` are both `None`
//! — so swapping the two assignments in
//!
//! ```ignore
//! identifier: info.as_ref().and_then(|i| i.identifier.clone()),
//! organization_id: info.and_then(|i| i.organization_id),
//! ```
//!
//! survived the whole suite while the Settings Account section would render
//! "Signed in as org-e2e-futurepac @ mingyu_song".
//!
//! This test drives the authenticated branch for real: it logs a live
//! `TokenManager` in against the shared mock server's
//! `POST /api/v1/auth/tokens` route, then asserts each value lands in its own
//! field. The two fixture values are deliberately non-interchangeable, which is
//! what makes the swap detectable.
//!
//! ```
//! cargo test -p maekon-app --features server --test auth_status_mapping
//! ```

mod mock_server;

use std::sync::Arc;

use maekon_app::commands::auth::{auth_status_inner, TokenManagerState};
use maekon_network::auth::TokenManager;
use mock_server::MockServer;
use uuid::Uuid;

/// Fixture identity. Distinct shapes (no shared prefix, different lengths) so a
/// transposition cannot pass by coincidence.
const FIXTURE_IDENTIFIER: &str = "mingyu_song";
const FIXTURE_ORGANIZATION_ID: &str = "org-e2e-futurepac";

/// Per-call unique test password. Avoids CodeQL
/// `rust/hard-coded-cryptographic-value` flagging a static fixture string.
fn fixture_password() -> String {
    format!("test-pwd-{}", Uuid::new_v4().simple())
}

#[tokio::test]
async fn auth_status_maps_session_metadata_to_its_own_fields_after_login() {
    let server = MockServer::start().await;

    let manager = TokenManager::new(server.url());
    manager
        .login_with_org(
            FIXTURE_IDENTIFIER,
            &fixture_password(),
            FIXTURE_ORGANIZATION_ID,
        )
        .await
        .expect("mock login route must accept the fixture credentials");

    // The request side: `TokenManager` seeds its session metadata from the
    // arguments it was called with, so a swap while *building* the request body
    // would be invisible to the response assertions below.
    let recorded = server
        .last_login()
        .expect("the mock server must have observed a login request");
    assert_eq!(
        recorded.identifier, FIXTURE_IDENTIFIER,
        "the login request's `identifier` field must carry the identifier"
    );
    assert_eq!(
        recorded.organization_id.as_deref(),
        Some(FIXTURE_ORGANIZATION_ID),
        "the login request's `organization_id` field must carry the organization id"
    );

    let state = TokenManagerState::empty();
    state.set(Arc::new(manager));

    let status = auth_status_inner(&state).await;

    assert!(
        status.server_feature,
        "a `server`-feature build must report the connected mode as available"
    );
    assert!(
        status.authenticated,
        "a live access token must be reported as authenticated"
    );
    assert_eq!(
        status.identifier.as_deref(),
        Some(FIXTURE_IDENTIFIER),
        "`identifier` must carry the login identifier, not the organization id"
    );
    assert_eq!(
        status.organization_id.as_deref(),
        Some(FIXTURE_ORGANIZATION_ID),
        "`organization_id` must carry the organization id, not the identifier"
    );

    // Close the loop to the shape the webview actually reads: the frontend
    // consumes these snake_case JSON keys verbatim (`GeneralTab.tsx`'s
    // `AuthStatus`), so a correct struct with a wrong serialization is still a
    // broken Settings Account section.
    let wire = serde_json::to_value(&status).expect("AuthStatusResponse must serialize");
    assert_eq!(
        wire["identifier"],
        serde_json::json!(FIXTURE_IDENTIFIER),
        "wire key `identifier` must carry the login identifier"
    );
    assert_eq!(
        wire["organization_id"],
        serde_json::json!(FIXTURE_ORGANIZATION_ID),
        "wire key `organization_id` must carry the organization id"
    );
    assert_eq!(wire["authenticated"], serde_json::json!(true));
}

#[tokio::test]
async fn auth_status_stops_reporting_session_metadata_after_logout() {
    // Complements the mapping assertion above: once the session is gone the
    // metadata must go with it, so a stale identifier cannot keep advertising a
    // signed-in account on a client that no longer holds a token.
    let server = MockServer::start().await;

    let manager = TokenManager::new(server.url());
    manager
        .login_with_org(
            FIXTURE_IDENTIFIER,
            &fixture_password(),
            FIXTURE_ORGANIZATION_ID,
        )
        .await
        .expect("mock login route must accept the fixture credentials");

    let state = TokenManagerState::empty();
    state.set(Arc::new(manager.clone()));
    assert!(
        auth_status_inner(&state).await.authenticated,
        "precondition: the freshly logged-in manager reports a live session"
    );

    // `logout` clears local state unconditionally; the mock server has no
    // matching route, which is exactly the "server unreachable" case it must
    // survive.
    manager
        .logout()
        .await
        .expect("logout clears local state and reports success regardless of server reachability");

    let status = auth_status_inner(&state).await;
    assert!(
        !status.authenticated,
        "a cleared token store must report signed out"
    );
    assert_eq!(
        status.identifier, None,
        "the previous identifier must not survive logout"
    );
    assert_eq!(
        status.organization_id, None,
        "the previous organization id must not survive logout"
    );
    assert!(
        status.server_feature,
        "`server_feature` is a build property and cannot change at runtime"
    );
}
