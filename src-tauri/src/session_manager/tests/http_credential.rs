//! #12541: an HttpApi session sends the API key Settings saved, and only on
//! the surface and host Settings saved it for. Every test goes through the real
//! factory with an in-memory secret store, and none sends a request.

use super::*;
use crate::scheduler::shared_regime_state::SharedRegimeState;
use maekon_api_contracts::provider_specs::{self, ProviderTransportKind, SurfaceExecutionKind};
use maekon_core::config::{
    AiProviderConfig, AiProviderType, AppConfig, CredentialAuthMode, CredentialBackendKind,
    CredentialBinding, ExternalApiEndpoint, SecretRef,
};
use maekon_core::error_codes::AuthCode;
use maekon_core::models::ai_session::{MessageRole, SessionMessage};
use maekon_core::ports::secret_store::{provider_api_key_secret_ref, SecretStore, SecretStoreSet};
use maekon_core::provider_surface::{provider_type_from_vendor_id, provider_vendor_id_or_default};

const OPENAI: &str = "provider_surface.openai.direct_api";
const GROQ: &str = "provider_surface.groq.direct_api";

#[derive(Default)]
struct MemorySecrets(parking_lot::Mutex<HashMap<(String, String), String>>);

#[async_trait]
impl SecretStore for MemorySecrets {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        self.0
            .lock()
            .insert((namespace.to_string(), key.to_string()), value.to_string());
        Ok(())
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        Ok(self
            .0
            .lock()
            .get(&(namespace.to_string(), key.to_string()))
            .cloned())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        self.0
            .lock()
            .remove(&(namespace.to_string(), key.to_string()));
        Ok(())
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        self.0
            .lock()
            .retain(|(existing, _), _| existing != namespace);
        Ok(())
    }
}

fn provider_type_of(surface_id: &str) -> AiProviderType {
    let surface = provider_specs::provider_surface_spec(surface_id).expect("catalog surface");
    provider_type_from_vendor_id(&surface.provider_type).expect("known provider type")
}

/// The URL an HttpApi session on `surface_id` sends to. Settings fills in the
/// same catalog URL as the default `llm_api` endpoint for that surface.
fn catalog_url(surface_id: &str) -> String {
    provider_specs::resolved_transport_spec(
        provider_type_of(surface_id),
        Some(surface_id),
        ProviderTransportKind::Llm,
    )
    .expect("llm transport")
    .url
    .clone()
}

/// `llm_api` as Settings records it after saving a key for `surface_id` on
/// `endpoint`: the binding points at `provider_api_key_secret_ref`, the
/// location Settings writes the key to (`persist_api_key_binding`).
fn settings_llm_api(surface_id: &str, endpoint: String) -> ExternalApiEndpoint {
    let provider_type = provider_type_of(surface_id);
    let (namespace, key) =
        provider_api_key_secret_ref(provider_vendor_id_or_default(provider_type), "llm")
            .expect("settings secret location");
    ExternalApiEndpoint {
        endpoint,
        api_key: String::new(),
        model: None,
        timeout_secs: 30,
        provider_type,
        surface_id: Some(surface_id.to_string()),
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace,
                key: key.to_string(),
            }),
            projection_enabled: false,
        }),
    }
}

async fn save_key(store: &MemorySecrets, llm_api: &ExternalApiEndpoint, value: &str) {
    let secret_ref = llm_api
        .credential
        .as_ref()
        .and_then(|binding| binding.secret_ref.as_ref())
        .expect("settings binding");
    store
        .store(&secret_ref.namespace, &secret_ref.key, value)
        .await
        .expect("save key");
}

/// A manager with one store behind every backend, so these tests exercise the
/// surface and host rules alone.
fn manager_with(ai_provider: AiProviderConfig, store: Arc<MemorySecrets>) -> SessionManagerImpl {
    let stores = SecretStoreSet {
        os_secret_store: Some(store.clone()),
        file_secret_store: Some(store.clone()),
        env_secret_store: Some(store),
        default_backend_kind: CredentialBackendKind::OsSecretStore,
        fallback_backend_kind: CredentialBackendKind::Unavailable,
    };
    manager_with_stores(ai_provider, stores)
}

