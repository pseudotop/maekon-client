//! #9459: construction of the ONE ONESHIM login session for this process.
//!
//! Before this wiring, the server-transport builder
//! (`agent_runtime_support::build_server_transports`) and the suggestion
//! pipeline (`suggestion_wiring::build_suggestion_manager`) each constructed
//! their own `TokenManager`, and only the latter ever reached the
//! `TokenManagerState` IPC slot. A sign-in through that slot therefore
//! authenticated a manager that no upload/REST/SSE transport consulted. The
//! composition root now builds one manager here, restores it from the OS
//! keychain, and hands the same `Arc` to both consumers.

use std::path::Path;
use std::sync::Arc;

use maekon_core::config::AppConfig;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::secret_store::SecretStore;
use maekon_network::auth::TokenManager;

/// Resolve the OS keychain store that durably backs the login session.
///
/// Reuses the composition root's existing keychain constructor
/// (`provider_secret_backend::create_os_secret_store`, also used by
/// `ServerBootstrapContext::build`) so the login session and the provider
/// credentials share one keychain registry file instead of two divergent ones.
///
/// `None` means the platform keychain is unavailable. That is a degradation,
/// not a failure: sign-in still works for the running process, the session just
/// does not survive a restart.
fn build_login_secret_store(data_dir_path: &Path) -> Option<Arc<dyn SecretStore>> {
    let config_dir = ConfigManager::config_dir().unwrap_or_else(|_| data_dir_path.to_path_buf());
    let store = crate::provider_secret_backend::create_os_secret_store(&config_dir);
    if store.is_none() {
        tracing::warn!(
            "OS keychain unavailable; the ONESHIM login session will not survive a restart \
             (sign-in still works for this run)"
        );
    }
    store
}

/// Build the process-wide login session and register it in the
/// `TokenManagerState` IPC slot, returning the same `Arc` for the wirings that
/// consume it.
///
/// Registration lives here rather than in the composition root so this module
/// owns the whole login-session concern (ADR-013), and so the slot write happens
/// exactly once — the suggestion wiring's own write is now conditional on this
/// one not having run.
pub(super) fn install_shared_token_manager(
    app_handle: &tauri::AppHandle,
    config: &AppConfig,
    handle: &tokio::runtime::Handle,
    data_dir_path: &Path,
) -> Option<Arc<TokenManager>> {
    let manager = build_shared_token_manager(config, handle, data_dir_path)?;

    // The slot is registered ONCE (empty) on the Tauri builder in `main.rs`;
    // writing through it is what makes the manager reachable from the auth IPC
    // commands. A second `Manager::manage()` would be a silent no-op — it does
    // not overwrite an already-managed type.
    use tauri::Manager;
    app_handle
        .state::<crate::commands::auth::TokenManagerState>()
        .set(manager.clone());

    install_context_home_client(app_handle, config, &manager);

    Some(manager)
}

/// Build the context-home transport off the shared session and register it in
/// the `ContextHomeState` IPC slot (#9625).
///
/// This lives here, next to the manager it consumes, for the reason stated in
/// the module doc: the failure this module exists to prevent is a transport
/// authenticating a `TokenManager` that no sign-in ever touches. Handing this
/// client the same `Arc` at the moment the shared session is registered is what
/// makes "one sign-in, every surface" true for the context home as well.
///
/// A TLS-config failure is logged and skipped rather than aborting: the login
/// session itself is already wired by this point, and blanking sign-in because
/// one read surface could not be built would be the larger regression. The slot
/// stays `None`, and the command answers `service.unavailable` — which is
/// exactly the state that error code describes.
fn install_context_home_client(
    app_handle: &tauri::AppHandle,
    config: &AppConfig,
    manager: &Arc<TokenManager>,
) {
    let client = match maekon_network::http_client::HttpApiClient::new_with_tls(
        &config.server.base_url,
        manager.clone(),
        config.request_timeout(),
        &config.tls,
    ) {
        Ok(client) => Arc::new(client),
        Err(error) => {
            tracing::error!(
                error = %error,
                "context-home transport construction failed; the context home will report \
                 service.unavailable this run (refusing to fall back to a client without TLS \
                 policy enforcement)"
            );
            return;
        }
    };

    use tauri::Manager;
    app_handle
        .state::<crate::commands::context_home::ContextHomeState>()
        .set(client.clone());
    app_handle
        .state::<crate::commands::assignment_email_draft::AssignmentEmailDraftState>()
        .set(client);
}

/// Build the process-wide `TokenManager` — keychain-backed, and restored from
/// the previous run's session.
///
/// Returns `None` on a `[tls]` *configuration* error, the same fail-loud
/// convention both former construction sites already used: a client without TLS
/// policy enforcement is a security regression, not a graceful degrade. The
/// callers keep their own local construction as a fallback, so a `None` here
/// never disables the app — it only means this run has no shared session.
fn build_shared_token_manager(
    config: &AppConfig,
    handle: &tokio::runtime::Handle,
    data_dir_path: &Path,
) -> Option<Arc<TokenManager>> {
    build_shared_token_manager_with_store(config, handle, build_login_secret_store(data_dir_path))
}

