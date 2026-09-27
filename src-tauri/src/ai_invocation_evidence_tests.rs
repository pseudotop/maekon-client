use super::*;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::TimeZone;
use maekon_core::config::{AiProviderType, ExternalApiEndpoint};
use maekon_core::error::CoreError;
use maekon_core::ports::secret_store::SecretStore;

const OLLAMA_SURFACE: &str = "provider_surface.ollama.local_http";
const LOCAL_COMPATIBLE_SURFACE: &str = "provider_surface.generic.local_openai_compatible";
const OPENAI_SURFACE: &str = "provider_surface.openai.direct_api";

#[derive(Default)]
struct MemorySecretStore {
    values: parking_lot::Mutex<HashMap<(String, String), String>>,
}

#[async_trait]
impl SecretStore for MemorySecretStore {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .insert((namespace.to_owned(), key.to_owned()), value.to_owned());
        Ok(())
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        let value = self
            .values
            .lock()
            .get(&(namespace.to_owned(), key.to_owned()))
            .cloned();
        Ok(value)
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .remove(&(namespace.to_owned(), key.to_owned()));
        Ok(())
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .retain(|(existing, _), _| existing != namespace);
        Ok(())
    }
}

/// A backend that never answers within the readiness bound.
struct StalledSecretStore;

#[async_trait]
impl SecretStore for StalledSecretStore {
    async fn store(&self, _namespace: &str, _key: &str, _value: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn retrieve(&self, _namespace: &str, _key: &str) -> Result<Option<String>, CoreError> {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(Some("sk-late".to_owned()))
    }

    async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

fn llm_api(surface_id: &str, model: Option<&str>) -> ExternalApiEndpoint {
    ExternalApiEndpoint {
        endpoint: "http://localhost:11434/v1/responses".to_owned(),
        api_key: String::new(),
        model: model.map(str::to_owned),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some(surface_id.to_owned()),
        credential: None,
    }
}

fn http_config(surface_id: &str, model: Option<&str>) -> AiProviderConfig {
    AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        llm_api: Some(llm_api(surface_id, model)),
        ..Default::default()
    }
}

fn openai_target() -> HttpChatTarget {
    HttpChatTarget {
        surface_id: OPENAI_SURFACE.to_owned(),
        model: "gpt-5.4".to_owned(),
        url: "https://api.openai.com/v1/responses".to_owned(),
    }
}

fn stored_secret(store: Arc<dyn SecretStore>) -> CredentialSource {
    CredentialSource::StoredSecret {
        namespace: "provider/openai/llm".to_owned(),
        key: "api_key".to_owned(),
        secret_store: store,
    }
}

fn verified_at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 23, 6, 0, 0)
        .single()
        .expect("valid timestamp")
}

/// A record in the format the verification command writes.
fn record(schema_version: u32, fingerprint: &str, verified_at: &str) -> String {
    serde_json::json!({
        "schema_version": schema_version,
        "fingerprint": fingerprint,
        "verified_at": verified_at,
    })
    .to_string()
}

fn no_auth(_surface_id: &str, _url: &str) -> Option<CredentialSource> {
    Some(CredentialSource::NoAuth)
}

#[test]
fn fingerprint_changes_with_surface_model_or_secret() {
    let base = http_chat_fingerprint(
        OPENAI_SURFACE,
        "gpt-5.4",
        FingerprintCredential::Secret("sk-one"),
    );
    assert!(is_fingerprint(&base), "unexpected format: {base}");
    assert_eq!(
        base,
        http_chat_fingerprint(
            OPENAI_SURFACE,
            "gpt-5.4",
            FingerprintCredential::Secret("sk-one")
        ),
        "the same inputs must reproduce the recorded fingerprint"
    );

    let variants = [
        http_chat_fingerprint(
            OLLAMA_SURFACE,
            "gpt-5.4",
            FingerprintCredential::Secret("sk-one"),
        ),
        http_chat_fingerprint(
            OPENAI_SURFACE,
            "gpt-5.2",
            FingerprintCredential::Secret("sk-one"),
        ),
        http_chat_fingerprint(
            OPENAI_SURFACE,
            "gpt-5.4",
            FingerprintCredential::Secret("sk-two"),
        ),
        http_chat_fingerprint(OPENAI_SURFACE, "gpt-5.4", FingerprintCredential::NoAuth),
    ];
    for variant in variants {
        assert_ne!(base, variant);
    }
}