/// The provider store set `ProviderRuntimeContext` builds: one store per
/// backend, with `default_backend` the one `MAEKON_PROVIDER_SECRET_BACKEND`
/// selects for new keys.
fn store_set(
    default_backend: CredentialBackendKind,
    os: Arc<MemorySecrets>,
    file: Arc<MemorySecrets>,
) -> SecretStoreSet {
    SecretStoreSet {
        os_secret_store: Some(os),
        file_secret_store: Some(file),
        env_secret_store: None,
        default_backend_kind: default_backend,
        fallback_backend_kind: CredentialBackendKind::Unavailable,
    }
}

/// A manager wired as `session_wiring.rs` wires it: the context assembler
/// carries the boot-time config whose `ai_provider` holds the binding, and the
/// provider store set reaches it whole.
fn manager_with_stores(
    ai_provider: AiProviderConfig,
    stores: SecretStoreSet,
) -> SessionManagerImpl {
    let mut app_config = AppConfig::default_config();
    app_config.ai_provider = ai_provider;
    let assembler = SessionContextAssembler::new(
        Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(30).expect("storage")),
        Arc::new(app_config),
        Arc::new(SharedRegimeState::new()),
    );
    SessionManagerImpl::new(
        test_config(),
        Arc::new(crate::auditing_session::tests::MockAudit::default()),
        Some(Arc::new(assembler)),
    )
    .with_privacy_guard(Arc::new(PassthroughGuard))
    .with_secret_stores(stores)
}

fn configured(llm_api: Option<ExternalApiEndpoint>) -> AiProviderConfig {
    AiProviderConfig {
        llm_api,
        ..AiProviderConfig::default()
    }
}

fn http_session(surface_id: &str) -> SessionConfig {
    SessionConfig {
        transport: SessionTransport::HttpApi,
        surface_id: Some(surface_id.to_string()),
        // Explicit, so a surface without a catalog default model still
        // reaches the credential step.
        model: Some("test-model".to_string()),
        // Explicit, so the context assembler adds no activity context.
        system_prompt: Some("Be brief.".to_string()),
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    }
}

/// The error the Chat page turns into "add its credential in Settings".
async fn assert_not_configured(manager: &SessionManagerImpl, surface_id: &str) {
    let Err(error) = manager.create_session(http_session(surface_id)).await else {
        panic!("{surface_id} must not open without a key saved for it");
    };
    match error {
        CoreError::Auth {
            code: AuthCode::Failed,
            message,
        } => assert_eq!(
            message,
            format!("no credential available for surface '{surface_id}'")
        ),
        other => panic!("{surface_id}: expected the not-configured error, got {other:?}"),
    }
    assert!(
        manager.list_sessions().await.is_empty(),
        "a refused session must not be admitted"
    );
}

/// HTTP surfaces the Chat page offers (`filterHttpApiSurfaces`) that need a key.
fn authenticated_chat_surfaces() -> Vec<&'static str> {
    provider_specs::provider_surface_catalog()
        .expect("catalog")
        .surfaces
        .iter()
        .filter(|surface| {
            surface.supports.llm && surface.execution_kind == SurfaceExecutionKind::DirectHttp
        })
        .filter(|surface| {
            surface.llm_transport.as_ref().is_some_and(|transport| {
                !matches!(transport.auth_scheme.as_str(), "none" | "aws_signature_v4")
            })
        })
        .map(|surface| surface.surface_id.as_str())
        .collect()
}

#[tokio::test]
async fn settings_saved_key_opens_a_session_on_the_configured_surface() {
    let store = Arc::new(MemorySecrets::default());
    let llm_api = settings_llm_api(OPENAI, catalog_url(OPENAI));
    save_key(&store, &llm_api, "sk-saved-in-settings").await;
    let manager = manager_with(configured(Some(llm_api)), store);

    let session = manager
        .create_session(http_session(OPENAI))
        .await
        .expect("the key Settings saved must open the configured surface");
    assert_eq!(session.provider_name(), "openai");
    assert!(session.is_external(), "a cloud session carries the guard");
    assert_eq!(manager.list_sessions().await.len(), 1);

    // The factory builds the session credential with this same call.
    let credential = manager
        .http_session_credential(OPENAI, &catalog_url(OPENAI))
        .expect("configured surface credential");
    assert_eq!(
        credential.resolve_bearer_token().await.expect("saved key"),
        "sk-saved-in-settings"
    );
}

