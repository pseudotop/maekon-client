//! External work-context envelope domain model (ADR-030, #8587/#8589).
//!
//! An envelope carries **only metadata and provenance**. Message bodies, document
//! text, meeting minutes, attachments, HTML, raw provider JSON, OAuth tokens, and
//! ACL member lists go into no field whatsoever (ADR-030 §2). When a body is
//! needed it is handled on a separate raw plane; here we keep only a reference
//! (handle) to it.
//!
//! This is a **completely separate source family** from `ContextEvent`, the PC
//! screen observation. Not mixing the two contracts is ADR-030's first frozen
//! invariant.
//!
//! OOS-TBD: ADR-013 file split — there is room to split the merge rules
//! (`merge_decide`), identity (HMAC), and connector contracts
//! (descriptor/record/page) into submodules. For now this is one domain's
//! cohesive contract, so it stays a single file (extension.rs 1155 as precedent).

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Envelope schema version. Bump it whenever field semantics change.
pub const WORK_CONTEXT_SCHEMA_VERSION: u32 = 1;

/// Domain-separation tag for `source_object_key` (ADR-030 §4).
const OBJECT_KEY_DOMAIN: &[u8] = b"work-context-object/v1\0";

/// Domain-separation tag for the revision fingerprint (ADR-030 §4).
const REVISION_DOMAIN: &[u8] = b"work-context-revision/v1\0";

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Canonical encoding
// ---------------------------------------------------------------------------

/// Records a field with a length prefix plus an explicit absence marker (ADR-030 §4).
///
/// With plain concatenation, `("ab","c")` and `("a","bc")` become the same byte
/// string, so distinct objects fold onto the same key. The length prefix removes
/// that ambiguity. Absence must be distinguishable from the empty string, so the
/// `N`/`P` markers are kept separate.
fn write_field(out: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(v) => {
            out.push(b'P');
            out.extend_from_slice(&(v.len() as u64).to_be_bytes());
            out.extend_from_slice(v.as_bytes());
        }
        None => out.push(b'N'),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------
// Classification axes
// ---------------------------------------------------------------------------

/// Bounded kind classification (ADR-030 §2).
///
/// `Unknown` round-trips through inventory/export, but until it is explicitly
/// mapped it cannot enter search projection, suggestion input, task creation, or
/// graph projection anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkContextKind {
    Message,
    Meeting,
    Document,
    Issue,
    Decision,
    Task,
    Unknown,
}

impl WorkContextKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Meeting => "meeting",
            Self::Document => "document",
            Self::Issue => "issue",
            Self::Decision => "decision",
            Self::Task => "task",
            Self::Unknown => "unknown",
        }
    }

    /// An unrecognized value is not silently dropped but folded into `Unknown` —
    /// under forward compatibility (ADR-030 §13) unknown values must remain readable.
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "message" => Self::Message,
            "meeting" => Self::Meeting,
            "document" => Self::Document,
            "issue" => Self::Issue,
            "decision" => Self::Decision,
            "task" => Self::Task,
            _ => Self::Unknown,
        }
    }

    /// Whether it may be exposed to search projection, suggestions, tasks, or the graph.
    pub fn is_projectable(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// Data classification (ADR-030 §2). `Unknown` is forced to `Restricted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    Public,
    Internal,
    Confidential,
    Restricted,
    Unknown,
}

impl DataClassification {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Confidential => "confidential",
            Self::Restricted => "restricted",
            Self::Unknown => "unknown",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "public" => Self::Public,
            "internal" => Self::Internal,
            "confidential" => Self::Confidential,
            "restricted" => Self::Restricted,
            _ => Self::Unknown,
        }
    }

    /// The effective classification actually used in policy decisions.
    ///
    /// Letting `Unknown` flow through as-is becomes "be lenient because we don't
    /// know", which is fail-open. Narrow it to `Restricted` as ADR-030 §2 requires.
    pub fn effective(self) -> Self {
        match self {
            Self::Unknown => Self::Restricted,
            other => other,
        }
    }
}