#[test]
fn fingerprint_parts_cannot_shift_between_fields() {
    assert_ne!(
        http_chat_fingerprint("ab", "c", FingerprintCredential::NoAuth),
        http_chat_fingerprint("a", "bc", FingerprintCredential::NoAuth),
    );
    // A secret spelled like the no-auth marker is still a secret.
    assert_ne!(
        http_chat_fingerprint(
            OLLAMA_SURFACE,
            "m",
            FingerprintCredential::Secret("no-auth")
        ),
        http_chat_fingerprint(OLLAMA_SURFACE, "m", FingerprintCredential::NoAuth),
    );
}

#[test]
fn the_record_never_carries_the_secret_or_its_bare_digest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let secret = "sk-live-0123456789abcdef";
    let fingerprint = http_chat_fingerprint(
        OPENAI_SURFACE,
        "gpt-5.4",
        FingerprintCredential::Secret(secret),
    );

    write_evidence(&path, &fingerprint, verified_at()).expect("write evidence");

    let stored = std::fs::read_to_string(&path).expect("read evidence");
    let bare_digest: String = Sha256::digest(secret.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(!stored.contains(secret));
    assert!(!stored.contains(&bare_digest));
    assert!(stored.contains(&fingerprint));
}

#[test]
fn write_replaces_the_record_and_leaves_no_temporary_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let first = http_chat_fingerprint(OLLAMA_SURFACE, "qwen3:8b", FingerprintCredential::NoAuth);
    let second = http_chat_fingerprint(OLLAMA_SURFACE, "llama3", FingerprintCredential::NoAuth);

    write_evidence(&path, &first, verified_at()).expect("first write");
    write_evidence(&path, &second, verified_at()).expect("second write");

    assert_eq!(read_recorded_fingerprint(&path), Some(second.clone()));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .expect("list dir")
        .map(|entry| entry.expect("dir entry").file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from(EVIDENCE_FILE_NAME)]);

    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
    assert_eq!(stored["schema_version"], 1);
    assert_eq!(stored["fingerprint"], second);
    assert_eq!(stored["verified_at"], "2026-09-23T06:00:00Z");
}

/// A reader running beside the writer sees a whole record every time. With a
/// truncate-then-write replacement the reader catches an empty or partial file.
#[cfg(unix)]
#[test]
fn a_concurrent_reader_never_observes_a_partial_record() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let fingerprints = [
        http_chat_fingerprint(OLLAMA_SURFACE, "qwen3:8b", FingerprintCredential::NoAuth),
        http_chat_fingerprint(OLLAMA_SURFACE, "llama3", FingerprintCredential::NoAuth),
    ];
    write_evidence(&path, &fingerprints[0], verified_at()).expect("seed record");

    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let done = Arc::clone(&done);
        let path = path.clone();
        let fingerprints = fingerprints.clone();
        std::thread::spawn(move || {
            let mut partial_reads = 0_u32;
            while !done.load(Ordering::Relaxed) {
                match read_recorded_fingerprint(&path) {
                    Some(value) if fingerprints.contains(&value) => {}
                    _ => partial_reads += 1,
                }
            }
            partial_reads
        })
    };
    for round in 0..400 {
        write_evidence(&path, &fingerprints[round % 2], verified_at()).expect("rewrite record");
    }
    done.store(true, Ordering::Relaxed);

    assert_eq!(reader.join().expect("reader thread"), 0);
}

#[test]
fn erasure_removes_the_record_and_treats_a_missing_one_as_erased() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let fingerprint =
        http_chat_fingerprint(OLLAMA_SURFACE, "qwen3:8b", FingerprintCredential::NoAuth);
    write_evidence(&path, &fingerprint, verified_at()).expect("write evidence");

    erase_evidence(&path).expect("erase the record");
    assert!(!path.exists(), "the record must be gone");
    erase_evidence(&path).expect("a missing record is already erased");

    // Something that cannot be removed as a file is a failed erasure.
    std::fs::create_dir(&path).expect("occupy the record path");
    let error = erase_evidence(&path).expect_err("a directory is not an erased record");
    assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
}

/// The writer and the reader agree on the record format.
#[test]
fn a_written_record_reads_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let fingerprint =
        http_chat_fingerprint(OLLAMA_SURFACE, "qwen3:8b", FingerprintCredential::NoAuth);
    write_evidence(&path, &fingerprint, verified_at()).expect("write evidence");
    assert_eq!(read_recorded_fingerprint(&path), Some(fingerprint));
}