/// The session reads its key from the Settings location on every request, so a
/// removed key fails the next turn at authentication, before any request.
#[tokio::test]
async fn session_reads_its_key_from_the_settings_location() {
    let store = Arc::new(MemorySecrets::default());
    let llm_api = settings_llm_api(OPENAI, catalog_url(OPENAI));
    save_key(&store, &llm_api, "sk-saved-in-settings").await;
    let manager = manager_with(configured(Some(llm_api)), store.clone());
    let session = manager
        .create_session(http_session(OPENAI))
        .await
        .expect("the configured surface opens");

    store
        .delete("provider/openai/llm", "api_key")
        .await
        .expect("remove the saved key");
    let message = SessionMessage {
        screen_derived: false,
        role: MessageRole::User,
        content: "hello".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };
    let Err(error) = session.send_message(&message).await else {
        panic!("a turn without its saved key must fail before sending");
    };
    assert_eq!(error.code(), "auth.failed");
    assert!(
        error.to_string().contains("provider/openai/llm.api_key"),
        "the session must read the Settings location, got: {error}"
    );
}

#[tokio::test]
async fn without_a_saved_key_the_session_stays_not_configured() {
    let store = Arc::new(MemorySecrets::default());
    assert_not_configured(&manager_with(configured(None), store.clone()), OPENAI).await;

    // The surface is selected, but no key was saved for it.
    let unsaved = ExternalApiEndpoint {
        credential: None,
        ..settings_llm_api(OPENAI, catalog_url(OPENAI))
    };
    assert_not_configured(&manager_with(configured(Some(unsaved)), store), OPENAI).await;
}

/// Every OpenAI-compatible vendor's key lands in `provider/generic/llm`, so
/// only the binding's surface keeps a Groq key away from DeepSeek. No other
/// authenticated surface, the Chat page's default among them, gets the key.
#[tokio::test]
async fn a_key_saved_for_one_provider_reaches_no_other_surface() {
    let store = Arc::new(MemorySecrets::default());
    let llm_api = settings_llm_api(GROQ, catalog_url(GROQ));
    save_key(&store, &llm_api, "gsk-saved-for-groq").await;
    let manager = manager_with(configured(Some(llm_api)), store);

    let others: Vec<&str> = authenticated_chat_surfaces()
        .into_iter()
        .filter(|surface_id| *surface_id != GROQ)
        .collect();
    assert!(
        others.contains(&"provider_surface.deepseek.direct_api") && others.contains(&OPENAI),
        "the sweep must cover the shared-namespace and cross-vendor cases: {others:?}"
    );
    for surface_id in others {
        assert_not_configured(&manager, surface_id).await;
    }
    manager
        .create_session(http_session(GROQ))
        .await
        .expect("the configured surface still opens");
}

/// The session sends to the catalog URL, so a key saved for a custom endpoint
/// on another host is never sent there.
#[tokio::test]
async fn a_key_saved_for_a_custom_host_is_not_sent_to_the_catalog_host() {
    let store = Arc::new(MemorySecrets::default());
    let llm_api = settings_llm_api(
        OPENAI,
        "https://llm-proxy.example.com/v1/responses".to_string(),
    );
    save_key(&store, &llm_api, "proxy-key").await;
    assert_not_configured(&manager_with(configured(Some(llm_api)), store), OPENAI).await;
}

