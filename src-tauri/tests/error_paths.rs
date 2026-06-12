#![cfg(feature = "server")]
#![allow(deprecated)]

use maekon_core::ports::storage::StorageService;
use maekon_network::auth::TokenManager;
use maekon_storage::sqlite::SqliteStorage;

#[tokio::test]
async fn auth_get_token_without_login() {
    let tm = TokenManager::new("http://localhost:9999");
    let result = tm.get_token().await;
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Authentication"));
}

#[tokio::test]
async fn auth_refresh_without_login() {
    let tm = TokenManager::new("http://localhost:9999");
    let result = tm.refresh().await;
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Authentication"));
}

#[tokio::test]
async fn auth_logout_without_login_is_ok() {
    let tm = TokenManager::new("http://localhost:9999");
    let result = tm.logout().await;
    // logout() must always succeed even without a prior login: the contract
    // (auth/mod.rs) calls get_token().ok() and skips the server DELETE when
    // no token is present, then clears local state.  The UX guarantee is that
    // after logout() the client is always in an unauthenticated state.
    // (#5594: ok-only IS the full contract here — no token to inspect)
    result.expect("logout without prior login must always return Ok");
}

#[tokio::test]
async fn storage_empty_mark_as_sent() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let result = storage.mark_as_sent(&[]).await;
    // mark_as_sent with an empty slice short-circuits to Ok(()) before opening a
    // connection (events.rs: `if event_ids.is_empty() { return Ok(()); }`).
    // (#5594: ok-only IS the full contract — empty-slice is a documented no-op)
    result.expect("mark_as_sent with an empty slice must be a no-op Ok");
}