#[test]
fn missing_or_corrupt_evidence_is_no_evidence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let fingerprint =
        http_chat_fingerprint(OLLAMA_SURFACE, "qwen3:8b", FingerprintCredential::NoAuth);

    assert_eq!(read_recorded_fingerprint(&path), None, "missing file");
    assert_eq!(read_recorded_fingerprint(dir.path()), None, "a directory");

    let upper = fingerprint
        .to_ascii_uppercase()
        .replace("SHA256:", "sha256:");
    let corrupt = [
        "not json".to_owned(),
        record(1, &fingerprint, "2026-09-23T06:00:00Z")[..40].to_owned(),
        "{}".to_owned(),
        record(2, &fingerprint, "2026-09-23T06:00:00Z"),
        record(
            1,
            fingerprint.trim_start_matches("sha256:"),
            "2026-09-23T06:00:00Z",
        ),
        record(1, &upper, "2026-09-23T06:00:00Z"),
        record(
            1,
            &fingerprint[..fingerprint.len() - 2],
            "2026-09-23T06:00:00Z",
        ),
        record(1, &fingerprint, "yesterday"),
    ];
    for contents in corrupt {
        std::fs::write(&path, &contents).expect("write fixture");
        assert_eq!(
            read_recorded_fingerprint(&path),
            None,
            "accepted {contents}"
        );
    }

    std::fs::write(&path, record(1, &fingerprint, "2026-09-23T06:00:00Z")).expect("write fixture");
    assert_eq!(read_recorded_fingerprint(&path), Some(fingerprint));
}

#[test]
fn resolve_uses_the_configured_model_or_the_catalog_default() {
    let default = HttpChatTarget::resolve(&http_config(OLLAMA_SURFACE, None)).expect("resolves");
    assert_eq!(default.surface_id, OLLAMA_SURFACE);
    assert_eq!(default.model, "qwen3:8b");

    let blank =
        HttpChatTarget::resolve(&http_config(OLLAMA_SURFACE, Some("  "))).expect("resolves");
    assert_eq!(blank.model, "qwen3:8b");

    let configured =
        HttpChatTarget::resolve(&http_config(OLLAMA_SURFACE, Some("llama3"))).expect("resolves");
    assert_eq!(configured.model, "llama3");

    let spelled = HttpChatTarget::resolve(&http_config(
        " PROVIDER_SURFACE.OLLAMA.LOCAL_HTTP ",
        Some("llama3"),
    ))
    .expect("resolves");
    assert_eq!(spelled.surface_id, OLLAMA_SURFACE);

    // No configured model and no catalog default: the session factory refuses.
    assert!(HttpChatTarget::resolve(&http_config(LOCAL_COMPATIBLE_SURFACE, None)).is_none());
    assert!(HttpChatTarget::resolve(&http_config("provider_surface.unknown", Some("m"))).is_none());
}

/// The session factory sends to the catalog transport URL, not to the
/// endpoint `llm_api` stores, so the credential is resolved for that URL.
#[test]
fn resolve_targets_the_catalog_transport_url() {
    let target =
        HttpChatTarget::resolve(&http_config(OLLAMA_SURFACE, Some("llama3"))).expect("resolves");
    let expected = provider_specs::resolved_transport_spec(
        AiProviderType::Ollama,
        Some(OLLAMA_SURFACE),
        ProviderTransportKind::Llm,
    )
    .expect("catalog transport")
    .url
    .clone();
    assert_eq!(target.url, expected);

    let mut proxied = http_config(OLLAMA_SURFACE, Some("llama3"));
    if let Some(endpoint) = proxied.llm_api.as_mut() {
        endpoint.endpoint = "https://gateway.example/v1".to_owned();
    }
    assert_eq!(
        HttpChatTarget::resolve(&proxied).expect("resolves").url,
        expected
    );
}

#[test]
fn only_an_http_access_mode_with_a_named_surface_has_a_target() {
    let mut config = http_config(OLLAMA_SURFACE, Some("llama3"));
    assert!(HttpChatTarget::resolve(&config).is_some());

    config.access_mode = AiAccessMode::ProviderOAuth;
    assert!(HttpChatTarget::resolve(&config).is_some());

    for mode in [
        AiAccessMode::ProviderSubscriptionCli,
        AiAccessMode::LocalModel,
    ] {
        config.access_mode = mode;
        assert!(HttpChatTarget::resolve(&config).is_none(), "{mode:?}");
    }

    config.access_mode = AiAccessMode::ProviderApiKey;
    if let Some(endpoint) = config.llm_api.as_mut() {
        endpoint.surface_id = Some("   ".to_owned());
    }
    assert!(HttpChatTarget::resolve(&config).is_none());
    config.llm_api = None;
    assert!(HttpChatTarget::resolve(&config).is_none());
}