/// An env-backed binding has no stored location; it resolves through the
/// active secret profile, as the analysis LLM resolver does.
#[tokio::test]
async fn env_backed_binding_uses_the_active_secret_profile() {
    let store = Arc::new(MemorySecrets::default());
    let (namespace, key) = provider_api_key_secret_ref("openai", "work").expect("profile location");
    store
        .store(&namespace, key, "sk-work-profile")
        .await
        .expect("save key");
    let mut llm_api = settings_llm_api(OPENAI, catalog_url(OPENAI));
    llm_api.credential = Some(CredentialBinding {
        auth_mode: CredentialAuthMode::ApiKey,
        backend_kind: CredentialBackendKind::Env,
        secret_ref: None,
        projection_enabled: false,
    });
    let ai_provider = AiProviderConfig {
        active_profile_id: Some("work".to_string()),
        ..configured(Some(llm_api))
    };
    let manager = manager_with(ai_provider, store);

    let credential = manager
        .http_session_credential(OPENAI, &catalog_url(OPENAI))
        .expect("env-backed credential");
    assert_eq!(
        credential
            .resolve_bearer_token()
            .await
            .expect("profile key"),
        "sk-work-profile"
    );
}

/// #12563: `MAEKON_PROVIDER_SECRET_BACKEND` picks the store new keys go to, but
/// a binding keeps naming the store its key was saved in. The headless Linux
/// guide switches `auto` to `file_secret_store`: a key saved before the switch
/// stays in the OS keychain, and the file store can hold an older copy at the
/// same location. The session must send the keychain key, as the LLM resolver
/// does (`SecretStoreSet::for_binding`).
#[tokio::test]
async fn a_keychain_binding_reads_the_keychain_after_the_backend_switches_to_file() {
    let os = Arc::new(MemorySecrets::default());
    let file = Arc::new(MemorySecrets::default());
    let llm_api = settings_llm_api(OPENAI, catalog_url(OPENAI));
    save_key(&os, &llm_api, "sk-saved-in-keychain").await;
    save_key(&file, &llm_api, "sk-stale-file-copy").await;
    let manager = manager_with_stores(
        configured(Some(llm_api)),
        store_set(CredentialBackendKind::FileSecretStore, os, file),
    );

    let credential = manager
        .http_session_credential(OPENAI, &catalog_url(OPENAI))
        .expect("keychain-bound credential");
    assert_eq!(
        credential
            .resolve_bearer_token()
            .await
            .expect("keychain key"),
        "sk-saved-in-keychain"
    );
}

/// #12563: a binding that names the file store (a non-default `backend_kind`
/// sent to `POST /api/settings`, or an edited config.json) opens a session with
/// the file store's key while the keychain stays the default.
#[tokio::test]
async fn a_file_store_binding_opens_a_session_with_the_file_store_key() {
    let os = Arc::new(MemorySecrets::default());
    let file = Arc::new(MemorySecrets::default());
    let mut llm_api = settings_llm_api(OPENAI, catalog_url(OPENAI));
    llm_api
        .credential
        .as_mut()
        .expect("settings binding")
        .backend_kind = CredentialBackendKind::FileSecretStore;
    save_key(&file, &llm_api, "sk-saved-in-file-store").await;
    let manager = manager_with_stores(
        configured(Some(llm_api)),
        store_set(CredentialBackendKind::OsSecretStore, os, file),
    );

    manager
        .create_session(http_session(OPENAI))
        .await
        .expect("the file store key must open the configured surface");
    let credential = manager
        .http_session_credential(OPENAI, &catalog_url(OPENAI))
        .expect("file-bound credential");
    assert_eq!(
        credential
            .resolve_bearer_token()
            .await
            .expect("file store key"),
        "sk-saved-in-file-store"
    );
}

/// Positive control: a no-auth surface still opens with nothing saved.
#[tokio::test]
async fn a_no_auth_surface_opens_without_a_saved_key() {
    let manager = manager_with(configured(None), Arc::new(MemorySecrets::default()));
    let session = manager
        .create_session(http_session("provider_surface.ollama.local_http"))
        .await
        .expect("a no-auth surface needs no key");
    assert!(!session.is_external());
}

/// #12530: HTTP Chat invocation evidence resolves its credential here.
#[path = "http_chat_evidence.rs"]
mod http_chat_evidence;