/// Revision quality declared by the connector (ADR-030 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionModel {
    /// Declarable only when provider semantics guarantee a total order over a single object.
    Monotonic,
    /// etag/revision supports equality only — once the value changes it cannot be compared.
    Opaque,
    /// No trustworthy version token. Deduplicated by a sanitized content hash.
    ContentHashOnly,
}

impl RevisionModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monotonic => "monotonic",
            Self::Opaque => "opaque",
            Self::ContentHashOnly => "content_hash_only",
        }
    }

    pub fn from_str_lossy(s: &str) -> Option<Self> {
        match s {
            "monotonic" => Some(Self::Monotonic),
            "opaque" => Some(Self::Opaque),
            "content_hash_only" => Some(Self::ContentHashOnly),
            _ => None,
        }
    }

    /// Whether this model allows ordering comparison between two revisions.
    pub fn is_comparable(self) -> bool {
        matches!(self, Self::Monotonic)
    }
}

/// Envelope lifecycle (ADR-030 §6). Monotonic within a single access epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Active,
    Deleted,
    AccessRevoked,
    RetentionExpired,
}

impl Lifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
            Self::AccessRevoked => "access_revoked",
            Self::RetentionExpired => "retention_expired",
        }
    }

    pub fn from_str_lossy(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "deleted" => Some(Self::Deleted),
            "access_revoked" => Some(Self::AccessRevoked),
            "retention_expired" => Some(Self::RetentionExpired),
            _ => None,
        }
    }

    /// A terminal state erases content availability.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Active)
    }
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Identity components of a remote object (ADR-030 §4).
///
/// `account_subject_ref` **is** ADR-031's `account_id` itself (ADR-030 revision I3).
/// Do not independently re-derive or re-hash it. A display name or email address
/// is under no circumstances an account ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceObjectIdentity {
    pub extension_id: String,
    pub install_id: String,
    pub account_subject_ref: String,
    pub remote_type: String,
    pub remote_id: String,
}

impl SourceObjectIdentity {
    /// Length-prefixed canonical encoding (ADR-030 §4).
    pub fn canonical_encoding(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_field(&mut out, Some(&self.extension_id));
        write_field(&mut out, Some(&self.install_id));
        write_field(&mut out, Some(&self.account_subject_ref));
        write_field(&mut out, Some(&self.remote_type));
        write_field(&mut out, Some(&self.remote_id));
        out
    }
}

/// Computes the irreversible local object key (ADR-030 §4).
///
/// `dedupe_key` is a secret generated once per install via CSPRNG and kept only in
/// the Keychain; it must not be derived from a value that is already persisted in
/// plaintext or sent to the server, such as `device_identity.device_id` (ADR-030
/// revision B2). Only then do a retained key or a public export avoid becoming a
/// provider/account ID dictionary.
///
/// Losing the key is a recoverable event — deduplication degrades to "everything is
/// new" and the ledger re-converges on the revision basis. Do not fall back to a
/// keyless hash.
pub fn compute_source_object_key(dedupe_key: &[u8], identity: &SourceObjectIdentity) -> String {
    #[allow(
        clippy::expect_used,
        reason = "hmac 0.13 implements new_from_slice as unconditional Ok for arbitrary key lengths"
    )]
    let mut mac = <HmacSha256 as KeyInit>::new_from_slice(dedupe_key)
        .expect("HMAC-SHA256 accepts keys of any length");
    mac.update(OBJECT_KEY_DOMAIN);
    mac.update(&identity.canonical_encoding());
    hex(&mac.finalize().into_bytes())
}

/// Computes the revision fingerprint (ADR-030 §4).
///
/// Equal fingerprints mean `duplicate`, and the stored result is replayed as-is.
#[allow(clippy::too_many_arguments)]
pub fn compute_revision_fingerprint(
    revision_model: RevisionModel,
    remote_revision: Option<&str>,
    etag: Option<&str>,
    source_updated_at: Option<DateTime<Utc>>,
    content_hash: &str,
    lifecycle: Lifecycle,
) -> String {
    let mut buf = Vec::new();
    buf.extend_from_slice(REVISION_DOMAIN);
    write_field(&mut buf, Some(revision_model.as_str()));
    write_field(&mut buf, remote_revision);
    write_field(&mut buf, etag);
    let updated = source_updated_at.map(|t| t.to_rfc3339());
    write_field(&mut buf, updated.as_deref());
    write_field(&mut buf, Some(content_hash));
    write_field(&mut buf, Some(lifecycle.as_str()));

    let mut hasher = Sha256::new();
    hasher.update(&buf);
    hex(&hasher.finalize())
}