#[tokio::test]
async fn readiness_input_matches_only_the_recorded_fingerprint() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let config = http_config(OLLAMA_SURFACE, Some("llama3"));

    assert!(
        !http_chat_invocation_verified(&config, no_auth, Some(&path)).await,
        "no record"
    );

    let current = HttpChatTarget::resolve(&config)
        .expect("target")
        .fingerprint(&CredentialSource::NoAuth)
        .await
        .expect("fingerprint");
    std::fs::write(&path, record(1, &current, "2026-09-23T06:00:00Z")).expect("write record");
    assert!(
        http_chat_invocation_verified(&config, no_auth, Some(&path)).await,
        "matching record"
    );
    assert!(
        !http_chat_invocation_verified(&config, no_auth, None).await,
        "no data directory"
    );
    assert!(
        !http_chat_invocation_verified(&config, |_: &str, _: &str| None, Some(&path)).await,
        "the session factory has no credential for the target"
    );

    let other_model = http_config(OLLAMA_SURFACE, Some("qwen3:8b"));
    let other_surface = http_config(LOCAL_COMPATIBLE_SURFACE, Some("llama3"));
    let mut cli = config.clone();
    cli.access_mode = AiAccessMode::ProviderSubscriptionCli;
    for (changed, label) in [
        (&other_model, "model changed"),
        (&other_surface, "surface changed"),
        (&cli, "access mode changed"),
    ] {
        assert!(
            !http_chat_invocation_verified(changed, no_auth, Some(&path)).await,
            "{label}"
        );
    }

    std::fs::write(&path, b"{").expect("corrupt record");
    assert!(
        !http_chat_invocation_verified(&config, no_auth, Some(&path)).await,
        "corrupt record"
    );
}

/// Readiness asks for the credential of the session the target describes,
/// and asks nothing while there is no record to compare against.
#[tokio::test]
async fn readiness_asks_for_the_credential_of_the_target_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(EVIDENCE_FILE_NAME);
    let config = http_config(OLLAMA_SURFACE, Some("llama3"));
    let target = HttpChatTarget::resolve(&config).expect("target");
    let asked = parking_lot::Mutex::new(Vec::<(String, String)>::new());
    let session_credential = |surface_id: &str, url: &str| {
        asked.lock().push((surface_id.to_owned(), url.to_owned()));
        Some(CredentialSource::NoAuth)
    };

    assert!(!http_chat_invocation_verified(&config, session_credential, Some(&path)).await);
    assert!(asked.lock().is_empty(), "no record, no credential lookup");

    let current = target
        .fingerprint(&CredentialSource::NoAuth)
        .await
        .expect("fingerprint");
    std::fs::write(&path, record(1, &current, "2026-09-23T06:00:00Z")).expect("write record");
    assert!(http_chat_invocation_verified(&config, session_credential, Some(&path)).await);
    assert_eq!(
        *asked.lock(),
        vec![(target.surface_id.to_owned(), target.url.to_owned())]
    );
}

/// Settings stores a new key under the same secret ref, so only the secret
/// value can tell the old key's evidence from the new key.
#[tokio::test]
async fn rotating_the_secret_under_the_same_ref_changes_the_fingerprint() {
    let store = Arc::new(MemorySecretStore::default());
    let target = openai_target();
    let credential = stored_secret(store.clone());

    store
        .store("provider/openai/llm", "api_key", "sk-one")
        .await
        .expect("store");
    let first = target.fingerprint(&credential).await.expect("first key");
    assert_eq!(
        first,
        http_chat_fingerprint(
            OPENAI_SURFACE,
            "gpt-5.4",
            FingerprintCredential::Secret("sk-one")
        )
    );

    store
        .store("provider/openai/llm", "api_key", "sk-two")
        .await
        .expect("store");
    let rotated = target.fingerprint(&credential).await.expect("rotated key");
    assert_ne!(first, rotated);

    store
        .store("provider/openai/llm", "api_key", "  ")
        .await
        .expect("store");
    assert_eq!(target.fingerprint(&credential).await, None, "blank secret");
    store
        .delete("provider/openai/llm", "api_key")
        .await
        .expect("delete");
    assert_eq!(
        target.fingerprint(&credential).await,
        None,
        "missing secret"
    );
}

#[tokio::test(start_paused = true)]
async fn a_stalled_secret_backend_reads_as_unverified() {
    let credential = stored_secret(Arc::new(StalledSecretStore));
    assert_eq!(openai_target().fingerprint(&credential).await, None);
}
