//! GUI HITL ticket HMAC secret provisioning (issue #7916, Epic #7908 T4.2).
//!
//! The GUI HITL trust core (`maekon_automation::gui_interaction`) signs
//! execution tickets with an HMAC secret and *fails closed* when that secret is
//! absent (`require_hmac_secret`). Historically the secret came ONLY from the
//! `MAEKON_GUI_TICKET_HMAC_SECRET` env var, which nothing ever generated — so on
//! a fresh install the entire GUI HITL flow was unreachable even though the
//! backend routes, overlay driver, and trust core were all complete.
//!
//! This module auto-provisions the secret by reusing the EXISTING OS-keychain
//! secret-store machinery (the same [`SecretStore`] backend that holds
//! provider/BYOK API keys — see `provider_secret_backend::create_os_secret_store`).
//! The secret is generated once on first need and persisted for later launches.
//!
//! ## Precedence (documented contract)
//! 1. An explicit, non-empty `MAEKON_GUI_TICKET_HMAC_SECRET` env var ALWAYS wins.
//!    This keeps the power-user / test / benchmark path byte-for-byte intact and
//!    lets it override whatever is (or is not) in the keychain.
//! 2. Otherwise: load the secret from the keychain; if it is absent, generate a
//!    new cryptographically-random secret, persist it, and use it.
//!
//! ## Fail-closed preservation (security anchor)
//! If the env var is unset/empty AND the keychain is unavailable, its read
//! errors, or the write of a freshly-generated secret fails, this returns
//! `None`. The trust core then refuses exactly as it did before — this module
//! NEVER weakens `require_hmac_secret`'s refusal path. It only provisions and
//! hands off an `Option<String>`; it does NOT touch ticket verification
//! semantics (constant-time compares, nonce burn, TTL, focus-drift binding all
//! remain in `gui_interaction`, unchanged).

use std::sync::Arc;

use maekon_core::ports::secret_store::SecretStore;
use tokio::runtime::Handle;
use tracing::{debug, warn};

/// Env var that overrides the keychain-provisioned secret. Kept in sync with
/// `maekon_automation::gui_interaction`'s `GUI_HMAC_SECRET_ENV` (the trust core
/// only uses that constant for an error message; the actual env read happens
/// here, at the wiring boundary).
pub const GUI_TICKET_HMAC_SECRET_ENV: &str = "MAEKON_GUI_TICKET_HMAC_SECRET";

/// Keychain namespace for the GUI HITL ticket secret. Mirrors the
/// `provider/<id>/<profile>` shape used by provider credentials; the keychain
/// account resolves to `"gui/ticket/default.hmac_secret"`.
pub const GUI_TICKET_SECRET_NAMESPACE: &str = "gui/ticket/default";
/// Keychain key name within [`GUI_TICKET_SECRET_NAMESPACE`].
pub const GUI_TICKET_SECRET_KEY: &str = "hmac_secret";

/// Byte length of a freshly generated secret before hex encoding. 32 bytes =
/// 256 bits, matching HMAC-SHA256's key strength; hex-encoded it becomes a
/// 64-character ASCII string that is safe to store in every OS keychain backend.
const GENERATED_SECRET_BYTES: usize = 32;

/// Resolve the GUI ticket HMAC secret, honoring the env override first and then
/// falling back to keychain auto-provisioning.
///
/// `store_factory` is invoked lazily — only when the env override is absent — so
/// the fast path never constructs (or touches) the OS keychain. Returns `None`
/// only when no secret can be obtained, preserving the trust core's fail-closed
/// refusal.
pub fn resolve_gui_ticket_hmac_secret<F>(
    runtime_handle: &Handle,
    store_factory: F,
) -> Option<String>
where
    F: FnOnce() -> Option<Arc<dyn SecretStore>>,
{
    resolve_gui_ticket_hmac_secret_inner(
        std::env::var(GUI_TICKET_HMAC_SECRET_ENV).ok(),
        runtime_handle,
        store_factory,
    )
}

/// Env-agnostic core of [`resolve_gui_ticket_hmac_secret`], split out so tests
/// can drive the override precedence and fail-closed behavior without mutating
/// the global process environment or touching the real OS keychain.
fn resolve_gui_ticket_hmac_secret_inner<F>(
    env_value: Option<String>,
    runtime_handle: &Handle,
    store_factory: F,
) -> Option<String>
where
    F: FnOnce() -> Option<Arc<dyn SecretStore>>,
{
    // 1. Explicit env override wins (power-user / test / benchmark path).
    if let Some(value) = env_value {
        if !value.trim().is_empty() {
            debug!("GUI ticket secret: using explicit env override");
            return Some(value);
        }
    }

    // 2. Keychain load-or-generate. `None` here (no backend, e.g. headless
    //    Linux without a Secret Service) leaves the trust core fail-closed.
    let store = store_factory()?;
    // The controller builder runs on the synchronous launch path (before the
    // async runtime is entered), so blocking on the shared handle is safe and
    // matches the surrounding `app_runtime_launch` wiring.
    runtime_handle.block_on(provision_from_store(store.as_ref()))
}