/// Deterministic conflict ID that quarantines incomparable revisions (ADR-030 §6 rule 7).
///
/// The fingerprints are fed in sorted order, so delivery order does not change the ID.
pub fn compute_conflict_id(fingerprints: &BTreeSet<String>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"work-context-conflict/v1\0");
    for fp in fingerprints {
        hasher.update((fp.len() as u64).to_be_bytes());
        hasher.update(fp.as_bytes());
    }
    hex(&hasher.finalize())
}

// ---------------------------------------------------------------------------
// Relation & evidence references
// ---------------------------------------------------------------------------

/// Bounded kinds of relation reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Thread,
    Parent,
    Project,
    Actor,
}

/// Opaque relation reference (ADR-030 §2).
///
/// Carries no names, emails, token-bearing URLs, or raw ACL entries. It carries
/// only a bounded kind, an opaque source ID, and an optional fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationRef {
    pub kind: RelationKind,
    pub opaque_source_id: String,
    pub fingerprint: Option<String>,
}

/// Access snapshot — evidence, not authorization (ADR-030 §8).
///
/// A stored snapshot does not authorize any later live read. Every read is
/// re-evaluated live each time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessSnapshot {
    /// Access state at observation time (e.g. `granted`, `denied`, `unknown`).
    pub state: String,
    pub observed_at: DateTime<Utc>,
}

impl AccessSnapshot {
    /// `unknown` access is treated as denial (fail-closed).
    pub fn is_denied(&self) -> bool {
        self.state != "granted"
    }
}

/// Consent snapshot — likewise evidence, not authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentSnapshot {
    pub state: String,
    pub observed_at: DateTime<Utc>,
}

impl ConsentSnapshot {
    pub fn is_denied(&self) -> bool {
        self.state != "granted"
    }
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// External work-context envelope (ADR-030 §2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkContextEnvelope {
    // Local identity
    pub envelope_id: String,
    pub schema_version: u32,
    pub access_epoch_id: i64,

    // Source identity
    pub identity: SourceObjectIdentity,
    /// HMAC-based irreversible local key.
    pub source_object_key: String,

    // Version
    pub revision_model: RevisionModel,
    pub remote_revision: Option<String>,
    pub etag: Option<String>,
    /// Normalized order value, populated only when there is provider grounding for it.
    pub source_order: Option<i64>,
    /// Hash of the sanitized content — not ciphertext, raw ACL, or provider JSON bytes.
    pub content_hash: String,
    pub revision_fingerprint: String,

    // Classification
    pub kind: WorkContextKind,
    pub classification: DataClassification,
    pub retention_class: Option<String>,

    // Time (§3 — each field has a different authority)
    pub occurred_at: Option<DateTime<Utc>>,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub ingested_at: DateTime<Utc>,

    // Relations
    pub relations: Vec<RelationRef>,

    // Authorization evidence
    pub access_snapshot: Option<AccessSnapshot>,
    pub consent_snapshot: Option<ConsentSnapshot>,

    // Provenance
    pub ingest_run_id: String,
    pub prior_envelope_id: Option<String>,
    pub source_cursor_digest: Option<String>,
    pub projection_ref: Option<String>,
    pub raw_blob_ref: Option<String>,

    // Lifecycle
    pub lifecycle: Lifecycle,
}

impl WorkContextEnvelope {
    /// Whether it may be exposed to search, suggestion, task, or graph projection.
    ///
    /// An unknown kind is excluded until mapped (§2), and a terminal lifecycle is
    /// excluded because it has no content availability (§6).
    pub fn is_projectable(&self) -> bool {
        self.kind.is_projectable() && self.lifecycle == Lifecycle::Active
    }

