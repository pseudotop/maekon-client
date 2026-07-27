//! Work-context acquisition and persistence ports (ADR-030 §9, #8587/#8589).
//!
//! ADR-030 §9 requires **separating acquisition from persistence**.
//! A connector only pulls bounded pages; it never writes to the store directly,
//! and atomically committing a page is the application use case's job. If this
//! separation is broken, a crash-before-commit leaves a window where only the
//! cursor advances.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::work_context::{
    ContextSourceDescriptor, ContextSourcePage, Lifecycle, MergeOutcome, WorkContextEnvelope,
};
use crate::models::work_context_projection::{ProjectionContent, TimelineEvidenceItem};

/// Connector health status — a typed result exposed directly to the user.
///
/// No raw provider error body, token, or URL secret is carried in any variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceHealth {
    Healthy,
    /// Authentication expired/revoked — re-authentication is required.
    Unauthorized,
    /// Insufficient permission — the scope is inadequate.
    Forbidden,
    /// Rate limited. `retry_after_secs` is filled only when the provider reports it.
    RateLimited {
        retry_after_secs: Option<u64>,
    },
    /// Transient failure such as a provider 5xx.
    ProviderUnavailable,
    /// The cursor has expired and a full resync is required.
    CursorExpired,
    /// The page could not be parsed.
    MalformedPage,
    /// Offline / network unreachable.
    Offline,
}

impl SourceHealth {
    /// A stable code string to show to the user.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::RateLimited { .. } => "rate_limited",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::CursorExpired => "cursor_expired",
            Self::MalformedPage => "malformed_page",
            Self::Offline => "offline",
        }
    }

    /// Whether the next sync may be attempted.
    ///
    /// Authentication failure and insufficient permission are pointless to retry
    /// until the user intervenes — this prevents a retry loop from exhausting the
    /// provider's rate limit.
    pub fn should_retry(&self) -> bool {
        !matches!(self, Self::Unauthorized | Self::Forbidden)
    }

    /// Whether the status requires user action (surfaces remediation in the console).
    pub fn needs_user_action(&self) -> bool {
        matches!(self, Self::Unauthorized | Self::Forbidden)
    }
}

/// A sync request for a single account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRequest {
    pub install_id: String,
    pub account_subject_ref: String,
    /// The cursor saved by the previous commit. `None` on first acquisition.
    pub cursor: Option<String>,
    /// The access epoch this request belongs to. Pass through the value issued by the store.
    pub access_epoch_id: i64,
    /// The maximum number of records this call may fetch.
    pub max_records: u32,
}

/// A sync result — either a page or a typed health status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    Page(ContextSourcePage),
    Unhealthy(SourceHealth),
}

/// Account authentication status (aligned with the ADR-031 account axis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    NotConnected,
    Connected,
    Revoked,
    Error,
}

/// Read-only context source connector (ADR-030 §9).
///
/// **Read-only.** This port has no write, action, or GUI-automation verbs, and
/// an implementing crate must not depend on a write transport (#8587 dependency gate).
#[async_trait]
pub trait ContextSourcePort: Send + Sync {
    /// Lists the source descriptors this connector provides.
    async fn discover(&self) -> Result<Vec<ContextSourceDescriptor>, CoreError>;

    /// The current connection status of a single account.
    async fn account_status(
        &self,
        install_id: &str,
        account_subject_ref: &str,
    ) -> Result<AccountStatus, CoreError>;

    /// Pulls one bounded page starting from the cursor.
    ///
    /// Implementations never exceed `max_records`. Unbounded historical backfill is
    /// out-of-scope for #8587.
    async fn sync(&self, request: SyncRequest) -> Result<SyncOutcome, CoreError>;

    /// Queries the connector health status.
    async fn health(&self, install_id: &str) -> Result<SourceHealth, CoreError>;

    /// Destroys local credentials and blocks subsequent syncs.
    async fn revoke(&self, install_id: &str, account_subject_ref: &str) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------
// Persistence ports
// ---------------------------------------------------------------------------

/// Cursor advance request. This is a compare-and-swap (ADR-030 revision I4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorAdvance {
    pub install_id: String,
    pub account_subject_ref: String,
    /// The cursor value read when this page began. If it differs from the current stored value, nothing is committed.
    pub expected_cursor: Option<String>,
    pub next_cursor: Option<String>,
}

/// Raw payload that is persisted to the raw plane only when explicit consent is present (ADR-030 §7, revision I1).
///
/// **In-memory only by default.** When `consent_present` is false, the adapter
/// does not write a raw row — no plaintext remains in the ledger. Even when true,
/// it is AEAD-encrypted and retained under a bounded TTL
/// (`clamp_raw_ttl_secs`, default 24 hours, hard maximum 7 days).
///
/// `plaintext` is sensitive source content, so `Debug` is implemented manually to
/// redact its contents — it does not leak into log or panic messages.
#[derive(Clone)]
pub struct RawPayloadInput {
    /// Raw bytes before refinement. Never persisted when consent is absent.
    pub plaintext: Vec<u8>,
    /// Whether explicit data-class consent is present. false = no raw row written (§7 default).
    pub consent_present: bool,
    /// Requested TTL (seconds). `clamp_raw_ttl_secs` narrows it to the 7-day hard maximum.
    pub requested_ttl_secs: Option<i64>,
}

impl std::fmt::Debug for RawPayloadInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawPayloadInput")
            .field(
                "plaintext",
                &format_args!("[redacted {} bytes]", self.plaintext.len()),
            )
            .field("consent_present", &self.consent_present)
            .field("requested_ttl_secs", &self.requested_ttl_secs)
            .finish()
    }
}