/// Load the secret from the store, generating and persisting a fresh one when it
/// is absent. Any store error is treated as fail-closed (returns `None`).
async fn provision_from_store(store: &dyn SecretStore) -> Option<String> {
    match store
        .retrieve(GUI_TICKET_SECRET_NAMESPACE, GUI_TICKET_SECRET_KEY)
        .await
    {
        Ok(Some(existing)) if !existing.trim().is_empty() => {
            debug!("GUI ticket secret: loaded existing secret from keychain");
            Some(existing)
        }
        Ok(_) => generate_and_persist(store).await,
        Err(error) => {
            // An ambiguous read (backend hiccup) must NOT trigger a rotate: we
            // refuse to overwrite a possibly-present secret and fail closed.
            warn!(%error, "GUI ticket secret: keychain read failed; refusing to provision (fail-closed)");
            None
        }
    }
}

/// Generate a fresh secret and persist it, returning it on success. A write
/// failure is fail-closed (`None`).
async fn generate_and_persist(store: &dyn SecretStore) -> Option<String> {
    let secret = generate_gui_ticket_secret();
    match store
        .store(GUI_TICKET_SECRET_NAMESPACE, GUI_TICKET_SECRET_KEY, &secret)
        .await
    {
        Ok(()) => {
            debug!("GUI ticket secret: generated and persisted a new secret to keychain");
            Some(secret)
        }
        Err(error) => {
            warn!(%error, "GUI ticket secret: keychain write failed; fail-closed");
            None
        }
    }
}