/// Store-injecting core of [`build_shared_token_manager`], split out so the
/// construct → attach-persistence → restore contract is unit-testable against a
/// temp-dir `FileSecretStore` rather than the developer's real OS keychain.
fn build_shared_token_manager_with_store(
    config: &AppConfig,
    handle: &tokio::runtime::Handle,
    store: Option<Arc<dyn SecretStore>>,
) -> Option<Arc<TokenManager>> {
    let manager = match TokenManager::new_with_tls(
        &config.server.base_url,
        &config.tls,
        Some(config.request_timeout()),
    ) {
        Ok(manager) => manager,
        Err(error) => {
            tracing::error!(
                error = %error,
                base_url = %config.server.base_url,
                "shared TokenManager construction failed; ONESHIM sign-in is unavailable this \
                 run (fail-loud — verify the [tls] config; refusing to fall back to a client \
                 without TLS policy enforcement)"
            );
            return None;
        }
    };

    // `with_persistence` consumes `self`, so it must be applied before the Arc wrap.
    let manager = match store {
        Some(store) => manager.with_persistence(store),
        None => manager,
    };
    let manager = Arc::new(manager);

    // `restored` means "usable session material was found", NOT "ready to call
    // the server": a restored session whose access token already expired stays
    // `is_authenticated() == false` until the first `get_token()` refreshes it.
    let restored = handle.block_on(manager.restore_persisted_session());
    tracing::info!(restored, "ONESHIM login session restore attempted");

    Some(manager)
}

#[cfg(test)]
mod tests {
    use maekon_core::ports::secret_store::{
        ONESHIM_ACCESS_TOKEN_SECRET_KEY, ONESHIM_AUTH_SECRET_NAMESPACE,
        ONESHIM_EXPIRES_AT_SECRET_KEY,
    };

    use super::*;

    fn temp_secret_store(dir: &Path) -> Arc<dyn SecretStore> {
        Arc::new(
            maekon_storage::file_secret_store::FileSecretStore::new(
                dir.join("secrets.json"),
                Some(Arc::new(
                    maekon_storage::encryption::EncryptionKey::from_bytes([0x42; 32]),
                )),
            )
            .expect("file secret store must build in a temp dir"),
        )
    }

    /// A `[tls]` configuration error must fail loud (`None`), never yield a
    /// manager built without TLS policy enforcement — the convention both
    /// pre-#9459 construction sites already followed.
    #[test]
    fn invalid_tls_config_yields_no_shared_manager() {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        let mut config = AppConfig::default_config();
        config.tls.allow_self_signed = true;

        let manager = build_shared_token_manager_with_store(&config, runtime.handle(), None);

        assert!(
            manager.is_none(),
            "an invalid [tls] config must fail loud (None), not silently build a shared \
             TokenManager without TLS policy enforcement"
        );
    }

    /// Persistence is a degradable extra, not a precondition: with no keychain
    /// available the manager is still built so the user can sign in for this run.
    #[test]
    fn manager_builds_without_a_secret_store() {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        let config = AppConfig::default_config();

        let manager = build_shared_token_manager_with_store(&config, runtime.handle(), None);

        assert!(
            manager.is_some(),
            "an unavailable keychain must degrade to an in-memory session, not block sign-in"
        );
    }

    /// #9459: the bootstrap must actually restore. A session already in the
    /// store is authenticated by the time the composition root hands the manager
    /// out, so the first upload after a restart carries a bearer token with no
    /// re-login.
    #[test]
    fn persisted_session_is_restored_at_bootstrap() {
        let runtime = tokio::runtime::Runtime::new().expect("test runtime");
        let temp = tempfile::TempDir::new().expect("temp dir");
        let store = temp_secret_store(temp.path());
        let expires_at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        runtime.block_on(async {
            store
                .store(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_ACCESS_TOKEN_SECRET_KEY,
                    "restored-access-token",
                )
                .await
                .expect("seeding the access token must succeed");
            store
                .store(
                    ONESHIM_AUTH_SECRET_NAMESPACE,
                    ONESHIM_EXPIRES_AT_SECRET_KEY,
                    &expires_at,
                )
                .await
                .expect("seeding the expiry must succeed");
        });
        let config = AppConfig::default_config();

        let manager = build_shared_token_manager_with_store(&config, runtime.handle(), Some(store))
            .expect("a valid [tls] config must build the shared manager");

        assert!(
            runtime.block_on(manager.is_authenticated()),
            "the persisted session must be restored during bootstrap — otherwise every \
             restart silently signs the user out"
        );
    }
}
