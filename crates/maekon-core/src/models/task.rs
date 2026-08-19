//! Durable task lifecycle models (ADR-028, #8577).
//!
//! A [`TaskCandidate`] is a reviewable, non-authoritative proposal. A
//! [`TodoItem`] is a user-confirmed durable record. Generation may create a
//! candidate, but only an explicit human confirmation may create a to-do. These
//! models and the pure transition functions below are the authority; the
//! storage adapter executes already-decided transitions and never invents one.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Reviewable candidate lifecycle state (ADR-028 §2). `Confirmed`, `Dismissed`,
/// and `Expired` are terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CandidateState {
    /// Freshly generated, awaiting an explicit human decision.
    Proposed,
    /// The user confirmed the candidate; exactly one to-do originates from it.
    Confirmed,
    /// The user rejected the candidate. Terminal, content cleared.
    Dismissed,
    /// The candidate's TTL elapsed without a decision. Terminal, content cleared.
    Expired,
}

impl CandidateState {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Proposed => "PROPOSED",
            Self::Confirmed => "CONFIRMED",
            Self::Dismissed => "DISMISSED",
            Self::Expired => "EXPIRED",
        }
    }

    /// Parse a known candidate state. Unknown values fail closed for mutation but
    /// are preserved elsewhere as raw strings for export round-tripping.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "PROPOSED" => Some(Self::Proposed),
            "CONFIRMED" => Some(Self::Confirmed),
            "DISMISSED" => Some(Self::Dismissed),
            "EXPIRED" => Some(Self::Expired),
            _ => None,
        }
    }

    /// Whether this state accepts no further transitions.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Proposed)
    }

    /// Whether the `proposed -> self` transition is allowed. Only the three
    /// terminal states are reachable, and only from `Proposed`.
    pub fn is_allowed_from_proposed(&self) -> bool {
        matches!(self, Self::Confirmed | Self::Dismissed | Self::Expired)
    }
}

/// Confirmed to-do lifecycle state (ADR-028 §2). `Done` and `Cancelled` are
/// terminal; reopening is forbidden (a future reopen mints a new to-do).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TodoState {
    /// Just confirmed, not yet started.
    Confirmed,
    /// Actively being worked on.
    InProgress,
    /// Paused, waiting on a blocker or external event.
    Waiting,
    /// Completed. Terminal.
    Done,
    /// Cancelled before completion. Terminal.
    Cancelled,
}

impl TodoState {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Confirmed => "CONFIRMED",
            Self::InProgress => "IN_PROGRESS",
            Self::Waiting => "WAITING",
            Self::Done => "DONE",
            Self::Cancelled => "CANCELLED",
        }
    }

    /// Parse a known to-do state; unknown values fail closed for mutation.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "CONFIRMED" => Some(Self::Confirmed),
            "IN_PROGRESS" => Some(Self::InProgress),
            "WAITING" => Some(Self::Waiting),
            "DONE" => Some(Self::Done),
            "CANCELLED" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Whether this state accepts no further transitions.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }

    /// The authoritative allowed-transition matrix (ADR-028 §2). Self-transitions,
    /// reverse transitions to `Confirmed`, and reopening a terminal state are all
    /// forbidden.
    pub fn can_transition_to(&self, target: TodoState) -> bool {
        use TodoState::*;
        match self {
            Confirmed => matches!(target, InProgress | Waiting | Done | Cancelled),
            InProgress => matches!(target, Waiting | Done | Cancelled),
            Waiting => matches!(target, InProgress | Done | Cancelled),
            Done | Cancelled => false,
        }
    }
}

/// Origin of a task candidate (ADR-028 §4). Unknown kinds round-trip through
/// storage/export but are fail-closed for acquisition and mutation.
///
/// Serialized as a plain SCREAMING_SNAKE_CASE string so unknown forward-compatible
/// values survive JSON round-trips exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// Derived from the local current screen scene.
    LocalCurrentScene,
    /// Derived from an interruption (restore-context) source.
    Interruption,
    /// Derived from an external extension context.
    ExtensionContext,
    /// A forward-compatible value a newer client understands.
    Other(String),
}

impl Serialize for SourceKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_sql_str())
    }
}

impl<'de> Deserialize<'de> for SourceKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_sql_str(&raw))
    }
}

impl SourceKind {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> String {
        match self {
            Self::LocalCurrentScene => "LOCAL_CURRENT_SCENE".to_string(),
            Self::Interruption => "INTERRUPTION".to_string(),
            Self::ExtensionContext => "EXTENSION_CONTEXT".to_string(),
            Self::Other(raw) => raw.clone(),
        }
    }