    /// Effective classification used in policy decisions (`unknown` → `restricted`).
    pub fn effective_classification(&self) -> DataClassification {
        self.classification.effective()
    }

    /// Local uniqueness key `(source_object_key, access_epoch_id, revision_fingerprint)`.
    pub fn uniqueness_key(&self) -> (String, i64, String) {
        (
            self.source_object_key.clone(),
            self.access_epoch_id,
            self.revision_fingerprint.clone(),
        )
    }
}

// ---------------------------------------------------------------------------
// Clock-rollback defense
// ---------------------------------------------------------------------------

/// Effective current time used for TTL evaluation (ADR-030 revision I5, isomorphic to ADR-028 §7).
///
/// If the system clock is pushed backward, data may expire **earlier** but never
/// later. The TTLs across the four planes are this design's entire privacy
/// argument, so letting a rolled-back clock keep a raw blob indefinitely "not yet
/// expired" would silently break the retention promise.
pub fn effective_now(
    current_utc: DateTime<Utc>,
    last_ingested_at: Option<DateTime<Utc>>,
) -> DateTime<Utc> {
    match last_ingested_at {
        Some(prev) if prev > current_utc => prev,
        _ => current_utc,
    }
}

// ---------------------------------------------------------------------------
// Merge decision (ADR-030 §6)
// ---------------------------------------------------------------------------

/// Deterministic merge result for a single source object and access epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum MergeOutcome {
    /// Identical fingerprint — replay the stored result.
    Duplicate,
    /// Access/consent denial dominates the content revision.
    AccessRevoked,
    /// A higher comparable active revision replaces the existing projection.
    Accepted,
    /// A lower comparable revision — cannot replace nor revive.
    Stale,
    /// Same comparable revision but a different content hash.
    RevisionConflict,
    /// A delete/retention tombstone suppresses an equal or lower active revision.
    Suppressed,
    /// A changed opaque revision — quarantine it without exposing a winner.
    Incomparable { conflict_id: String },
}

/// Merge decision input — existing state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeExisting {
    pub revision_fingerprint: String,
    pub revision_model: RevisionModel,
    pub source_order: Option<i64>,
    pub content_hash: String,
    pub lifecycle: Lifecycle,
}

/// Merge decision input — the newly arrived record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeIncoming {
    pub revision_fingerprint: String,
    pub revision_model: RevisionModel,
    pub source_order: Option<i64>,
    pub content_hash: String,
    pub lifecycle: Lifecycle,
    /// Whether the provider contract explicitly supports undelete (§6 rule 8).
    pub provider_supports_undelete: bool,
    /// Whether access/consent is currently in a denied state.
    pub access_denied: bool,
}

/// Implements ADR-030 §6's merge ordering exactly.
///
/// The order itself is the contract — reordering the rules would let delivery
/// order decide visibility, breaking delete-before-update safety.
pub fn merge_decide(existing: Option<&MergeExisting>, incoming: &MergeIncoming) -> MergeOutcome {
    // Rule 2 is evaluated first, regardless of whether existing state is present —
    // access denial dominates the content revision.
    if incoming.access_denied {
        return MergeOutcome::AccessRevoked;
    }

    let Some(existing) = existing else {
        // First observation. If what arrives is a tombstone, it is recorded as suppressed as-is.
        return match incoming.lifecycle {
            Lifecycle::Active => MergeOutcome::Accepted,
            Lifecycle::AccessRevoked => MergeOutcome::AccessRevoked,
            _ => MergeOutcome::Suppressed,
        };
    };

    // Rule 1: identical fingerprint = duplicate.
    if existing.revision_fingerprint == incoming.revision_fingerprint {
        return MergeOutcome::Duplicate;
    }

    // Rule 6: if the existing state is a tombstone, suppress an equal or lower active revision.
    if existing.lifecycle.is_terminal() && incoming.lifecycle == Lifecycle::Active {
        // Rule 8: undelete only with explicit provider support + a strictly higher comparable revision.
        let strictly_higher = comparable_order(existing, incoming) == Some(Ordering2::Higher);
        let undelete_ok = existing.lifecycle == Lifecycle::Deleted
            && incoming.provider_supports_undelete
            && strictly_higher;
        if undelete_ok {
            return MergeOutcome::Accepted;
        }
        return MergeOutcome::Suppressed;
    }

    // Rule 6: a new delete/retention tombstone suppresses the existing active revision.
    // The existing that reaches here is active (the tombstone path was filtered
    // above), so the active → terminal transition is always allowed. It must be
    // handled as Suppressed rather than Accepted so that content is erased and a
    // tombstone remains — this is not an active insertion.
    if incoming.lifecycle.is_terminal() {
        return MergeOutcome::Suppressed;
    }

    // Rules 3/4/5/7: active-vs-active comparison.
    match comparable_order(existing, incoming) {
        Some(Ordering2::Higher) => MergeOutcome::Accepted,
        Some(Ordering2::Lower) => MergeOutcome::Stale,
        Some(Ordering2::Same) => {
            if existing.content_hash == incoming.content_hash {
                MergeOutcome::Duplicate
            } else {
                MergeOutcome::RevisionConflict
            }
        }
        None => {
            let mut set = BTreeSet::new();
            set.insert(existing.revision_fingerprint.clone());
            set.insert(incoming.revision_fingerprint.clone());
            MergeOutcome::Incomparable {
                conflict_id: compute_conflict_id(&set),
            }
        }
    }
}

