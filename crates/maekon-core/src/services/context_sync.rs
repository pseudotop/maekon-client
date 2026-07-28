//! Context source sync orchestrator (ADR-030 §9, #8587).
//!
//! A pure use case that pulls bounded pages via `ContextSourcePort` and commits
//! them atomically via `WorkContextStorePort`. It composes only ports without
//! depending on network or storage implementations, so it structurally satisfies
//! the #8587 dependency gate that a read-only crate must not depend on a write
//! transport.
//!
//! Cancellation is handled cooperatively via `CancelFlag` (Arc<AtomicBool>). When
//! a runtime revoke/disable/uninstall sets the flag, it stops at the next page
//! boundary.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::ports::work_context::{
    CommitOutcome, CommitPageRequest, ContextSourcePort, CursorAdvance, SourceHealth, SyncOutcome,
    SyncRequest, WorkContextStorePort,
};

/// Cooperative cancellation flag.
///
/// When the broker sets it on revoke/disable/uninstall, the orchestrator safely
/// stops at the next page boundary. An in-flight page is already an atomic commit,
/// so it leaves no partial state.
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Result summary of one sync run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSummary {
    /// Number of committed pages.
    pub pages_committed: u32,
    /// Number of processed records (including duplicates).
    pub records_seen: u32,
    /// Reason the run stopped.
    pub stop_reason: StopReason,
}

/// Why the sync loop stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The provider reported no more pages (has_more=false).
    Drained,
    /// Reached the page budget (max_pages) — prevents unbounded collection.
    PageBudgetReached,
    /// The cancellation flag was set (revoke/disable/uninstall).
    Cancelled,
    /// The connector cannot be advertised as supported (ADR-030 §5, revision I6). A
    /// connector that cannot deterministically handle deletion/access loss (e.g.
    /// content_hash_only with no explicit delete signal) is not collected at all —
    /// fundamentally preventing an absent listing from being mistaken for a deletion.
    NotAdvertisable,
    /// The connector reported an unhealthy state — surfaced as typed health.
    Unhealthy(SourceHealth),
    /// Cursor CAS failed — an overlapping collection was detected, so this run yields.
    CursorConflict,
    /// The page epoch is stale — re-authorization is required.
    EpochChanged { current_epoch: i64 },
}

/// Sync parameters for a single account.
pub struct SyncPlan {
    pub install_id: String,
    pub account_subject_ref: String,
    pub access_epoch_id: i64,
    pub ingest_run_id: String,
    /// Maximum number of records a single page can hold (quota).
    pub max_records: u32,
    /// Maximum number of pages to process in one run (prevents unbounded collection).
    pub max_pages: u32,
    /// HMAC dedupe key — injected from wiring as a Keychain secret (revision B2).
    /// The orchestrator does not store the key and uses it only for identity mapping.
    pub dedupe_key: Vec<u8>,
    /// The revision model declared by this connector (descriptor §5).
    pub revision_model: crate::models::work_context::RevisionModel,
}