    /// Parse from storage. Unknown values become [`SourceKind::Other`] so they
    /// round-trip; callers must treat `Other` as fail-closed for mutation.
    pub fn from_sql_str(value: &str) -> Self {
        match value {
            "LOCAL_CURRENT_SCENE" => Self::LocalCurrentScene,
            "INTERRUPTION" => Self::Interruption,
            "EXTENSION_CONTEXT" => Self::ExtensionContext,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether this is a forward-compatible unknown kind (fail-closed).
    pub fn is_known(&self) -> bool {
        !matches!(self, Self::Other(_))
    }

    /// Whether this kind lacks a stable upstream object/revision anchor, so its
    /// dedupe namespace must fold a per-capture occurrence discriminator
    /// (ADR-028 Amendment B2).
    pub fn is_anchorless(&self) -> bool {
        matches!(self, Self::LocalCurrentScene)
    }
}

/// Monotonic source lifecycle (ADR-028 §4). Advances away from `Active` only.
///
/// Serialized as a plain SCREAMING_SNAKE_CASE string so unknown forward-compatible
/// values survive JSON round-trips exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceLifecycle {
    /// Source is live and acquirable.
    Active,
    /// Source object was deleted upstream.
    Deleted,
    /// Access to the source was revoked (consent/auth loss).
    AccessRevoked,
    /// Source content passed its retention boundary.
    RetentionExpired,
    /// Forward-compatible unknown lifecycle value.
    Other(String),
}

impl Serialize for SourceLifecycle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.as_sql_str())
    }
}

impl<'de> Deserialize<'de> for SourceLifecycle {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_sql_str(&raw))
    }
}

impl SourceLifecycle {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> String {
        match self {
            Self::Active => "ACTIVE".to_string(),
            Self::Deleted => "DELETED".to_string(),
            Self::AccessRevoked => "ACCESS_REVOKED".to_string(),
            Self::RetentionExpired => "RETENTION_EXPIRED".to_string(),
            Self::Other(raw) => raw.clone(),
        }
    }

    /// Parse from storage; unknown values round-trip as [`SourceLifecycle::Other`].
    pub fn from_sql_str(value: &str) -> Self {
        match value {
            "ACTIVE" => Self::Active,
            "DELETED" => Self::Deleted,
            "ACCESS_REVOKED" => Self::AccessRevoked,
            "RETENTION_EXPIRED" => Self::RetentionExpired,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether the source is still live.
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Interruption-source outcome (ADR-028 §4). Valid only for
/// [`SourceKind::Interruption`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SourceOutcome {
    /// No resume/abandon decision exists; candidate still proposed.
    Pending,
    /// The source recorded a resume.
    Resumed,
    /// The user explicitly chose not to restore.
    Abandoned,
    /// The candidate TTL elapsed without a decision.
    Expired,
}

impl SourceOutcome {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Resumed => "RESUMED",
            Self::Abandoned => "ABANDONED",
            Self::Expired => "EXPIRED",
        }
    }

    /// Parse a known outcome value.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "PENDING" => Some(Self::Pending),
            "RESUMED" => Some(Self::Resumed),
            "ABANDONED" => Some(Self::Abandoned),
            "EXPIRED" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// The entity a transition receipt belongs to (ADR-028 §3, Amendment I3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskEntityKind {
    /// A [`TaskCandidate`].
    Candidate,
    /// A [`TodoItem`].
    Todo,
    /// A directed [`TaskBlocker`] edge (keyed on the blocked to-do).
    TodoBlocker,
}

impl TaskEntityKind {
    /// Wire/SQL string form.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            Self::Candidate => "CANDIDATE",
            Self::Todo => "TODO",
            Self::TodoBlocker => "TODO_BLOCKER",
        }
    }

    /// Parse a known entity kind.
    pub fn from_sql_str(value: &str) -> Option<Self> {
        match value {
            "CANDIDATE" => Some(Self::Candidate),
            "TODO" => Some(Self::Todo),
            "TODO_BLOCKER" => Some(Self::TodoBlocker),
            _ => None,
        }
    }
}