/// Relative order within a comparable model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ordering2 {
    Higher,
    Lower,
    Same,
}

/// Decides whether two revisions are comparable and, if so, which one is higher.
///
/// If either is not `monotonic`, or either lacks an order value, they are
/// **incomparable**. Do not invent an order from lexical strings or timestamps —
/// ADR-030 §5 forbids a monotonic claim without provider grounding.
fn comparable_order(existing: &MergeExisting, incoming: &MergeIncoming) -> Option<Ordering2> {
    if !existing.revision_model.is_comparable() || !incoming.revision_model.is_comparable() {
        return None;
    }
    match (existing.source_order, incoming.source_order) {
        (Some(a), Some(b)) if b > a => Some(Ordering2::Higher),
        (Some(a), Some(b)) if b < a => Some(Ordering2::Lower),
        (Some(_), Some(_)) => Some(Ordering2::Same),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Connector contracts
// ---------------------------------------------------------------------------

/// Source capability descriptor declared by the connector (ADR-030 §5, §9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSourceDescriptor {
    pub extension_id: String,
    pub install_id: String,
    pub remote_type: String,
    pub revision_model: RevisionModel,
    /// Whether the provider supplies an explicit delete signal
    /// (event/webhook/audit entry/object-scoped not-found). Disappearing from a
    /// listing is not evidence of deletion (ADR-030 revision I6).
    pub has_explicit_delete_signal: bool,
    /// Whether the provider contract supports undelete (§6 rule 8).
    pub supports_undelete: bool,
    /// Maximum number of records a single page can hold.
    pub max_page_records: u32,
}

impl ContextSourceDescriptor {
    /// Whether it can be advertised as a supported connector (ADR-030 §5).
    ///
    /// A connector that cannot deterministically handle deletion/access loss must
    /// report `unavailable` and cannot be marked as supported. If `content_hash_only`
    /// also lacks an explicit delete signal, deletion can never be known, so it
    /// cannot be advertised.
    pub fn is_advertisable(&self) -> bool {
        if self.max_page_records == 0 {
            return false;
        }
        self.has_explicit_delete_signal
    }
}

/// A record carried within a single page (ADR-030 §9).
///
/// `raw_payload_handle` is not a public DTO, and the adapter does not write it to
/// storage directly. Only the application use case commits a page atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSourceRecord {
    pub identity: SourceObjectIdentity,
    pub kind: WorkContextKind,
    pub classification: DataClassification,
    pub remote_revision: Option<String>,
    pub etag: Option<String>,
    pub source_order: Option<i64>,
    pub content_hash: String,
    pub occurred_at: Option<DateTime<Utc>>,
    pub source_updated_at: Option<DateTime<Utc>>,
    pub observed_at: DateTime<Utc>,
    pub relations: Vec<RelationRef>,
    pub lifecycle: Lifecycle,
    pub raw_payload_handle: Option<String>,
}

