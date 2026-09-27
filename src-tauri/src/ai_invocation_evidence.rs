//! Evidence that one API-key (HTTP) Chat invocation succeeded (#12530).
//!
//! Configuration alone cannot show that a provider accepts the saved key and
//! model, so `chat.http_api` readiness stays unverified until a verification
//! call succeeds. That call records a fingerprint of what its session used:
//! the provider surface, the model, and a digest of the secret. Readiness
//! trusts the record only while the current configuration and the secret the
//! session factory would send produce the same fingerprint. The secret is
//! never stored.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use maekon_api_contracts::provider_specs::{self, ProviderTransportKind, SurfaceCapabilityKind};
use maekon_core::config::{AiAccessMode, AiProviderConfig};
use maekon_core::ports::credential_source::CredentialSource;
use maekon_core::provider_surface::provider_type_from_vendor_id;

/// Kept next to `consent.json` in the app data directory.
const EVIDENCE_FILE_NAME: &str = "http_chat_invocation_evidence.json";
const EVIDENCE_SCHEMA_VERSION: u32 = 1;
const FINGERPRINT_DOMAIN: &[u8] = b"http-chat-invocation/v1\0";
const FINGERPRINT_PREFIX: &str = "sha256:";
const NO_AUTH_CREDENTIAL: &[u8] = b"no-auth";
/// A readiness read must not wait on a slow secret backend.
const SECRET_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize, Deserialize)]
struct EvidenceRecord {
    schema_version: u32,
    fingerprint: String,
    verified_at: String,
}

/// Evidence file for this installation, or `None` when the data directory
/// cannot be resolved.
pub(crate) fn evidence_path() -> Option<PathBuf> {
    maekon_core::config_manager::ConfigManager::data_dir()
        .ok()
        .map(|dir| dir.join(EVIDENCE_FILE_NAME))
}

/// Fingerprint recorded at `path`. A missing, unreadable, malformed, or
/// other-version file is no evidence; reading it never fails the caller.
pub(crate) fn read_recorded_fingerprint(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let record: EvidenceRecord = serde_json::from_slice(&bytes).ok()?;
    let valid = record.schema_version == EVIDENCE_SCHEMA_VERSION
        && is_fingerprint(&record.fingerprint)
        && DateTime::parse_from_rfc3339(&record.verified_at).is_ok();
    valid.then_some(record.fingerprint)
}