/// Runs the pagination loop, collecting until the cursor is drained or the
/// budget/cancellation/unhealthy condition is reached.
///
/// Each page is committed atomically (ADR-030 §9). If a crash occurs before the
/// commit, the cursor does not advance, so the next run resumes from the same
/// cursor and the local uniqueness key prevents duplicate envelopes.
pub async fn run_sync(
    source: &dyn ContextSourcePort,
    store: &dyn WorkContextStorePort,
    plan: &SyncPlan,
    cancel: &CancelFlag,
    now: DateTime<Utc>,
) -> Result<SyncSummary, CoreError> {
    let mut pages_committed = 0u32;
    let mut records_seen = 0u32;

    // Cancellation comes before the network — if the flag is already set, we do not even discover.
    if cancel.is_cancelled() {
        return Ok(SyncSummary {
            pages_committed,
            records_seen,
            stop_reason: StopReason::Cancelled,
        });
    }

    // I6/§5 advertisability gate: a connector that cannot deterministically handle
    // deletion/access loss is refused for collection (e.g. content_hash_only with no
    // explicit delete signal). Without this check, `is_advertisable` remains a dead
    // pure function that nobody calls, and a connector that mistakes an absent listing
    // for a deletion gets collected anyway. fail-closed: refuse if there is no
    // descriptor for this install at all, or if there is even one non-advertisable descriptor.
    let descriptors = source.discover().await?;
    let relevant: Vec<_> = descriptors
        .iter()
        .filter(|d| d.install_id == plan.install_id)
        .collect();
    if relevant.is_empty() || !relevant.iter().all(|d| d.is_advertisable()) {
        return Ok(SyncSummary {
            pages_committed,
            records_seen,
            stop_reason: StopReason::NotAdvertisable,
        });
    }

    loop {
        // Check cancellation at the page boundary. Already-committed pages are intact.
        if cancel.is_cancelled() {
            return Ok(SyncSummary {
                pages_committed,
                records_seen,
                stop_reason: StopReason::Cancelled,
            });
        }
        if pages_committed >= plan.max_pages {
            return Ok(SyncSummary {
                pages_committed,
                records_seen,
                stop_reason: StopReason::PageBudgetReached,
            });
        }

        // Read the current cursor (restart recovery point).
        let cursor_state = store
            .get_cursor(&plan.install_id, &plan.account_subject_ref)
            .await?;
        let expected_cursor = cursor_state.as_ref().and_then(|c| c.cursor.clone());

        // Pull a bounded page.
        let outcome = source
            .sync(SyncRequest {
                install_id: plan.install_id.clone(),
                account_subject_ref: plan.account_subject_ref.clone(),
                cursor: expected_cursor.clone(),
                access_epoch_id: plan.access_epoch_id,
                max_records: plan.max_records,
            })
            .await?;

        let page = match outcome {
            SyncOutcome::Page(p) => p,
            // An unhealthy state is surfaced as typed health without the raw error body.
            SyncOutcome::Unhealthy(h) => {
                return Ok(SyncSummary {
                    pages_committed,
                    records_seen,
                    stop_reason: StopReason::Unhealthy(h),
                });
            }
        };

        // Do not trust a connector quota violation — a page exceeding the cap is refused.
        if page.records.len() as u32 > plan.max_records {
            return Ok(SyncSummary {
                pages_committed,
                records_seen,
                stop_reason: StopReason::Unhealthy(SourceHealth::MalformedPage),
            });
        }

        let batch_len = page.records.len() as u32;
        let has_more = page.has_more;
        let next_cursor = page.next_cursor.clone();

        // Map records to canonical envelopes (§9 step 2). The identity/revision
        // fingerprint is computed here, and the dedupe key is injected from the plan.
        let envelopes = page
            .records
            .iter()
            .map(|r| record_to_envelope(r, plan, now))
            .collect();

        let commit = store
            .commit_page(CommitPageRequest {
                install_id: plan.install_id.clone(),
                account_subject_ref: plan.account_subject_ref.clone(),
                access_epoch_id: plan.access_epoch_id,
                ingest_run_id: plan.ingest_run_id.clone(),
                envelopes,
                // #8589: the orchestrator does not synthesize the sanitized projection/raw content —
                // that is content the connector (#8590) supplies together with its sanitization/consent decision.
                // With no connector yet, this is a pure envelope commit, and the store's projection/
                // raw writer only runs when `contents` is populated (§9 step 4).
                contents: Vec::new(),
                cursor: CursorAdvance {
                    install_id: plan.install_id.clone(),
                    account_subject_ref: plan.account_subject_ref.clone(),
                    expected_cursor,
                    next_cursor,
                },
                now,
            })
            .await?;

        match commit {
            CommitOutcome::Committed { .. } => {
                pages_committed += 1;
                records_seen += batch_len;
            }
            // An overlapping collection was detected — this run yields and defers to the next scheduled run.
            CommitOutcome::CursorConflict => {
                return Ok(SyncSummary {
                    pages_committed,
                    records_seen,
                    stop_reason: StopReason::CursorConflict,
                });
            }
            CommitOutcome::EpochMismatch { current_epoch } => {
                return Ok(SyncSummary {
                    pages_committed,
                    records_seen,
                    stop_reason: StopReason::EpochChanged { current_epoch },
                });
            }
        }

        if !has_more {
            return Ok(SyncSummary {
                pages_committed,
                records_seen,
                stop_reason: StopReason::Drained,
            });
        }
    }
}

/// Maps a connector record to a canonical envelope (§9 step 2).
///
/// The HMAC identity key and revision fingerprint are computed here. The dedupe
/// key and revision model are injected from `plan`, so the orchestrator stores no
/// secret.
fn record_to_envelope(
    record: &crate::models::work_context::ContextSourceRecord,
    plan: &SyncPlan,
    now: DateTime<Utc>,
) -> crate::models::work_context::WorkContextEnvelope {
    use crate::models::work_context::{
        compute_revision_fingerprint, compute_source_object_key, WorkContextEnvelope,
    };

    let fingerprint = compute_revision_fingerprint(
        plan.revision_model,
        record.remote_revision.as_deref(),
        record.etag.as_deref(),
        record.source_updated_at,
        &record.content_hash,
        record.lifecycle,
    );
    let source_object_key = compute_source_object_key(&plan.dedupe_key, &record.identity);

    WorkContextEnvelope {
        // envelope_id is derived from the **HMAC source_object_key** — it does not
        // carry the raw provider remote_id (IMPORTANT 4). Because tombstone_id is
        // `{envelope_id}_tomb`, embedding remote_id here would make the content-free,
        // longest-lived tombstone PK — which survives even after uninstall — carry the
        // remote identifier, collapsing the HMAC keying rationale of §4/§7.
        // source_object_key is irreversible and distinguishes revisions by
        // (order, epoch), so it is deterministic yet safe (replay is idempotent via the UNIQUE key).
        envelope_id: format!(
            "wctx_{}_{}_{}",
            source_object_key,
            record.source_order.unwrap_or(0),
            plan.access_epoch_id
        ),
        schema_version: crate::models::work_context::WORK_CONTEXT_SCHEMA_VERSION,
        access_epoch_id: plan.access_epoch_id,
        source_object_key,
        identity: record.identity.clone(),
        revision_model: plan.revision_model,
        remote_revision: record.remote_revision.clone(),
        etag: record.etag.clone(),
        source_order: record.source_order,
        content_hash: record.content_hash.clone(),
        revision_fingerprint: fingerprint,
        kind: record.kind,
        classification: record.classification,
        retention_class: None,
        occurred_at: record.occurred_at,
        source_updated_at: record.source_updated_at,
        observed_at: record.observed_at,
        ingested_at: now,
        relations: record.relations.clone(),
        access_snapshot: None,
        consent_snapshot: None,
        ingest_run_id: plan.ingest_run_id.clone(),
        prior_envelope_id: None,
        source_cursor_digest: None,
        projection_ref: None,
        raw_blob_ref: None,
        lifecycle: record.lifecycle,
    }
}