/// Generate a cryptographically-random 256-bit secret, hex-encoded.
///
/// Uses `rand::random()` (a ChaCha-based CSPRNG seeded from the OS) — the same
/// idiom `app_runtime_launch::launch_result` uses for its 32-byte token — so no
/// new randomness dependency is introduced. Hex encoding keeps the value ASCII
/// and keychain-safe.
fn generate_gui_ticket_secret() -> String {
    use std::fmt::Write as _;
    let bytes: [u8; GENERATED_SECRET_BYTES] = rand::random();
    let mut hex = String::with_capacity(GENERATED_SECRET_BYTES * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory secret store fake — mirrors the `TestSecretStore` doubles used
    /// across the workspace, keeping the real OS keychain out of unit tests.
    struct InMemoryStore {
        values: Mutex<HashMap<(String, String), String>>,
    }

    impl InMemoryStore {
        fn new() -> Self {
            Self {
                values: Mutex::new(HashMap::new()),
            }
        }

        fn seeded(namespace: &str, key: &str, value: &str) -> Self {
            let store = Self::new();
            store
                .values
                .lock()
                .unwrap()
                .insert((namespace.to_string(), key.to_string()), value.to_string());
            store
        }

        fn get(&self, namespace: &str, key: &str) -> Option<String> {
            self.values
                .lock()
                .unwrap()
                .get(&(namespace.to_string(), key.to_string()))
                .cloned()
        }
    }

    #[async_trait]
    impl SecretStore for InMemoryStore {
        async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
            self.values
                .lock()
                .unwrap()
                .insert((namespace.to_string(), key.to_string()), value.to_string());
            Ok(())
        }

        async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
            Ok(self.get(namespace, key))
        }

        async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    /// Store whose `retrieve` fails — models a keychain read error.
    struct ReadFailStore;

    #[async_trait]
    impl SecretStore for ReadFailStore {
        async fn store(&self, _namespace: &str, _key: &str, _value: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn retrieve(
            &self,
            _namespace: &str,
            _key: &str,
        ) -> Result<Option<String>, CoreError> {
            Err(CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: "simulated keychain read failure".to_string(),
            })
        }

        async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    /// Store whose `retrieve` is empty but whose `store` fails — models a
    /// keychain write error on a fresh install.
    struct WriteFailStore;

    #[async_trait]
    impl SecretStore for WriteFailStore {
        async fn store(&self, _namespace: &str, _key: &str, _value: &str) -> Result<(), CoreError> {
            Err(CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: "simulated keychain write failure".to_string(),
            })
        }

        async fn retrieve(
            &self,
            _namespace: &str,
            _key: &str,
        ) -> Result<Option<String>, CoreError> {
            Ok(None)
        }

        async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
            Ok(())
        }

        async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[test]
    fn generate_produces_64_char_lowercase_hex() {
        let secret = generate_gui_ticket_secret();
        assert_eq!(secret.len(), GENERATED_SECRET_BYTES * 2);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(secret.chars().all(|c| !c.is_ascii_uppercase()));
    }

    #[test]
    fn generate_produces_unique_secrets() {
        let a = generate_gui_ticket_secret();
        let b = generate_gui_ticket_secret();
        assert_ne!(a, b, "two generated secrets must differ");
    }

    #[tokio::test]
    async fn provision_returns_existing_secret_without_overwriting() {
        let store = InMemoryStore::seeded(
            GUI_TICKET_SECRET_NAMESPACE,
            GUI_TICKET_SECRET_KEY,
            "preexisting-secret",
        );
        let resolved = provision_from_store(&store).await;
        assert_eq!(resolved.as_deref(), Some("preexisting-secret"));
        // The existing value must be untouched (no rotation on load).
        assert_eq!(
            store.get(GUI_TICKET_SECRET_NAMESPACE, GUI_TICKET_SECRET_KEY),
            Some("preexisting-secret".to_string())
        );
    }

    #[tokio::test]
    async fn provision_generates_and_persists_when_absent() {
        let store = InMemoryStore::new();
        let resolved = provision_from_store(&store).await;
        let secret = resolved.expect("fresh install must auto-provision a secret");
        assert_eq!(secret.len(), GENERATED_SECRET_BYTES * 2);
        // The generated secret is persisted for the next launch.
        assert_eq!(
            store.get(GUI_TICKET_SECRET_NAMESPACE, GUI_TICKET_SECRET_KEY),
            Some(secret)
        );
    }

    #[tokio::test]
    async fn provision_treats_empty_stored_value_as_absent() {
        let store =
            InMemoryStore::seeded(GUI_TICKET_SECRET_NAMESPACE, GUI_TICKET_SECRET_KEY, "   ");
        let resolved = provision_from_store(&store).await;
        let secret = resolved.expect("blank stored value must be regenerated");
        assert_eq!(secret.len(), GENERATED_SECRET_BYTES * 2);
        assert_ne!(secret.trim(), "");
    }

    #[tokio::test]
    async fn provision_fails_closed_on_read_error() {
        let resolved = provision_from_store(&ReadFailStore).await;
        assert_eq!(resolved, None, "read error must fail closed");
    }

    #[tokio::test]
    async fn provision_fails_closed_on_write_error() {
        let resolved = provision_from_store(&WriteFailStore).await;
        assert_eq!(resolved, None, "write error must fail closed");
    }

    #[test]
    fn env_override_takes_precedence_and_skips_store() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resolved = resolve_gui_ticket_hmac_secret_inner(
            Some("explicit-env-secret".to_string()),
            rt.handle(),
            || panic!("store factory must not run when the env override is present"),
        );
        assert_eq!(resolved.as_deref(), Some("explicit-env-secret"));
    }

    #[test]
    fn blank_env_falls_through_to_keychain_provisioning() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryStore::new());
        let resolved =
            resolve_gui_ticket_hmac_secret_inner(Some("   ".to_string()), rt.handle(), || {
                Some(store)
            });
        let secret = resolved.expect("blank env must fall through to auto-provisioning");
        assert_eq!(secret.len(), GENERATED_SECRET_BYTES * 2);
    }

    #[test]
    fn fails_closed_when_no_env_and_no_store() {
        // DoD: keychain unavailable + no env ⇒ refuse exactly as before.
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resolved = resolve_gui_ticket_hmac_secret_inner(None, rt.handle(), || None);
        assert_eq!(resolved, None);
    }

    #[test]
    fn fails_closed_when_no_env_and_store_read_errors() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resolved = resolve_gui_ticket_hmac_secret_inner(None, rt.handle(), || {
            Some(Arc::new(ReadFailStore) as Arc<dyn SecretStore>)
        });
        assert_eq!(resolved, None);
    }

    #[test]
    fn auto_provisions_when_no_env_and_store_available() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let store: Arc<dyn SecretStore> = Arc::new(InMemoryStore::new());
        let store_probe = store.clone();
        let resolved = resolve_gui_ticket_hmac_secret_inner(None, rt.handle(), move || Some(store));
        let secret = resolved.expect("fresh install with a keychain must provision");
        assert_eq!(secret.len(), GENERATED_SECRET_BYTES * 2);
        // Persisted so the next launch loads the same secret.
        let persisted =
            rt.block_on(store_probe.retrieve(GUI_TICKET_SECRET_NAMESPACE, GUI_TICKET_SECRET_KEY));
        assert_eq!(persisted.unwrap().as_deref(), Some(secret.as_str()));
    }
}