/// Carries a source object's projection/raw content in the commit, paired with an envelope (#8589).
///
/// Per the §2 principle, the envelope **carries no content** — the refined
/// projection and the plaintext are separated into this side structure and, only
/// for objects judged `Accepted`, written to the projection/raw plane. Matched to
/// the envelope by `source_object_key`.
#[derive(Debug, Clone)]
pub struct CommitContent {
    pub source_object_key: String,
    /// Refined title/summary. When `None`, no projection is written.
    pub projection: Option<ProjectionContent>,
    /// Plaintext payload. When `None` or consent is absent, no raw is written (in-memory only).
    pub raw_payload: Option<RawPayloadInput>,
}

/// Page commit request (performs the 7 steps of ADR-030 §9 in one transaction).
#[derive(Debug, Clone)]
pub struct CommitPageRequest {
    pub install_id: String,
    pub account_subject_ref: String,
    pub access_epoch_id: i64,
    pub ingest_run_id: String,
    pub envelopes: Vec<WorkContextEnvelope>,
    /// #8589: per-envelope projection/raw content (§9 step 4). When empty, this is a
    /// pure envelope commit (the #8587 path) — populated only when the connector
    /// supplies refined content (#8590).
    pub contents: Vec<CommitContent>,
    pub cursor: CursorAdvance,
    pub now: DateTime<Utc>,
}

/// Page commit result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// Commit succeeded. Returns the per-record merge outcomes in order.
    Committed { results: Vec<MergeOutcome> },
    /// Cursor CAS failed — nothing was committed and the cursor did not advance.
    ///
    /// Prevents two overlapping acquisitions from overwriting or rewinding each
    /// other's cursor. The caller discards the page and re-reads the current cursor
    /// on the next run.
    CursorConflict,
    /// The page's epoch differs from the account's current epoch — discard it.
    EpochMismatch { current_epoch: i64 },
}

/// Stored cursor state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub install_id: String,
    pub account_subject_ref: String,
    pub cursor: Option<String>,
    pub access_epoch_id: i64,
    pub last_ingested_at: Option<DateTime<Utc>>,
}

/// Work-context persistence port (ADR-030 §9).
#[async_trait]
pub trait WorkContextStorePort: Send + Sync {
    /// Issues a new access epoch (ADR-030 revision I2).
    ///
    /// The epoch is **owned and issued by the store** — not by the capability broker
    /// nor the connector. The broker only signals revocation/re-authorization; it does
    /// not supply the epoch value. ADR-031's broker cancellation epoch is separate and
    /// has no ordering relationship with this counter.
    async fn begin_access_epoch(
        &self,
        install_id: &str,
        account_subject_ref: &str,
        now: DateTime<Utc>,
    ) -> Result<i64, CoreError>;

    /// Reads the account's current cursor state.
    async fn get_cursor(
        &self,
        install_id: &str,
        account_subject_ref: &str,
    ) -> Result<Option<CursorState>, CoreError>;

    /// Commits a page atomically.
    ///
    /// A crash before commit does not advance the cursor. If the commit succeeds but
    /// the response is lost, the page is replayed and the local uniqueness key returns
    /// the original result verbatim (ADR-030 §9).
    async fn commit_page(&self, request: CommitPageRequest) -> Result<CommitOutcome, CoreError>;

    /// Queries the current envelope of a single source object.
    async fn get_envelope(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
    ) -> Result<Option<WorkContextEnvelope>, CoreError>;

    /// List of projectable envelopes (unknown kinds and terminal lifecycles are excluded).
    async fn list_projectable(&self, limit: u32) -> Result<Vec<WorkContextEnvelope>, CoreError>;

    /// Queries external work-context timeline items (ADR-030 §11, #8589).
    ///
    /// Returns only refined projections joined to active, unexpired, non-isolated
    /// envelopes, labeled `source_family = work_context`. **Read-only, and it does
    /// not trust the stored snapshot** — the caller (the application) must re-evaluate
    /// live consent/access before displaying (§8). Composition with PC events is
    /// merged by the caller using the family label.
    async fn list_work_context_timeline(
        &self,
        limit: u32,
    ) -> Result<Vec<TimelineEvidenceItem>, CoreError>;

    /// Dereferences a source object's live refined projection (ADR-030 §8/§11, #8589).
    ///
    /// Used when the suggestion/memory pipeline fetches a refined projection from an
    /// evidence reference. Returns only projections of active, unexpired, non-isolated
    /// envelopes — terminal/expired/isolated ones return `None`. Because the stored
    /// consent snapshot does not authorize this read, the caller must re-verify live
    /// consent/access/data-class on every call before invoking (§8).
    async fn read_projection(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
    ) -> Result<Option<ProjectionContent>, CoreError>;

    /// Transitions the lifecycle and leaves the necessary tombstone.
    async fn mark_lifecycle(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
        lifecycle: Lifecycle,
        now: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    /// Stops acquisition for a single account and erases its content (the revocation path).
    ///
    /// Content-free suppression tombstones remain for the replay horizon — because on
    /// reconnection, stale re-sent pages must continue to be suppressed (ADR-030 §12).
    async fn revoke_account(
        &self,
        install_id: &str,
        account_subject_ref: &str,
        now: DateTime<Utc>,
    ) -> Result<(), CoreError>;

    /// Cleans up expired planes.
    ///
    /// Expiry is judged with `effective_now = max(current_utc, last_ingested_at)`, so
    /// a clock rewind cannot extend expiry (ADR-030 revision I5).
    async fn expire_planes(&self, now: DateTime<Utc>) -> Result<u64, CoreError>;
}
