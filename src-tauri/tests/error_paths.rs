#![cfg(feature = "server")]
#![allow(deprecated)]

use oneshim_core::ports::storage::StorageService;
use oneshim_network::auth::TokenManager;
use oneshim_storage::sqlite::SqliteStorage;

#[tokio::test]
async fn auth_get_token_without_login() {
    let tm = TokenManager::new("http://localhost:9999");
    let result = tm.get_token().await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Authentication"));
}

#[tokio::test]
async fn auth_refresh_without_login() {
    let tm = TokenManager::new("http://localhost:9999");
    let result = tm.refresh().await;
    assert!(result.is_err());
    let err_msg = format!("{}", result.unwrap_err());
    assert!(err_msg.contains("Authentication"));
}

#[tokio::test]
async fn auth_logout_without_login_is_ok() {
    let tm = TokenManager::new("http://localhost:9999");
    let result = tm.logout().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn storage_empty_mark_as_sent() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let result = storage.mark_as_sent(&[]).await;
    assert!(result.is_ok());
}