/// A bounded page returned by the connector (ADR-030 §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSourcePage {
    pub records: Vec<ContextSourceRecord>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    /// Page/checkpoint digest — recorded in provenance.
    pub page_digest: String,
    /// The access epoch at the time this page was fetched.
    pub access_epoch_id: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        vec![7u8; 32]
    }

    fn identity(remote_id: &str) -> SourceObjectIdentity {
        SourceObjectIdentity {
            extension_id: "com.maekon.calendar".into(),
            install_id: "inst_1".into(),
            account_subject_ref: "acct_opaque_1".into(),
            remote_type: "event".into(),
            remote_id: remote_id.into(),
        }
    }

    #[test]
    fn length_prefix_removes_boundary_ambiguity() {
        // With plain concatenation the two identities would become the same byte string and the keys would collide.
        let mut a = identity("x");
        a.remote_type = "ab".into();
        a.remote_id = "c".into();
        let mut b = identity("x");
        b.remote_type = "a".into();
        b.remote_id = "bc".into();
        assert_ne!(a.canonical_encoding(), b.canonical_encoding());
        assert_ne!(
            compute_source_object_key(&key(), &a),
            compute_source_object_key(&key(), &b)
        );
    }

    #[test]
    fn object_key_is_account_and_install_scoped() {
        // Even if the provider reuses the same remote_id, different account/install must not collide.
        let mut other_account = identity("evt_1");
        other_account.account_subject_ref = "acct_opaque_2".into();
        let mut other_install = identity("evt_1");
        other_install.install_id = "inst_2".into();

        let base = compute_source_object_key(&key(), &identity("evt_1"));
        assert_ne!(base, compute_source_object_key(&key(), &other_account));
        assert_ne!(base, compute_source_object_key(&key(), &other_install));
    }

    #[test]
    fn object_key_changes_with_the_dedupe_key() {
        // If the key is not mixed in, it is just a hash rather than an HMAC — the dictionary-attack defense disappears.
        let a = compute_source_object_key(&key(), &identity("evt_1"));
        let b = compute_source_object_key(&[9u8; 32], &identity("evt_1"));
        assert_ne!(a, b);
    }

    #[test]
    fn object_key_is_deterministic_and_matches_canonical_vector() {
        let identity = identity("evt_1");
        let first = compute_source_object_key(&key(), &identity);

        assert_eq!(first, compute_source_object_key(&key(), &identity));
        assert_eq!(
            first,
            "6f8eac34004b94e9b951e3fd1270da6d2e8723602cc300a311737ae6b286d8af"
        );
    }

    #[test]
    fn missing_value_differs_from_empty_string() {
        let with_none = compute_revision_fingerprint(
            RevisionModel::Opaque,
            None,
            None,
            None,
            "hash",
            Lifecycle::Active,
        );
        let with_empty = compute_revision_fingerprint(
            RevisionModel::Opaque,
            Some(""),
            None,
            None,
            "hash",
            Lifecycle::Active,
        );
        assert_ne!(with_none, with_empty);
    }

    #[test]
    fn unknown_classification_is_enforced_as_restricted() {
        assert_eq!(
            DataClassification::Unknown.effective(),
            DataClassification::Restricted
        );
        assert_eq!(
            DataClassification::from_str_lossy("nonsense").effective(),
            DataClassification::Restricted
        );
    }

    #[test]
    fn unknown_kind_is_not_projectable() {
        assert!(!WorkContextKind::Unknown.is_projectable());
        assert!(!WorkContextKind::from_str_lossy("brand_new_kind").is_projectable());
        assert!(WorkContextKind::Meeting.is_projectable());
    }

    #[test]
    fn clock_rollback_cannot_extend_a_ttl() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let later = DateTime::parse_from_rfc3339("2026-07-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        // Even if the clock is pushed backward, the effective now does not drop below the last ingest time.
        assert_eq!(effective_now(now, Some(later)), later);
        // Normal forward progress passes through unchanged.
        assert_eq!(effective_now(later, Some(now)), later);
        assert_eq!(effective_now(now, None), now);
    }

    fn existing(order: i64, hash: &str, lifecycle: Lifecycle) -> MergeExisting {
        MergeExisting {
            revision_fingerprint: format!("fp_{order}_{hash}_{}", lifecycle.as_str()),
            revision_model: RevisionModel::Monotonic,
            source_order: Some(order),
            content_hash: hash.into(),
            lifecycle,
        }
    }

    fn incoming(order: i64, hash: &str, lifecycle: Lifecycle) -> MergeIncoming {
        MergeIncoming {
            revision_fingerprint: format!("fp_{order}_{hash}_{}", lifecycle.as_str()),
            revision_model: RevisionModel::Monotonic,
            source_order: Some(order),
            content_hash: hash.into(),
            lifecycle,
            provider_supports_undelete: false,
            access_denied: false,
        }
    }

    #[test]
    fn identical_fingerprint_is_duplicate() {
        let e = existing(1, "h", Lifecycle::Active);
        let i = incoming(1, "h", Lifecycle::Active);
        assert_eq!(merge_decide(Some(&e), &i), MergeOutcome::Duplicate);
    }

    #[test]
    fn access_denial_dominates_content_revisions() {
        let e = existing(1, "h", Lifecycle::Active);
        let mut i = incoming(9, "h2", Lifecycle::Active);
        i.access_denied = true;
        // Even a much higher revision loses to access denial.
        assert_eq!(merge_decide(Some(&e), &i), MergeOutcome::AccessRevoked);
    }

    #[test]
    fn higher_revision_replaces_and_lower_is_stale() {
        let e = existing(5, "h", Lifecycle::Active);
        assert_eq!(
            merge_decide(Some(&e), &incoming(6, "h2", Lifecycle::Active)),
            MergeOutcome::Accepted
        );
        assert_eq!(
            merge_decide(Some(&e), &incoming(4, "h3", Lifecycle::Active)),
            MergeOutcome::Stale
        );
    }

    #[test]
    fn same_revision_with_different_content_is_a_conflict() {
        let e = existing(5, "h", Lifecycle::Active);
        assert_eq!(
            merge_decide(Some(&e), &incoming(5, "different", Lifecycle::Active)),
            MergeOutcome::RevisionConflict
        );
    }

    #[test]
    fn tombstone_blocks_a_replayed_older_update() {
        // delete-before-update: even if a delete arrives first and a stale update is
        // then replayed, it does not revive. Delivery order does not decide visibility.
        let deleted = existing(5, "h", Lifecycle::Deleted);
        assert_eq!(
            merge_decide(Some(&deleted), &incoming(4, "old", Lifecycle::Active)),
            MergeOutcome::Suppressed
        );
        // The same revision behaves the same way.
        assert_eq!(
            merge_decide(Some(&deleted), &incoming(5, "same", Lifecycle::Active)),
            MergeOutcome::Suppressed
        );
    }

    #[test]
    fn undelete_requires_provider_support_and_strictly_higher_revision() {
        let deleted = existing(5, "h", Lifecycle::Deleted);

        // No provider support → suppressed even at a higher revision.
        assert_eq!(
            merge_decide(Some(&deleted), &incoming(6, "new", Lifecycle::Active)),
            MergeOutcome::Suppressed
        );

        // Support + strictly higher → allowed.
        let mut ok = incoming(6, "new", Lifecycle::Active);
        ok.provider_supports_undelete = true;
        assert_eq!(merge_decide(Some(&deleted), &ok), MergeOutcome::Accepted);

        // Even with support, the same revision is not allowed.
        let mut same = incoming(5, "new", Lifecycle::Active);
        same.provider_supports_undelete = true;
        assert_eq!(
            merge_decide(Some(&deleted), &same),
            MergeOutcome::Suppressed
        );
    }

    #[test]
    fn access_revoke_is_never_undone_by_undelete() {
        // Recovering from access loss requires a new epoch — the undelete path must not break through it.
        let revoked = existing(5, "h", Lifecycle::AccessRevoked);
        let mut i = incoming(99, "new", Lifecycle::Active);
        i.provider_supports_undelete = true;
        assert_eq!(merge_decide(Some(&revoked), &i), MergeOutcome::Suppressed);
    }

    #[test]
    fn changed_opaque_revisions_are_quarantined_not_ranked() {
        let e = MergeExisting {
            revision_fingerprint: "fp_a".into(),
            revision_model: RevisionModel::Opaque,
            source_order: None,
            content_hash: "h1".into(),
            lifecycle: Lifecycle::Active,
        };
        let i = MergeIncoming {
            revision_fingerprint: "fp_b".into(),
            revision_model: RevisionModel::Opaque,
            source_order: None,
            content_hash: "h2".into(),
            lifecycle: Lifecycle::Active,
            provider_supports_undelete: false,
            access_denied: false,
        };
        match merge_decide(Some(&e), &i) {
            MergeOutcome::Incomparable { conflict_id } => assert!(!conflict_id.is_empty()),
            other => panic!("expected incomparable, got {other:?}"),
        }
    }

    #[test]
    fn conflict_id_is_delivery_order_independent() {
        let mut a = BTreeSet::new();
        a.insert("fp_a".to_string());
        a.insert("fp_b".to_string());
        let mut b = BTreeSet::new();
        b.insert("fp_b".to_string());
        b.insert("fp_a".to_string());
        assert_eq!(compute_conflict_id(&a), compute_conflict_id(&b));
    }

    #[test]
    fn connector_without_explicit_delete_signal_is_not_advertisable() {
        // A connector that infers deletion from absence in a listing cannot be advertised as supported (revision I6).
        let mut d = ContextSourceDescriptor {
            extension_id: "com.maekon.calendar".into(),
            install_id: "inst_1".into(),
            remote_type: "event".into(),
            revision_model: RevisionModel::ContentHashOnly,
            has_explicit_delete_signal: false,
            supports_undelete: false,
            max_page_records: 100,
        };
        assert!(!d.is_advertisable());
        d.has_explicit_delete_signal = true;
        assert!(d.is_advertisable());
        // A page cap of 0 means unbounded collection, so it is likewise not advertisable.
        d.max_page_records = 0;
        assert!(!d.is_advertisable());
    }

    #[test]
    fn lexical_or_timestamp_order_is_never_invented() {
        // The opaque model does not compare even when order values are present.
        let e = MergeExisting {
            revision_fingerprint: "fp_a".into(),
            revision_model: RevisionModel::Opaque,
            source_order: Some(1),
            content_hash: "h1".into(),
            lifecycle: Lifecycle::Active,
        };
        let i = MergeIncoming {
            revision_fingerprint: "fp_b".into(),
            revision_model: RevisionModel::Opaque,
            source_order: Some(2),
            content_hash: "h2".into(),
            lifecycle: Lifecycle::Active,
            provider_supports_undelete: false,
            access_denied: false,
        };
        assert!(matches!(
            merge_decide(Some(&e), &i),
            MergeOutcome::Incomparable { .. }
        ));
    }

    #[test]
    fn first_observation_of_a_tombstone_is_suppressed_not_accepted() {
        let i = incoming(1, "h", Lifecycle::Deleted);
        assert_eq!(merge_decide(None, &i), MergeOutcome::Suppressed);
    }

    #[test]
    fn a_delete_over_an_active_revision_suppresses_not_accepts() {
        // §6 rule 6: when a delete arrives on top of an active revision it is suppressed.
        // Handling it as Accepted would re-insert it as active and nullify the delete (regression guard).
        let active = existing(5, "h", Lifecycle::Active);
        let delete = incoming(6, "gone", Lifecycle::Deleted);
        assert_eq!(
            merge_decide(Some(&active), &delete),
            MergeOutcome::Suppressed
        );

        // A retention-expired tombstone behaves the same way.
        let expire = incoming(6, "gone", Lifecycle::RetentionExpired);
        assert_eq!(
            merge_decide(Some(&active), &expire),
            MergeOutcome::Suppressed
        );
    }
}