/// Typed multi-outcome result of a task command (ADR-028 §8 + Amendment B1).
///
/// `AlreadyTransitioned` is an idempotent no-op success (the desired end-state was
/// reached, possibly by another key); `RevisionConflict` means refetch-then-retry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum TaskOutcome {
    /// A candidate was confirmed into a new to-do.
    Confirmed {
        /// The originating candidate id.
        candidate_id: String,
        /// The newly created to-do id.
        todo_id: String,
        /// The candidate's post-transition revision.
        revision: i64,
    },
    /// A candidate was dismissed.
    Dismissed {
        /// The candidate id.
        candidate_id: String,
        /// The candidate's post-transition revision.
        revision: i64,
    },
    /// A candidate expired.
    Expired {
        /// The candidate id.
        candidate_id: String,
        /// The candidate's post-transition revision.
        revision: i64,
    },
    /// A to-do reached the requested state.
    Transitioned {
        /// The to-do id.
        todo_id: String,
        /// The new state.
        state: TodoState,
        /// The to-do's post-transition revision.
        revision: i64,
    },
    /// The entity's revision differs and the target was not reached; refetch and
    /// retry.
    RevisionConflict,
    /// The requested target state was already reached (idempotent no-op success).
    AlreadyTransitioned {
        /// The current terminal/target state name.
        current_state: String,
    },
    /// The same idempotency key was reused with different request content.
    IdempotencyMismatch,
    /// Live consent for the source category is absent at mutation time.
    ConsentRequired,
    /// The source lifecycle is not active and the operation needs live source
    /// acquisition.
    SourceUnavailable,
}

/// Immutable, minimized source provenance (ADR-028 §4). Never carries raw OCR,
/// screenshots, accessibility trees, titles, tokens, or extension payloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSourceRef {
    /// Origin kind of the candidate.
    pub source_kind: SourceKind,
    /// Optional stable extension type id; never an executable path.
    pub extension_id: Option<String>,
    /// Optional opaque installation id; never a credential.
    pub install_id: Option<String>,
    /// Optional opaque account subject ref; never email, name, token, or secret.
    pub account_subject_ref: Option<String>,
    /// Optional source-owned object id.
    pub upstream_object_id: Option<String>,
    /// Optional opaque source version.
    pub upstream_revision: Option<String>,
    /// Optional opaque source etag.
    pub upstream_etag: Option<String>,
    /// When the source event happened, if known.
    pub occurred_at: Option<DateTime<Utc>>,
    /// When Maekon observed it.
    pub observed_at: DateTime<Utc>,
    /// Stable non-secret namespace scoped to the source subject. For anchor-less
    /// kinds it MUST fold a per-capture occurrence discriminator (Amendment B2).
    pub dedupe_namespace: String,
    /// `sha256:<hex>` of canonical sanitized candidate content; never raw bytes.
    pub content_hash: String,
    /// Monotonic source lifecycle.
    pub lifecycle: SourceLifecycle,
    /// Optional interruption outcome; valid only for `Interruption`.
    pub source_outcome: Option<SourceOutcome>,
}

/// A reviewable, non-authoritative task proposal (ADR-028 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCandidate {
    /// `tcand`-prefixed ULID.
    pub id: String,
    /// Lifecycle state.
    pub state: CandidateState,
    /// Sanitized title; `None` once cleared in a terminal transition.
    pub title: Option<String>,
    /// Sanitized body; `None` once cleared in a terminal transition.
    pub body: Option<String>,
    /// Proposed due time; a proposal until the human confirms it.
    pub proposed_due: Option<DateTime<Utc>>,
    /// Proposed opaque local owner reference; a proposal until confirmed.
    pub proposed_owner_ref: Option<String>,
    /// When the proposed candidate expires (<= 7 days after creation or the
    /// source's earlier expiry).
    pub expires_at: DateTime<Utc>,
    /// Immutable provenance.
    pub source_ref: TaskSourceRef,
    /// Globally unique at-least-once ingestion guard key.
    pub dedupe_key: String,
    /// Compare-and-swap revision.
    pub revision: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A user-confirmed durable task (ADR-028 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    /// `todo`-prefixed ULID.
    pub id: String,
    /// Lifecycle state.
    pub state: TodoState,
    /// Sanitized title.
    pub title: String,
    /// Sanitized body.
    pub body: Option<String>,
    /// Confirmed due time.
    pub due: Option<DateTime<Utc>>,
    /// Confirmed opaque local owner reference.
    pub owner_ref: Option<String>,
    /// The unique originating candidate id.
    pub origin_candidate_id: String,
    /// Optional prior to-do this one supersedes (future reopen flow).
    pub supersedes_todo_id: Option<String>,
    /// Compare-and-swap revision.
    pub revision: i64,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last update timestamp.
    pub updated_at: DateTime<Utc>,
}