/// Replaces the record at `path` atomically. The JSON goes to a temporary file
/// in the same directory, which is then renamed over the record, so a reader
/// sees the previous record or the new one and never a partial write.
pub(crate) fn write_evidence(
    path: &Path,
    fingerprint: &str,
    verified_at: DateTime<Utc>,
) -> std::io::Result<()> {
    let directory = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "evidence path has no parent directory",
        )
    })?;
    std::fs::create_dir_all(directory)?;
    let record = EvidenceRecord {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        fingerprint: fingerprint.to_owned(),
        verified_at: verified_at.to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let json = serde_json::to_vec_pretty(&record)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(&json)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Deletes the record at `path` for local data erasure (GDPR Art. 17). A
/// record that is already gone counts as erased; any other failure is
/// reported so the erasure does not claim success with the record left.
pub(crate) fn erase_evidence(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

fn is_fingerprint(value: &str) -> bool {
    value.strip_prefix(FINGERPRINT_PREFIX).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Credential part of a fingerprint.
pub(crate) enum FingerprintCredential<'a> {
    /// The surface takes no credential.
    NoAuth,
    /// The secret the session sends. It enters the fingerprint only as its
    /// own SHA-256 digest.
    Secret(&'a str),
}

/// `sha256:<hex>` over the surface, the model, and the credential. Every part
/// is length-prefixed, so two different inputs never share an encoding.
pub(crate) fn http_chat_fingerprint(
    surface_id: &str,
    model: &str,
    credential: FingerprintCredential<'_>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(FINGERPRINT_DOMAIN);
    update_part(&mut hasher, surface_id.as_bytes());
    update_part(&mut hasher, model.as_bytes());
    match credential {
        FingerprintCredential::NoAuth => update_part(&mut hasher, NO_AUTH_CREDENTIAL),
        FingerprintCredential::Secret(secret) => {
            update_part(&mut hasher, Sha256::digest(secret.as_bytes()).as_slice());
        }
    }
    let hex: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{FINGERPRINT_PREFIX}{hex}")
}

fn update_part(hasher: &mut Sha256, part: &[u8]) {
    hasher.update((part.len() as u64).to_be_bytes());
    hasher.update(part);
}

/// Surface, model, and URL of the HTTP Chat session opened for the configured
/// `llm_api`. The credential is not part of the target: it comes from the
/// session factory (`SessionManagerImpl::http_session_credential`, #12541), so
/// the fingerprint describes the secret a session actually sends.
pub(crate) struct HttpChatTarget {
    /// Catalog id of the provider surface.
    pub(crate) surface_id: String,
    /// The configured model, or the catalog default when none is set.
    pub(crate) model: String,
    /// The catalog transport URL the session sends to.
    pub(crate) url: String,
}

impl HttpChatTarget {
    /// Resolves `llm_api` the way `create_http_api_session`
    /// (`session_manager/factory.rs`) builds its session: the provider catalog
    /// supplies the transport URL and the default model. `None` unless the
    /// access mode is an HTTP one and the factory could build the session.
    pub(crate) fn resolve(ai_provider: &AiProviderConfig) -> Option<Self> {
        let http_mode = matches!(
            ai_provider.access_mode.normalized_for_ai_surfaces(),
            AiAccessMode::ProviderApiKey | AiAccessMode::ProviderOAuth
        );
        if !http_mode {
            return None;
        }
        let llm_api = ai_provider.llm_api.as_ref()?;
        let surface_id = llm_api
            .surface_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())?;
        let surface = provider_specs::provider_surface_spec(surface_id).ok()?;
        let provider_type = provider_type_from_vendor_id(&surface.provider_type)?;
        let transport = provider_specs::resolved_transport_spec(
            provider_type,
            Some(surface_id),
            ProviderTransportKind::Llm,
        )
        .ok()?;
        let model = llm_api
            .model
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                provider_specs::resolved_default_model(
                    provider_type,
                    Some(surface_id),
                    SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })?;
        Some(Self {
            surface_id: surface.surface_id.clone(),
            model,
            url: transport.url.clone(),
        })
    }

    /// Fingerprint of this target sending `credential`, or `None` when the
    /// secret cannot be read within the lookup bound. The secret is hashed and
    /// dropped here.
    pub(crate) async fn fingerprint(&self, credential: &CredentialSource) -> Option<String> {
        if matches!(credential, CredentialSource::NoAuth) {
            return Some(http_chat_fingerprint(
                &self.surface_id,
                &self.model,
                FingerprintCredential::NoAuth,
            ));
        }
        let secret = tokio::time::timeout(SECRET_LOOKUP_TIMEOUT, credential.resolve_bearer_token())
            .await
            .ok()?
            .ok()
            .filter(|secret| !secret.trim().is_empty())?;
        Some(http_chat_fingerprint(
            &self.surface_id,
            &self.model,
            FingerprintCredential::Secret(&secret),
        ))
    }
}

/// Credential the session factory resolves for an HttpApi session on
/// `surface_id` sending to `url` (#12541). Without a session manager there is
/// no session to describe.
#[cfg(feature = "analysis")]
pub(crate) fn session_factory_credential(
    manager: Option<&crate::session_manager::SessionManagerImpl>,
    surface_id: &str,
    url: &str,
) -> Option<CredentialSource> {
    manager?.http_session_credential(surface_id, url).ok()
}

#[cfg(not(feature = "analysis"))]
// HttpApi sessions are compiled out without `analysis`, so no call can have
// produced evidence. The changed-line mutation lane runs the default feature
// set, where this adapter is cfg-elided.
#[mutants::skip]
pub(crate) fn session_factory_credential(
    _manager: Option<&crate::session_manager::SessionManagerImpl>,
    _surface_id: &str,
    _url: &str,
) -> Option<CredentialSource> {
    None
}

/// Readiness input: true only when the recorded evidence matches the current
/// configuration and the credential `session_credential` resolves for its
/// surface and URL. Production passes the session factory. Any failure along
/// the way, including an unreadable record or secret, reads as unverified.
pub(crate) async fn http_chat_invocation_verified(
    ai_provider: &AiProviderConfig,
    session_credential: impl FnOnce(&str, &str) -> Option<CredentialSource>,
    evidence_path: Option<&Path>,
) -> bool {
    let Some(path) = evidence_path.map(Path::to_path_buf) else {
        return false;
    };
    // Checked first: without a record, readiness never touches the secret.
    let recorded = tokio::task::spawn_blocking(move || read_recorded_fingerprint(&path))
        .await
        .ok()
        .flatten();
    let Some(recorded) = recorded else {
        return false;
    };
    let Some(target) = HttpChatTarget::resolve(ai_provider) else {
        return false;
    };
    let Some(credential) = session_credential(&target.surface_id, &target.url) else {
        return false;
    };
    target
        .fingerprint(&credential)
        .await
        .is_some_and(|current| current == recorded)
}

#[cfg(test)]
#[path = "ai_invocation_evidence_tests.rs"]
mod tests;
