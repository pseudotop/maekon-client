//! #12530: HTTP Chat invocation evidence names the key this factory sends.
//! Readiness resolves the credential through `http_session_credential`, so the
//! fingerprint follows the #12541 rule instead of restating it. These tests
//! drive the real factory with the Settings binding and an in-memory store.

use super::*;
use crate::ai_invocation_evidence::{
    http_chat_fingerprint, http_chat_invocation_verified, session_factory_credential,
    FingerprintCredential, HttpChatTarget,
};
use maekon_core::config::AiAccessMode;
use maekon_core::ports::credential_source::CredentialSource;

const OLLAMA: &str = "provider_surface.ollama.local_http";
const MODEL: &str = "test-model";

/// `llm_api` as the current configuration holds it in an HTTP access mode.
fn http_mode(mut llm_api: ExternalApiEndpoint) -> AiProviderConfig {
    llm_api.model = Some(MODEL.to_string());
    AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        llm_api: Some(llm_api),
        ..AiProviderConfig::default()
    }
}

fn record(dir: &tempfile::TempDir, fingerprint: &str) -> std::path::PathBuf {
    let path = dir.path().join("http_chat_invocation_evidence.json");
    let json = serde_json::json!({
        "schema_version": 1,
        "fingerprint": fingerprint,
        "verified_at": "2026-09-24T00:00:00Z",
    });
    std::fs::write(&path, json.to_string()).expect("write record");
    path
}

fn factory_credential(
    manager: &SessionManagerImpl,
    target: &HttpChatTarget,
) -> Option<CredentialSource> {
    session_factory_credential(Some(manager), &target.surface_id, &target.url)
}

/// The readiness input `feature_capability_snapshot` computes with `manager`.
async fn verified_by(
    manager: &SessionManagerImpl,
    config: &AiProviderConfig,
    path: &std::path::Path,
) -> bool {
    http_chat_invocation_verified(
        config,
        |surface_id, url| session_factory_credential(Some(manager), surface_id, url),
        Some(path),
    )
    .await
}

#[tokio::test]
async fn evidence_names_the_key_the_factory_sends_on_the_configured_surface() {
    let store = Arc::new(MemorySecrets::default());
    let llm_api = settings_llm_api(OPENAI, catalog_url(OPENAI));
    save_key(&store, &llm_api, "sk-saved-in-settings").await;
    let manager = manager_with(configured(Some(llm_api.clone())), store.clone());
    let config = http_mode(llm_api);

    let target = HttpChatTarget::resolve(&config).expect("configured surface");
    assert_eq!(target.url, catalog_url(OPENAI));
    let credential = factory_credential(&manager, &target).expect("factory credential");
    let fingerprint = target.fingerprint(&credential).await.expect("fingerprint");
    assert_eq!(
        fingerprint,
        http_chat_fingerprint(
            OPENAI,
            MODEL,
            FingerprintCredential::Secret("sk-saved-in-settings")
        )
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let path = record(&dir, &fingerprint);
    assert!(
        verified_by(&manager, &config, &path).await,
        "the key the session sends"
    );

    // Settings saves a new key to the same location.
    store
        .store("provider/openai/llm", "api_key", "sk-rotated")
        .await
        .expect("rotate key");
    assert!(
        !verified_by(&manager, &config, &path).await,
        "evidence for the old key"
    );
}

/// Where the factory refuses to open a session, no evidence can describe one.
#[tokio::test]
async fn surfaces_and_hosts_the_factory_refuses_have_no_credential() {
    let store = Arc::new(MemorySecrets::default());
    let openai = settings_llm_api(OPENAI, catalog_url(OPENAI));
    save_key(&store, &openai, "sk-openai").await;
    let manager = manager_with(configured(Some(openai)), store.clone());

    // Another surface than the one the key was saved for.
    let groq = HttpChatTarget::resolve(&http_mode(settings_llm_api(GROQ, catalog_url(GROQ))))
        .expect("groq target");
    assert!(factory_credential(&manager, &groq).is_none());

    // A key saved for a custom host is not sent to the catalog host.
    let proxied = settings_llm_api(OPENAI, "https://gateway.example/v1".to_string());
    let proxied_manager = manager_with(configured(Some(proxied.clone())), store.clone());
    let target = HttpChatTarget::resolve(&http_mode(proxied)).expect("openai target");
    assert!(factory_credential(&proxied_manager, &target).is_none());

    // No session manager, no session.
    assert!(session_factory_credential(None, &target.surface_id, &target.url).is_none());
}

#[tokio::test]
async fn a_no_auth_surface_is_fingerprinted_without_a_key() {
    let store = Arc::new(MemorySecrets::default());
    let manager = manager_with(configured(None), store);
    let llm_api = ExternalApiEndpoint {
        endpoint: catalog_url(OLLAMA),
        api_key: String::new(),
        model: None,
        timeout_secs: 30,
        provider_type: provider_type_of(OLLAMA),
        surface_id: Some(OLLAMA.to_string()),
        credential: None,
    };

    let target = HttpChatTarget::resolve(&http_mode(llm_api)).expect("ollama target");
    let credential = factory_credential(&manager, &target).expect("no-auth credential");
    assert!(matches!(credential, CredentialSource::NoAuth));
    assert_eq!(
        target.fingerprint(&credential).await,
        Some(http_chat_fingerprint(
            OLLAMA,
            MODEL,
            FingerprintCredential::NoAuth
        ))
    );
}