/// A directed `blocked -> blocker` edge between two existing to-dos (ADR-028 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskBlocker {
    /// The to-do that is blocked.
    pub blocked_todo_id: String,
    /// The to-do that blocks it.
    pub blocker_todo_id: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// An idempotent mutation receipt (ADR-028 §3). Metadata only; never a title,
/// body, OCR, or source subject identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTransitionReceipt {
    /// `tmut`-prefixed ULID.
    pub id: String,
    /// The entity the receipt belongs to.
    pub entity_kind: TaskEntityKind,
    /// The entity id (for a blocker edge, the blocked to-do id).
    pub entity_id: String,
    /// The opaque idempotency key.
    pub idempotency_key: String,
    /// Canonical request hash for replay/mismatch detection.
    pub request_hash: String,
    /// State before the transition (SQL string form).
    pub from_state: String,
    /// State after the transition (SQL string form).
    pub to_state: String,
    /// Resulting entity revision.
    pub resulting_revision: i64,
    /// Optional resulting entity id (e.g. the new to-do on confirmation).
    pub resulting_entity_id: Option<String>,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
}

/// Compute the globally-unique dedupe key for a candidate (ADR-028 §5).
///
/// Canonical encoding is length-prefixed UTF-8 with an explicit marker for a
/// missing optional value, so distinct field sets can never collide. For
/// anchor-less source kinds the caller must already have folded a per-capture
/// occurrence discriminator into `dedupe_namespace` (Amendment B2); this function
/// is a pure hash of whatever provenance it is given.
pub fn compute_dedupe_key(source: &TaskSourceRef) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"task-candidate/v1\0");
    write_field(&mut hasher, Some(&source.dedupe_namespace));
    write_field(&mut hasher, Some(&source.source_kind.as_sql_str()));
    write_field(&mut hasher, source.extension_id.as_deref());
    write_field(&mut hasher, source.install_id.as_deref());
    write_field(&mut hasher, source.account_subject_ref.as_deref());
    write_field(&mut hasher, source.upstream_object_id.as_deref());
    write_field(&mut hasher, source.upstream_revision.as_deref());
    write_field(&mut hasher, Some(&source.content_hash));
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// Length-prefixed canonical field encoding. A present value is encoded as
/// `b'P'` + 8-byte big-endian length + raw UTF-8 bytes; a missing value is the
/// single marker byte `b'N'`, which no present value can produce.
fn write_field(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(v) => {
            hasher.update(*b"P");
            hasher.update((v.len() as u64).to_be_bytes());
            hasher.update(v.as_bytes());
        }
        None => {
            hasher.update(*b"N");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_source(kind: SourceKind) -> TaskSourceRef {
        TaskSourceRef {
            source_kind: kind,
            extension_id: None,
            install_id: None,
            account_subject_ref: None,
            upstream_object_id: None,
            upstream_revision: None,
            upstream_etag: None,
            occurred_at: None,
            observed_at: Utc::now(),
            dedupe_namespace: "ns-1".to_string(),
            content_hash: "sha256:abc".to_string(),
            lifecycle: SourceLifecycle::Active,
            source_outcome: None,
        }
    }

    #[test]
    fn candidate_state_sql_roundtrip() {
        for state in [
            CandidateState::Proposed,
            CandidateState::Confirmed,
            CandidateState::Dismissed,
            CandidateState::Expired,
        ] {
            assert_eq!(
                CandidateState::from_sql_str(state.as_sql_str()),
                Some(state)
            );
        }
        assert_eq!(CandidateState::from_sql_str("BOGUS"), None);
    }

    #[test]
    fn candidate_only_proposed_is_non_terminal() {
        assert!(!CandidateState::Proposed.is_terminal());
        assert!(CandidateState::Confirmed.is_terminal());
        assert!(CandidateState::Dismissed.is_terminal());
        assert!(CandidateState::Expired.is_terminal());
    }

    #[test]
    fn todo_transition_matrix_matches_adr() {
        use TodoState::*;
        // confirmed -> {in_progress, waiting, done, cancelled}
        assert!(Confirmed.can_transition_to(InProgress));
        assert!(Confirmed.can_transition_to(Waiting));
        assert!(Confirmed.can_transition_to(Done));
        assert!(Confirmed.can_transition_to(Cancelled));
        assert!(!Confirmed.can_transition_to(Confirmed)); // self-transition forbidden
                                                          // in_progress -> {waiting, done, cancelled}, not back to confirmed
        assert!(InProgress.can_transition_to(Waiting));
        assert!(InProgress.can_transition_to(Done));
        assert!(InProgress.can_transition_to(Cancelled));
        assert!(!InProgress.can_transition_to(Confirmed));
        // waiting <-> in_progress toggle allowed
        assert!(Waiting.can_transition_to(InProgress));
        assert!(Waiting.can_transition_to(Done));
        assert!(Waiting.can_transition_to(Cancelled));
        assert!(!Waiting.can_transition_to(Confirmed));
        // terminal states are frozen
        for term in [Done, Cancelled] {
            for target in [Confirmed, InProgress, Waiting, Done, Cancelled] {
                assert!(!term.can_transition_to(target), "{term:?} must be terminal");
            }
        }
    }

    #[test]
    fn source_kind_unknown_roundtrips_and_is_failclosed() {
        let unknown = SourceKind::from_sql_str("FUTURE_KIND");
        assert_eq!(unknown, SourceKind::Other("FUTURE_KIND".to_string()));
        assert!(!unknown.is_known());
        assert_eq!(unknown.as_sql_str(), "FUTURE_KIND");
        assert!(SourceKind::LocalCurrentScene.is_known());
        assert!(SourceKind::LocalCurrentScene.is_anchorless());
        assert!(!SourceKind::Interruption.is_anchorless());
    }

    #[test]
    fn source_lifecycle_unknown_roundtrips() {
        let lc = SourceLifecycle::from_sql_str("FUTURE_STATE");
        assert_eq!(lc, SourceLifecycle::Other("FUTURE_STATE".to_string()));
        assert!(!lc.is_active());
        assert!(SourceLifecycle::Active.is_active());
    }

    #[test]
    fn dedupe_key_is_deterministic_and_field_sensitive() {
        let a = sample_source(SourceKind::LocalCurrentScene);
        let key1 = compute_dedupe_key(&a);
        let key2 = compute_dedupe_key(&a);
        assert_eq!(key1, key2, "same provenance must be stable");
        assert!(key1.starts_with("sha256:"));

        // A different namespace (e.g. a distinct capture occurrence) diverges.
        let mut b = a.clone();
        b.dedupe_namespace = "ns-2".to_string();
        assert_ne!(compute_dedupe_key(&b), key1);

        // A changed content hash diverges.
        let mut c = a.clone();
        c.content_hash = "sha256:def".to_string();
        assert_ne!(compute_dedupe_key(&c), key1);
    }

    #[test]
    fn dedupe_key_no_collision_between_present_empty_and_absent() {
        // A present empty optional must not hash the same as an absent one,
        // thanks to the P/N marker bytes.
        let mut with_empty = sample_source(SourceKind::ExtensionContext);
        with_empty.extension_id = Some(String::new());
        let mut without = sample_source(SourceKind::ExtensionContext);
        without.extension_id = None;
        assert_ne!(
            compute_dedupe_key(&with_empty),
            compute_dedupe_key(&without),
            "present-empty and absent must differ"
        );
    }

    #[test]
    fn task_outcome_serde_tags_variants() {
        let json = serde_json::to_string(&TaskOutcome::RevisionConflict).unwrap();
        assert_eq!(json, r#"{"outcome":"revision_conflict"}"#);
        let confirmed = TaskOutcome::Confirmed {
            candidate_id: "tcand_1".to_string(),
            todo_id: "todo_1".to_string(),
            revision: 2,
        };
        let round: TaskOutcome =
            serde_json::from_str(&serde_json::to_string(&confirmed).unwrap()).unwrap();
        assert_eq!(round, confirmed);
    }

    #[test]
    fn entity_kind_and_outcome_roundtrip() {
        for kind in [
            TaskEntityKind::Candidate,
            TaskEntityKind::Todo,
            TaskEntityKind::TodoBlocker,
        ] {
            assert_eq!(TaskEntityKind::from_sql_str(kind.as_sql_str()), Some(kind));
        }
        for outcome in [
            SourceOutcome::Pending,
            SourceOutcome::Resumed,
            SourceOutcome::Abandoned,
            SourceOutcome::Expired,
        ] {
            assert_eq!(
                SourceOutcome::from_sql_str(outcome.as_sql_str()),
                Some(outcome)
            );
        }
    }
}
