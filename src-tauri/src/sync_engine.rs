//! SyncEngine -- orchestrates the pull/merge/push sync cycle.
//!
//! This is a wiring-level component (no SQL, no transport logic).
//! It coordinates ChangeExtractor, ChangeMerger, and SyncTransport
//! through the port traits defined in maekon-core.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

use chrono::{DateTime, Utc};

use maekon_core::error::CoreError;
use maekon_core::models::storage_records::EgressLedgerRecord;
use maekon_core::models::sync::{ChangeSet, ChangeSetKind, SyncResult};
use maekon_core::ports::change_extractor::ChangeExtractor;
use maekon_core::ports::change_merger::ChangeMerger;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::egress_ledger::EgressLedgerSink;
use maekon_core::ports::erasure_propagation_store::ErasurePropagationStore;
use maekon_core::ports::sync_transport::SyncTransport;
use maekon_core::sync::Hlc;

/// Coarse erasure id for the web "Delete all data" path — it carries no per-erasure
/// id and is not restart-durable, so its DeletionEvent uses an in-memory one-shot
/// keyed on this constant (per-erasure durability for the web path is #4478).
const WEB_ERASURE_ID: &str = "web-erasure";

/// #5165: exponential-backoff state for retrying an undeliverable DeletionEvent, so a
/// pending erasure with no reachable peer does not push every sync tick indefinitely.
#[derive(Default)]
struct ErasureBackoff {
    /// The erasure id the backoff applies to (a new erasure id resets it).
    id: Option<String>,
    failures: u32,
    /// Earliest instant the next attempt is allowed; `None` = no backoff.
    retry_after: Option<tokio::time::Instant>,
}

#[allow(dead_code)] // fields stored for cross-device sync lifecycle
pub struct SyncEngine {
    extractor: Arc<dyn ChangeExtractor>,
    merger: Arc<dyn ChangeMerger>,
    transport: Arc<dyn SyncTransport>,
    /// Shared ConsentManager from the runtime. Read-only access via `Arc`.
    /// Callers must pass the same instance used elsewhere in the runtime
    /// rather than letting SyncEngine construct its own instance.
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// #5143: optional egress audit sink. When present, every successful push
    /// (a normal changeset OR the GDPR `DeletionEvent`) records an `uploaded`
    /// row in the #4803 egress ledger. `None` (tests / no ledger) = no
    /// recording; an injected sink failure is logged, never fatal.
    egress_sink: Option<Arc<dyn EgressLedgerSink>>,
    /// Egress destination label for the ledger (`sync.lan` / `sync.remote` /
    /// `sync.file`), set by the runtime via [`SyncEngine::with_egress`].
    egress_destination: String,
    device_id: String,
    device_name: String,
    /// High-watermark HLC from the last successful push. Only rows with
    /// HLC > this value will be extracted on the next push cycle, avoiding
    /// re-extraction of all local data every cycle.
    last_push_watermark: parking_lot::Mutex<Hlc>,
    /// #5156: web "Delete all data" one-shot dedup (in-memory, per process). The
    /// web `erasure_requested` flag carries no per-erasure id and is not
    /// restart-durable (#4478), so its DeletionEvent uses this simple latch. The
    /// CONSENT-revoke path uses the PERSISTED fire-once gate below instead.
    deletion_pushed: AtomicBool,
    /// #5156: durable store for the last propagated consent-revoke erasure id — the
    /// persisted fire-once gate. The sender propagates each distinct erasure exactly
    /// once (keyed on `ConsentManagerPort::pending_erasure_id`) and never re-announces
    /// it, even across restart. Since #5181 the device-wide delete is also BOUNDED by
    /// the erasure HLC anchor (post-re-grant data, HLC > anchor, is spared), so a
    /// re-announce is now doubly safe; the gate remains as defense-in-depth and still
    /// guards the legacy/`None`-anchor unbounded fallback. `None` = no store wired
    /// (tests / sync off).
    erasure_store: Option<Arc<dyn ErasurePropagationStore>>,
    /// In-memory mirror of `erasure_store`, seeded once at `with_erasure_store` so the
    /// gate avoids a SQLite read every cycle. Updated on each successful erasure push.
    last_pushed_erasure_id: parking_lot::Mutex<Option<String>>,
    /// #5165: retry backoff for an undeliverable DeletionEvent (no peers / transport
    /// error) so it does not push every tick forever; reset on a new id or delivery.
    erasure_backoff: parking_lot::Mutex<ErasureBackoff>,
    /// One-shot signal set by the web "Delete all data" (GDPR right-to-erasure)
    /// endpoint so a LOCAL erasure also propagates a device-wide `DeletionEvent`
    /// to LAN peers — closing the re-hydration gap (#4478 G3). Independent of
    /// consent revocation (the user may keep using the app + syncing new data);
    /// fires once, deduped by `deletion_pushed`.
    erasure_requested: Option<Arc<AtomicBool>>,
    /// Timestamp of the last successful sync cycle completion.
    last_sync_at: parking_lot::Mutex<Option<DateTime<Utc>>>,
    /// Error message from the most recent failed sync cycle, if any.
    last_error: parking_lot::Mutex<Option<String>>,
}

impl SyncEngine {
    /// Create a new SyncEngine.
    ///
    /// `consent_manager` should be the application-wide `ConsentManager`
    /// (the same `Arc<dyn ConsentManagerPort>` used by the scheduler and other
    /// components). When `None`, consent checks are skipped (sync always
    /// runs). Do **not** construct a separate `ConsentManager` from the
    /// file path — that creates divergent in-memory state.
    pub async fn new(
        extractor: Arc<dyn ChangeExtractor>,
        merger: Arc<dyn ChangeMerger>,
        transport: Arc<dyn SyncTransport>,
        consent_manager: Option<Arc<dyn ConsentManagerPort>>,
        erasure_requested: Option<Arc<AtomicBool>>,
        device_id: String,
        device_name: String,
    ) -> Self {
        // Seed the push watermark from storage so we never re-push rows that
        // were already successfully pushed in a previous process lifetime.
        let initial_watermark = match extractor.local_watermark().await {
            Ok(wm) => {
                if wm != Hlc::default() {
                    debug!(
                        wall_ms = wm.wall_ms,
                        counter = wm.counter,
                        "initialized push watermark from storage"
                    );
                }
                wm
            }
            Err(e) => {
                warn!("failed to read initial push watermark, starting from zero: {e}");
                Hlc::default()
            }
        };

        Self {
            extractor,
            merger,
            transport,
            consent_manager,
            // Egress auditing is opt-in via `with_egress`; absent by default so
            // the constructor signature (and its many test call sites) is unchanged.
            egress_sink: None,
            egress_destination: String::new(),
            erasure_requested,
            device_id,
            device_name,
            last_push_watermark: parking_lot::Mutex::new(initial_watermark),
            deletion_pushed: AtomicBool::new(false),
            erasure_store: None,
            last_pushed_erasure_id: parking_lot::Mutex::new(None),
            erasure_backoff: parking_lot::Mutex::new(ErasureBackoff::default()),
            last_sync_at: parking_lot::Mutex::new(None),
            last_error: parking_lot::Mutex::new(None),
        }
    }

    /// Attach the #4803 egress audit sink (compliance: record what leaves the
    /// device via sync). `destination` is the ledger label for this transport
    /// (`sync.lan` / `sync.remote` / `sync.file`). Builder so the constructor —
    /// and its test call sites — stay unchanged (#5143).
    pub fn with_egress(mut self, sink: Arc<dyn EgressLedgerSink>, destination: String) -> Self {
        self.egress_sink = Some(sink);
        self.egress_destination = destination;
        self
    }

    /// #5147: test-only accessor so a regression test can assert that the production
    /// `build_sync_engine` actually wired the egress ledger sink. The `.with_egress`
    /// builder is opt-in (absent = silent no-recording), so a future construction path
    /// that drops the chain would compile and silently skip the compliance egress audit.
    #[cfg(test)]
    pub(crate) fn has_egress_sink(&self) -> bool {
        self.egress_sink.is_some()
    }

    /// Attach the durable erasure-propagation store (#5156). Seeds the in-memory
    /// `last_pushed_erasure_id` mirror from it ONCE so a restart resumes the
    /// fire-once gate where it left off — the consent-revoke DeletionEvent is
    /// propagated exactly once per distinct erasure, never re-announced.
    pub fn with_erasure_store(mut self, store: Arc<dyn ErasurePropagationStore>) -> Self {
        *self.last_pushed_erasure_id.lock() = store.last_pushed_erasure_id();
        self.erasure_store = Some(store);
        self
    }

    /// Record one `uploaded` egress entry for a successful push. Best-effort:
    /// a ledger write failure is logged and swallowed so it never fails the
    /// sync push it audits (matches the telemetry egress path). No-op when no
    /// sink is attached.
    fn record_push_egress(
        &self,
        changeset: &ChangeSet,
        event_type: &str,
        dedup_key: &str,
        recipient_count: usize,
    ) {
        let Some(ref sink) = self.egress_sink else {
            return;
        };
        // PLAINTEXT serialized changeset size (information volume), deliberately
        // NOT the encrypted/compressed on-wire byte count (each transport
        // encrypts after this), and per single serialization. #5147 item 2: a LAN
        // fan-out sends this same serialization to `recipient_count` peers, so the
        // true aggregate egress volume is `byte_count * recipient_count` (recorded
        // as a separate field; byte_count keeps its single-serialization meaning).
        let byte_count = serde_json::to_vec(changeset)
            .map(|v| v.len() as i64)
            .unwrap_or(0);
        // Consent snapshot at egress time. The push path only runs once the
        // cross_device_sync permission is granted (Gate 1), so this is the
        // effective state that authorised the egress.
        let consent_state = self
            .consent_manager
            .as_ref()
            .map(|cm| {
                format!(
                    "cross_device_sync={}",
                    cm.effective_permissions().cross_device_sync
                )
            })
            .unwrap_or_else(|| "cross_device_sync=unmanaged".to_string());

        // #5147: DETERMINISTIC record_id so a crash/restart re-push of the SAME
        // logical egress dedups via the store's `INSERT OR IGNORE` instead of
        // writing a duplicate audit row (a fresh uuid per call never collides).
        // `dedup_key` is the changeset watermark for a normal CrossDeviceSync push
        // (stable per batch) and the PER-ERASURE id for a DeletionEvent (#5156), so
        // distinct erasures get distinct audit rows and a re-announce of the same
        // erasure dedups. (Was the device id — do NOT restore that; it collapsed
        // distinct erasures into one Art.17 audit row.)
        let record_id = format!(
            "egress|{}|{}|{}",
            self.egress_destination, event_type, dedup_key
        );
        // event_id correlates the row to the changeset's HLC watermark — the key
        // tying a ledger row to the exact data boundary that egressed.
        let event_id = serde_json::to_string(&changeset.watermark).ok();
        let record = EgressLedgerRecord {
            record_id,
            event_type: event_type.to_string(),
            event_id,
            byte_count,
            recipient_count: recipient_count.max(1) as i64,
            destination: self.egress_destination.clone(),
            disposition: "uploaded".to_string(),
            consent_state,
            occurred_at: Utc::now().to_rfc3339(),
        };
        if let Err(e) = sink.record_egress(&record) {
            warn!(err.code = %e.code(), "sync egress ledger write failed: {e}");
        }
    }

    /// Run one complete sync cycle: check consent, handle deletion,
    /// pull + merge, extract + push.
    ///
    /// On success, updates `last_sync_at` and clears `last_error`.
    /// On failure, records the error message in `last_error`.
    pub async fn run_cycle(&self) -> Result<Option<SyncResult>, CoreError> {
        match self.run_cycle_inner().await {
            Ok(result) => {
                *self.last_sync_at.lock() = Some(Utc::now());
                *self.last_error.lock() = None;
                Ok(result)
            }
            Err(e) => {
                *self.last_error.lock() = Some(e.to_string());
                Err(e)
            }
        }
    }

    /// Inner implementation of the sync cycle, called by `run_cycle` which
    /// wraps it with health tracking.
    async fn run_cycle_inner(&self) -> Result<Option<SyncResult>, CoreError> {
        // GDPR Art. 17 erasure propagation runs FIRST and BYPASSES the consent gate
        // (#5165): a pending erasure must reach peers even after the user revoked
        // cross_device_sync — a tombstone is an erasure, not data collection. (The
        // scheduler likewise calls `propagate_pending_erasure` directly when the
        // capture gate is closed, so the erasure is not blocked by capture scheduling.)
        if self.propagate_pending_erasure().await? {
            return Ok(None); // a deletion cycle does no normal data sync
        }

        // Gate 1: consent check (NORMAL DATA SYNC only — the erasure above already ran).
        // #5143 note: a consent-denied cycle records NO egress-ledger row (unlike the
        // telemetry path's `blocked` disposition). The gate short-circuits before any
        // changeset is extracted, so there is no concrete staged egress to mark
        // `blocked`. "Nothing left → nothing audited" — deliberate, tested by
        // `consent_blocked_cycle_records_no_egress`.
        // #5147 item 5 (re-evaluated): keep this — recording a per-cycle `blocked` row
        // would require extracting a changeset inside a consent-denied cycle purely to
        // compute a byte_count, which the egress ledger (what physically LEFT the device)
        // does not need. The egress semantic differs from telemetry's per-event filter;
        // the decision to not emit a sync `blocked` row stands.
        if let Some(ref cm) = self.consent_manager {
            if !cm.effective_permissions().cross_device_sync {
                debug!("sync skipped: cross_device_sync consent not granted");
                return Ok(None);
            }
        }

        // --- Pull phase ---
        let local_watermark = self.extractor.local_watermark().await?;
        let mut merge_result: Option<SyncResult> = None;

        // Pull changesets in a loop until no more are available
        loop {
            let watermark = merge_result
                .as_ref()
                .map(|r| &r.new_watermark)
                .unwrap_or(&local_watermark);

            match self.transport.pull(watermark).await? {
                None => break,
                Some(changeset) => {
                    info!(
                        origin = %changeset.origin_device_id,
                        rows = changeset.row_count(),
                        "pulled changeset from transport"
                    );
                    let result = self.merger.apply_changes(changeset).await?;
                    debug!(
                        applied = result.applied,
                        skipped_lww = result.skipped_lww,
                        skipped_dup = result.skipped_dup,
                        tombstoned = result.tombstoned,
                        "merge completed"
                    );
                    merge_result = Some(result);
                }
            }
        }

        // --- Push phase ---
        // Use the last successful push watermark so we only extract rows
        // that were created or modified since the previous push.
        let since = { self.last_push_watermark.lock().clone() };
        // #6247: PUSH uses the SELF-ORIGIN scope. The LAN `/sync/push` receiver (#5211)
        // rejects any data row whose origin is not the authenticated pusher, so we must
        // not re-send peer-origin rows we received via merge — doing so both fails the
        // push and risks cross-device echo loops. (Pull-serving keeps the all-origin
        // `get_changes_since` so a relay can forward another peer's rows.)
        let local_changes = self.extractor.get_local_changes_since(&since).await?;

        if !local_changes.is_empty() {
            info!(rows = local_changes.row_count(), "pushing local changes");
            let delivered = self.transport.push(&local_changes).await?;
            // #5143: audit the egress in the #4803 ledger ONLY when the data
            // actually reached a destination. A best-effort transport (LAN with
            // zero peers or all-peers-failed) returns 0 → nothing left the
            // device → no `uploaded` row (don't assert an egress that did not
            // happen in a legally-retained ledger). Best-effort: a ledger write
            // failure is logged inside the helper, never fails the sync.
            if delivered > 0 {
                // dedup_key = the changeset's watermark (the DB-global max HLC at
                // extraction time, sync_extractor::compute_max_hlc — it only rises
                // when new rows are extracted). So a re-push of the EXACT same
                // batch reuses the same key and dedups; this fires when the
                // in-memory `last_push_watermark` is lost AND `local_watermark()`
                // re-seeds `since` at/below the pushed boundary (notably the
                // read-failure fallback to `Hlc::default()` → since=0 → re-extract
                // all). An ordinary restart re-seeds `since` to the DB max and
                // re-extracts nothing, so no duplicate arises. On the (infallible)
                // serialize failure, fall back to a UNIQUE id rather than an empty
                // key — err toward recording the egress, never silently collapsing.
                let dedup_key = serde_json::to_string(&local_changes.watermark)
                    .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
                self.record_push_egress(&local_changes, "CrossDeviceSync", &dedup_key, delivered);
            }
            // Advance watermark only after a successful push so that a
            // transient transport failure causes a retry of the same rows.
            let new_watermark = local_changes.watermark.clone();
            *self.last_push_watermark.lock() = new_watermark;
        }

        Ok(merge_result)
    }

    /// Propagate a pending GDPR Art. 17 erasure to peers, BYPASSING the consent gate
    /// (Gate 1) — and, when called directly by the scheduler, the capture gate too. A
    /// tombstone is an erasure, not data collection, so it must reach peers even when
    /// the user has revoked cross_device_sync or sync/capture is gated off (#5165 —
    /// Reclaim transport-side storage (e.g. consumed changeset files in a shared
    /// sync folder) by delegating to the transport's `enforce_retention` (#6243).
    /// No-op for transports with no reclaimable local artifacts (remote/in-memory).
    /// Called periodically by the cross-device sync loop.
    pub async fn enforce_transport_retention(&self) -> Result<usize, CoreError> {
        self.transport.enforce_retention().await
    }

    /// "revoke-and-walk-away"). Returns `true` if a DeletionEvent was due (and pushed).
    ///
    /// Safe to call every tick (and inside `run_cycle`): the persisted fire-once gate
    /// makes each distinct erasure propagate exactly once, so repeat calls are no-ops.
    pub async fn propagate_pending_erasure(&self) -> Result<bool, CoreError> {
        if let Some((erasure_id, persist)) = self.deletion_to_propagate() {
            // #5165: if we're backing off this erasure after repeated non-delivery,
            // SKIP the push and report `false` — so `run_cycle` is NOT short-circuited
            // and normal data sync still runs for a still-consented device during the
            // backoff window. (The erasure retries once the window elapses.)
            if self.erasure_backoff_active(&erasure_id) {
                debug!(erasure_id = %erasure_id, "deletion event backing off; deferring this tick");
                return Ok(false);
            }
            self.push_deletion_event(&erasure_id, persist).await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Decide whether a DeletionEvent must be propagated this cycle, and how to
    /// dedup it. Returns `(erasure_id, persist)`, or `None` when nothing is due.
    /// `persist=true` = consent-revoke path (durable fire-once); `false` = web path
    /// (in-memory one-shot).
    fn deletion_to_propagate(&self) -> Option<(String, bool)> {
        // Consent-revoke path: persisted, fire-once per distinct erasure id.
        if let Some(id) = self
            .consent_manager
            .as_ref()
            .and_then(|cm| cm.pending_erasure_id())
        {
            if self.last_pushed_erasure_id.lock().as_deref() != Some(id.as_str()) {
                return Some((id, true));
            }
            // Pending-but-already-propagated: do NOT re-announce the consent erasure,
            // but FALL THROUGH to the web path (#5156 review) — a still-pending consent
            // erasure must not shadow a web "Delete all data". The consent tombstone
            // already ran an unbounded delete that cannot cover data collected AFTER
            // it (e.g. post-re-grant), so a concurrent web erase must still propagate.
        }
        // Web "Delete all data" path: in-memory one-shot (no per-erasure id; #4478).
        if !self.deletion_pushed.load(Ordering::Acquire)
            && self
                .erasure_requested
                .as_ref()
                .map(|f| f.load(Ordering::Acquire))
                .unwrap_or(false)
        {
            return Some((WEB_ERASURE_ID.to_string(), false));
        }
        None
    }

    /// Propagate a GDPR Article 17 DeletionEvent for erasure `id` and, on confirmed
    /// delivery, record it as propagated so it is never re-announced.
    ///
    /// `persist`: true for the consent-revoke path (DURABLE fire-once — the id is
    /// written to the erasure store so a restart does not re-announce); false for
    /// the web path (in-memory one-shot via `deletion_pushed`). On ZERO confirmed
    /// deliveries nothing is marked, so the erasure is retried next cycle/restart —
    /// the durability guarantee (offline-at-revoke still propagates eventually).
    async fn push_deletion_event(
        &self,
        id: &str,
        persist: bool,
    ) -> Result<Option<SyncResult>, CoreError> {
        // (#5165 backoff is checked by `propagate_pending_erasure` before this is
        // called, so a deferred erasure does not short-circuit normal sync.)
        info!(erasure_id = id, "pushing GDPR Article 17 deletion event");

        // #5181: bound the device-wide delete to data that existed at erasure time by
        // stamping the DeletionEvent watermark with the persisted erasure HLC anchor
        // (written by the S2 producer, retained across the wipe). A peer running #5181
        // spares any post-re-grant data (HLC > anchor); a pre-#5181 peer ignores the
        // watermark and still does the conservative unbounded delete (R3 compat). No
        // anchor (pre-#5179 install, or a test) → now() = effectively unbounded.
        let watermark = match self.extractor.persisted_erasure_hlc().await {
            Ok(Some(hlc)) => hlc,
            Ok(None) => Hlc::now(&self.device_id),
            Err(e) => {
                warn!(err.code = %e.code(), "failed to read erasure anchor; unbounded delete: {e}");
                Hlc::now(&self.device_id)
            }
        };

        let deletion_cs = ChangeSet {
            kind: ChangeSetKind::DeletionEvent,
            origin_device_id: self.device_id.clone(),
            origin_device_name: self.device_name.clone(),
            watermark,
            ..Default::default()
        };

        let delivered = match self.transport.push(&deletion_cs).await {
            Ok(n) => n,
            Err(e) => {
                self.bump_erasure_backoff(id);
                return Err(e);
            }
        };
        if delivered == 0 {
            // Nothing reached a peer (e.g. no LAN peers). Do NOT mark propagated, so a
            // later cycle/restart retries (durability) — but back off the cadence.
            self.bump_erasure_backoff(id);
            debug!("deletion event reached no peers; will retry (backing off)");
            return Ok(None);
        }
        // Delivered → clear any backoff for this erasure.
        self.reset_erasure_backoff();

        // #5143/#5156: audit the egress, keyed on the PER-ERASURE id so distinct
        // erasures get distinct ledger rows and a re-announce of the same one dedups.
        self.record_push_egress(&deletion_cs, "DeletionEvent", id, delivered);

        if persist {
            // Consent-revoke path: DURABLE fire-once. Advance the in-memory mirror ONLY
            // once the id is durably persisted (or there is no store, e.g. tests).
            // Otherwise a failed persist + restart would re-announce the erasure and
            // re-delete post-re-grant data on peers — so on persist failure we leave the
            // mirror unchanged and retry next cycle. The retry re-pushes the (unbounded)
            // DeletionEvent each cycle until the retained-table write finally succeeds;
            // this is idempotent for already-erased data, with a NARROW residual window
            // if a re-grant + new-data sync interleaves before the write succeeds (a
            // rare app_meta-write failure). Bounding that window is a #5156 follow-up.
            match &self.erasure_store {
                Some(store) => match store.record_pushed_erasure_id(id) {
                    Ok(()) => *self.last_pushed_erasure_id.lock() = Some(id.to_string()),
                    Err(e) => {
                        warn!(err.code = %e.code(), "failed to persist last_pushed_erasure_id (will retry): {e}");
                    }
                },
                None => *self.last_pushed_erasure_id.lock() = Some(id.to_string()),
            }
        } else {
            // Web path: in-memory one-shot.
            self.deletion_pushed.store(true, Ordering::Release);
        }

        info!(erasure_id = id, "GDPR deletion event propagated");
        Ok(None)
    }

    /// True while the erasure with `id` is in its post-failure backoff window (#5165).
    fn erasure_backoff_active(&self, id: &str) -> bool {
        let st = self.erasure_backoff.lock();
        st.id.as_deref() == Some(id)
            && st
                .retry_after
                .map(|t| tokio::time::Instant::now() < t)
                .unwrap_or(false)
    }

    /// Grow the backoff after a non-delivery (no peers / transport error): exponential
    /// `30s·2^(n-1)` capped at 30 min. A different erasure id resets the failure count.
    fn bump_erasure_backoff(&self, id: &str) {
        let mut st = self.erasure_backoff.lock();
        if st.id.as_deref() != Some(id) {
            *st = ErasureBackoff {
                id: Some(id.to_string()),
                failures: 0,
                retry_after: None,
            };
        }
        st.failures = st.failures.saturating_add(1);
        let secs = 30u64
            .saturating_mul(1u64 << (st.failures - 1).min(6))
            .min(1800);
        st.retry_after = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(secs));
    }

    /// Clear the backoff once a delivery is confirmed.
    fn reset_erasure_backoff(&self) {
        *self.erasure_backoff.lock() = ErasureBackoff::default();
    }

    // ── Public accessors for IPC/REST ──────────────────────────────────

    /// Device identity of this sync engine instance.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Human-readable device name.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Returns `(last_sync_at_rfc3339, last_error)` for health reporting.
    pub fn health_status(&self) -> (Option<String>, Option<String>) {
        let sync_at = self.last_sync_at.lock().as_ref().map(|d| d.to_rfc3339());
        let error = self.last_error.lock().clone();
        (sync_at, error)
    }

    /// Discover known peers via the configured transport.
    pub async fn discover_peers(
        &self,
    ) -> Result<Vec<maekon_core::models::sync::PeerInfo>, CoreError> {
        self.transport.discover_peers().await
    }

    /// Remove a peer from the transport's known-peers list.
    pub async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        self.transport.forget_peer(device_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::models::sync::PeerInfo;
    use maekon_core::ports::consent_manager::ConsentManagerPort;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- Mock implementations ---

    struct MockExtractor {
        changeset: ChangeSet,
        /// Records the `since` argument from each `get_changes_since` call.
        since_log: std::sync::Mutex<Vec<Hlc>>,
    }

    impl MockExtractor {
        fn new(changeset: ChangeSet) -> Self {
            Self {
                changeset,
                since_log: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ChangeExtractor for MockExtractor {
        async fn get_changes_since(&self, since: &Hlc) -> Result<ChangeSet, CoreError> {
            self.since_log.lock().unwrap().push(since.clone());
            Ok(self.changeset.clone())
        }
        async fn local_watermark(&self) -> Result<Hlc, CoreError> {
            Ok(self.changeset.watermark.clone())
        }
    }

    struct MockMerger {
        apply_count: AtomicUsize,
    }

    #[async_trait]
    impl ChangeMerger for MockMerger {
        async fn apply_changes(&self, _changes: ChangeSet) -> Result<SyncResult, CoreError> {
            self.apply_count.fetch_add(1, Ordering::SeqCst);
            Ok(SyncResult {
                applied: 1,
                ..Default::default()
            })
        }
    }

    struct MockTransport {
        pull_result: std::sync::Mutex<Vec<Option<ChangeSet>>>,
        push_count: AtomicUsize,
    }

    #[async_trait]
    impl SyncTransport for MockTransport {
        async fn push(&self, _changes: &ChangeSet) -> Result<usize, CoreError> {
            self.push_count.fetch_add(1, Ordering::SeqCst);
            Ok(1) // one confirmed delivery (single mock destination)
        }
        async fn pull(&self, _since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
            let mut results = self.pull_result.lock().unwrap();
            if results.is_empty() {
                Ok(None)
            } else {
                Ok(results.remove(0))
            }
        }
        async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
            Ok(vec![])
        }
    }

    /// Captures every egress record so a test can assert what the engine
    /// audited (#5143).
    struct MockEgressSink {
        records: std::sync::Mutex<Vec<EgressLedgerRecord>>,
    }
    impl MockEgressSink {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                records: std::sync::Mutex::new(Vec::new()),
            })
        }
        fn records(&self) -> Vec<EgressLedgerRecord> {
            self.records.lock().unwrap().clone()
        }
    }
    impl EgressLedgerSink for MockEgressSink {
        fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError> {
            self.records.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    /// In-memory `ErasurePropagationStore` (#5156). Survives across two engines in a
    /// test the way the persisted store survives a restart.
    #[derive(Default)]
    struct MockErasureStore {
        last: std::sync::Mutex<Option<String>>,
    }
    impl MockErasureStore {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
    }
    impl ErasurePropagationStore for MockErasureStore {
        fn last_pushed_erasure_id(&self) -> Option<String> {
            self.last.lock().unwrap().clone()
        }
        fn record_pushed_erasure_id(&self, id: &str) -> Result<(), CoreError> {
            *self.last.lock().unwrap() = Some(id.to_string());
            Ok(())
        }
    }

    /// An erasure store whose persist always fails — to prove the fire-once gate does
    /// NOT advance its in-memory mirror on a failed persist (so it retries rather than
    /// risk a restart re-announce of an un-persisted erasure).
    struct FailingErasureStore;
    impl ErasurePropagationStore for FailingErasureStore {
        fn last_pushed_erasure_id(&self) -> Option<String> {
            None
        }
        fn record_pushed_erasure_id(&self, _id: &str) -> Result<(), CoreError> {
            Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "simulated erasure-store failure".into(),
            })
        }
    }

    /// A transport whose push reports ZERO confirmed deliveries — models a LAN
    /// push with no peers (or all peers failed): the cycle "succeeds" but
    /// nothing actually left the device (#5143 deep-review regression guard).
    struct ZeroDeliveryTransport;
    #[async_trait]
    impl SyncTransport for ZeroDeliveryTransport {
        async fn push(&self, _changes: &ChangeSet) -> Result<usize, CoreError> {
            Ok(0)
        }
        async fn pull(&self, _since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
            Ok(None)
        }
        async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
            Ok(vec![])
        }
    }

    /// Multi-peer delivery transport — push reaches N peers (LAN fan-out, #5147 item 2).
    struct MultiPeerTransport {
        peers: usize,
    }
    #[async_trait]
    impl SyncTransport for MultiPeerTransport {
        async fn push(&self, _changes: &ChangeSet) -> Result<usize, CoreError> {
            Ok(self.peers)
        }
        async fn pull(&self, _since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
            Ok(None)
        }
        async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
            Ok(vec![])
        }
    }

    /// Zero-delivery transport that COUNTS pushes — for the #5165 backoff test.
    #[derive(Default)]
    struct CountingZeroDeliveryTransport {
        pushes: AtomicUsize,
    }
    impl CountingZeroDeliveryTransport {
        fn count(&self) -> usize {
            self.pushes.load(Ordering::SeqCst)
        }
    }
    #[async_trait]
    impl SyncTransport for CountingZeroDeliveryTransport {
        async fn push(&self, _changes: &ChangeSet) -> Result<usize, CoreError> {
            self.pushes.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        }
        async fn pull(&self, _since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
            Ok(None)
        }
        async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
            Ok(vec![])
        }
    }

    /// A sink that always fails, to pin the best-effort/warn-only contract: a
    /// ledger-write error must NOT fail the sync push it audits.
    struct FailingEgressSink;
    impl EgressLedgerSink for FailingEgressSink {
        fn record_egress(&self, _record: &EgressLedgerRecord) -> Result<(), CoreError> {
            Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "simulated ledger failure".into(),
            })
        }
    }

    fn make_consent_manager(sync_granted: bool) -> Option<Arc<dyn ConsentManagerPort>> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let cm = ConsentManager::new(path);
        if sync_granted {
            cm.grant_consent(
                ConsentPermissions {
                    cross_device_sync: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        }
        // Leak the tempdir to keep the path alive
        std::mem::forget(dir);
        Some(Arc::new(cm))
    }

    /// Build a ConsentManager with pending_deletion=true and cross_device_sync
    /// consent granted. This simulates a revoke-then-re-grant scenario.
    fn make_consent_manager_with_pending_deletion() -> Arc<dyn ConsentManagerPort> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let cm = ConsentManager::new(path);
        // Grant, revoke (sets pending_deletion=true), then re-grant with sync.
        cm.grant_consent(
            ConsentPermissions {
                cross_device_sync: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        cm.revoke_consent().unwrap();
        cm.grant_consent(
            ConsentPermissions {
                cross_device_sync: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        std::mem::forget(dir);
        Arc::new(cm)
    }

    #[tokio::test]
    async fn cycle_skipped_when_consent_not_granted() {
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            Arc::new(MockTransport {
                pull_result: std::sync::Mutex::new(vec![]),
                push_count: AtomicUsize::new(0),
            }),
            make_consent_manager(false),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn normal_pull_merge_push_cycle() {
        let remote_cs = ChangeSet {
            origin_device_id: "dev-b".to_string(),
            origin_device_name: "Remote".to_string(),
            segments: vec![serde_json::json!({"id": "seg-1"})],
            ..Default::default()
        };

        let merger = Arc::new(MockMerger {
            apply_count: AtomicUsize::new(0),
        });
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![Some(remote_cs), None]),
            push_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "local-seg"})],
                origin_device_id: "dev-a".to_string(),
                ..Default::default()
            })),
            merger.clone(),
            transport.clone(),
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_some());
        assert_eq!(merger.apply_count.load(Ordering::SeqCst), 1);
        assert!(transport.push_count.load(Ordering::SeqCst) >= 1);
    }

    // ── #5143: egress-ledger auditing ──────────────────────────────────

    #[tokio::test]
    async fn successful_push_records_uploaded_egress() {
        let sink = MockEgressSink::new();
        let cs = ChangeSet {
            segments: vec![serde_json::json!({"id": "local-seg"})],
            origin_device_id: "dev-a".to_string(),
            ..Default::default()
        };
        // byte_count must equal the EXACT plaintext serialized size of the
        // pushed changeset (not just > 0, which any non-empty payload passes).
        let expected_bytes = serde_json::to_vec(&cs).unwrap().len() as i64;
        let expected_wm = serde_json::to_string(&cs.watermark).unwrap();
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]), // push-only
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(cs)),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_egress(sink.clone(), "sync.test".to_string());

        engine.run_cycle().await.unwrap();

        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);
        let records = sink.records();
        assert_eq!(
            records.len(),
            1,
            "exactly one egress row per successful push"
        );
        let r = &records[0];
        assert_eq!(r.disposition, "uploaded");
        assert_eq!(r.event_type, "CrossDeviceSync");
        assert_eq!(r.destination, "sync.test");
        assert_eq!(
            r.byte_count, expected_bytes,
            "byte_count is the plaintext serialized changeset size"
        );
        assert!(
            r.consent_state.contains("cross_device_sync=true"),
            "consent snapshot recorded: {}",
            r.consent_state
        );
        // #5147: deterministic record_id (destination|event_type|watermark) so a
        // crash-retry re-push dedups; event_id correlates to the HLC watermark.
        assert_eq!(
            r.record_id,
            format!("egress|sync.test|CrossDeviceSync|{expected_wm}")
        );
        assert_eq!(r.event_id.as_deref(), Some(expected_wm.as_str()));
        assert_eq!(
            r.recipient_count, 1,
            "single mock destination → 1 recipient"
        );
    }

    #[tokio::test]
    async fn lan_fanout_records_recipient_count() {
        // #5147 item 2: a multi-peer LAN push records recipient_count = the delivered-peer
        // count, so the audit reflects the true aggregate egress (byte_count * recipients),
        // not a single-recipient under-count.
        let sink = MockEgressSink::new();
        let cs = ChangeSet {
            segments: vec![serde_json::json!({"id": "seg-fan"})],
            origin_device_id: "dev-a".to_string(),
            ..Default::default()
        };
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(cs)),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            Arc::new(MultiPeerTransport { peers: 3 }),
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_egress(sink.clone(), "sync.lan".to_string());

        engine.run_cycle().await.unwrap();

        let records = sink.records();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].recipient_count, 3,
            "egress records the 3 LAN recipients (fan-out grain)"
        );
    }

    #[tokio::test]
    async fn deletion_event_push_records_uploaded_egress() {
        let sink = MockEgressSink::new();
        let consent = make_consent_manager_with_pending_deletion();
        let erasure_id = consent
            .pending_erasure_id()
            .expect("revoke set a per-erasure id");
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            Some(consent.clone()),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_egress(sink.clone(), "sync.test".to_string());

        engine.run_cycle().await.unwrap();

        let records = sink.records();
        assert_eq!(records.len(), 1, "the GDPR deletion push is audited too");
        let r = &records[0];
        assert_eq!(r.event_type, "DeletionEvent");
        assert_eq!(r.disposition, "uploaded");
        assert_eq!(r.destination, "sync.test");
        assert!(r.byte_count > 0, "the tombstone changeset has bytes");
        // The consent snapshot at the Art.17 deletion egress is the key evidence
        // the ledger exists to capture.
        assert!(
            r.consent_state.starts_with("cross_device_sync="),
            "consent snapshot recorded at deletion egress: {}",
            r.consent_state
        );
        // #5156: the DeletionEvent dedup_key is now the PER-ERASURE id (the consent
        // revocation instant), so distinct erasures get distinct audit rows and a
        // re-announce of the SAME erasure dedups.
        assert_eq!(
            r.record_id,
            format!("egress|sync.test|DeletionEvent|{erasure_id}")
        );
        // event_id is the tombstone's HLC watermark — verify its CONTENT (carries
        // this device's id), not just presence.
        let ev: Hlc = serde_json::from_str(r.event_id.as_deref().unwrap()).unwrap();
        assert_eq!(
            ev.device_id, "dev-a",
            "event_id correlates to the tombstone HLC"
        );
    }

    // ── #5156 stage 2: persisted fire-once gate ────────────────────────

    fn deletion_engine_builder(
        transport: Arc<dyn SyncTransport>,
        consent: Arc<dyn ConsentManagerPort>,
    ) -> impl std::future::Future<Output = SyncEngine> {
        SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport,
            Some(consent),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
    }

    #[tokio::test]
    async fn consent_erasure_propagates_once_then_dedups() {
        let store = MockErasureStore::new();
        let consent = make_consent_manager_with_pending_deletion();
        let erasure_id = consent.pending_erasure_id().unwrap();
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = deletion_engine_builder(transport.clone(), consent)
            .await
            .with_erasure_store(store.clone());

        engine.run_cycle().await.unwrap(); // propagates the erasure
        engine.run_cycle().await.unwrap(); // must NOT re-announce (same id)

        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "a consent erasure propagates exactly once per id"
        );
        assert_eq!(
            store.last_pushed_erasure_id(),
            Some(erasure_id),
            "the propagated erasure id is persisted"
        );
    }

    #[tokio::test]
    async fn persisted_id_prevents_reannounce_across_restart() {
        // The headline fix: a re-announce after restart would make peers re-run the
        // unbounded delete on post-re-grant data. The persisted id prevents it.
        let store = MockErasureStore::new();
        let consent = make_consent_manager_with_pending_deletion();

        let t1 = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine1 = deletion_engine_builder(t1.clone(), consent.clone())
            .await
            .with_erasure_store(store.clone());
        engine1.run_cycle().await.unwrap();
        assert_eq!(t1.push_count.load(Ordering::SeqCst), 1);

        // "Restart": a fresh engine seeded from the SAME persisted store. The consent
        // still reports the erasure pending (durable id), but it must NOT re-announce.
        let t2 = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine2 = deletion_engine_builder(t2.clone(), consent.clone())
            .await
            .with_erasure_store(store.clone());
        engine2.run_cycle().await.unwrap();
        assert_eq!(
            t2.push_count.load(Ordering::SeqCst),
            0,
            "a restart must NOT re-announce an already-propagated erasure"
        );
    }

    #[tokio::test]
    async fn distinct_second_erasure_refires() {
        let store = MockErasureStore::new();
        // The helper leaves the manager revoked-then-re-granted: cross_device_sync is
        // ON (so the deletion gate is reachable past Gate 1) with erasure id1 pending.
        let consent = make_consent_manager_with_pending_deletion();
        let id1 = consent.pending_erasure_id().unwrap();

        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = deletion_engine_builder(transport.clone(), consent.clone())
            .await
            .with_erasure_store(store.clone());
        engine.run_cycle().await.unwrap();
        assert_eq!(store.last_pushed_erasure_id(), Some(id1.clone()));
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);

        // A genuinely DISTINCT second erasure: revoke again (id2), then re-grant so
        // the cross_device_sync gate is open again for propagation.
        consent.revoke_consent().unwrap();
        consent
            .grant_consent(
                ConsentPermissions {
                    cross_device_sync: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        let id2 = consent.pending_erasure_id().unwrap();
        assert_ne!(id1, id2, "the second erasure has a distinct id");

        engine.run_cycle().await.unwrap();
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            2,
            "a distinct second erasure re-fires"
        );
        assert_eq!(store.last_pushed_erasure_id(), Some(id2));
    }

    #[tokio::test]
    async fn offline_deletion_not_persisted_so_it_retries() {
        // Durability: a deletion that reaches NO peer must not be marked propagated,
        // so a later cycle/restart retries it.
        let store = MockErasureStore::new();
        let consent = make_consent_manager_with_pending_deletion();
        let engine = deletion_engine_builder(Arc::new(ZeroDeliveryTransport), consent)
            .await
            .with_erasure_store(store.clone());

        engine.run_cycle().await.unwrap();
        assert_eq!(
            store.last_pushed_erasure_id(),
            None,
            "a zero-delivery deletion must NOT be recorded as propagated"
        );
    }

    #[tokio::test]
    async fn persist_failure_does_not_advance_the_gate_so_it_retries() {
        // If the durable persist fails, the in-memory mirror must NOT advance — else a
        // restart would re-announce an un-persisted erasure (and re-delete post-re-grant
        // data). So the deletion re-fires next cycle (retry) until persistence succeeds.
        let consent = make_consent_manager_with_pending_deletion();
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = deletion_engine_builder(transport.clone(), consent)
            .await
            .with_erasure_store(Arc::new(FailingErasureStore));

        engine.run_cycle().await.unwrap();
        engine.run_cycle().await.unwrap();
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            2,
            "a failed persist must leave the gate open so the erasure retries"
        );
    }

    #[tokio::test]
    async fn web_erasure_fires_even_when_consent_erasure_already_propagated() {
        // #5156 review BLOCKER regression: a pending-but-propagated consent erasure must
        // NOT shadow a web "Delete all data". `clear_pending_deletion` has no production
        // caller and re-grant keeps the consent id, so without the fall-through a web
        // erase of post-re-grant data would be silently dropped on peers (Art.17
        // under-propagation).
        let store = MockErasureStore::new();
        let consent = make_consent_manager_with_pending_deletion();
        let web_flag = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            Some(consent),
            Some(web_flag.clone()),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_erasure_store(store.clone());

        // Cycle 1: the consent erasure propagates (and is persisted).
        engine.run_cycle().await.unwrap();
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);

        // The user now triggers a web "Delete all data" of post-re-grant data.
        web_flag.store(true, Ordering::Release);
        // Cycle 2: the consent erasure is pending-but-propagated → must fall through to
        // the web path → a second DeletionEvent fires.
        engine.run_cycle().await.unwrap();
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            2,
            "a web erasure must propagate even while a propagated consent erasure is pending"
        );
    }

    // ── #5165: revoke-and-walk-away — erasure bypasses the consent gate ──

    #[tokio::test]
    async fn erasure_propagates_even_when_consent_revoked() {
        // A plain revoke turns cross_device_sync OFF (Gate 1 would skip normal sync),
        // but the pending erasure MUST still reach peers (a tombstone is an erasure,
        // not data collection). The dedicated path propagates it.
        let consent = make_consent_manager(true).unwrap();
        consent.revoke_consent().unwrap(); // cross_device_sync now false; erasure pending
        let id = consent.pending_erasure_id().unwrap();
        let store = MockErasureStore::new();
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = deletion_engine_builder(transport.clone(), consent)
            .await
            .with_erasure_store(store.clone());

        let propagated = engine.propagate_pending_erasure().await.unwrap();
        assert!(
            propagated,
            "a pending erasure propagates despite revoked consent"
        );
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);
        assert_eq!(store.last_pushed_erasure_id(), Some(id));
    }

    #[tokio::test]
    async fn run_cycle_propagates_erasure_before_the_consent_gate() {
        // run_cycle_inner runs the erasure FIRST, so even a revoked-consent cycle
        // (which Gate 1 would otherwise skip) still propagates the deletion.
        let consent = make_consent_manager(true).unwrap();
        consent.revoke_consent().unwrap();
        let store = MockErasureStore::new();
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = deletion_engine_builder(transport.clone(), consent)
            .await
            .with_erasure_store(store);

        engine.run_cycle().await.unwrap();
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "run_cycle propagates the erasure before the consent gate"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn undeliverable_erasure_backs_off_then_retries() {
        // #5165: an erasure that reaches no peer must NOT push every tick forever —
        // it backs off (capped exponential) but still retries once the window elapses.
        let consent = make_consent_manager_with_pending_deletion();
        let transport = Arc::new(CountingZeroDeliveryTransport::default());
        let engine = deletion_engine_builder(transport.clone(), consent)
            .await
            .with_erasure_store(MockErasureStore::new());

        // First attempt pushes (zero delivery) and arms the backoff (~30s).
        engine.propagate_pending_erasure().await.unwrap();
        assert_eq!(transport.count(), 1);

        // An immediate retry (no time advance) is SKIPPED — backed off.
        engine.propagate_pending_erasure().await.unwrap();
        assert_eq!(transport.count(), 1, "backed off → no push this tick");

        // Once the backoff window elapses, it retries (durability preserved).
        tokio::time::advance(std::time::Duration::from_secs(31)).await;
        engine.propagate_pending_erasure().await.unwrap();
        assert_eq!(transport.count(), 2, "retries once the backoff elapses");
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_does_not_block_normal_sync_in_run_cycle() {
        // #5165 review regression: during an erasure backoff window, run_cycle must NOT
        // short-circuit — normal data sync must still push local changes for a
        // still-consented device (else up to 30 min of activity is stranded).
        let consent = make_consent_manager_with_pending_deletion(); // cross_device_sync granted
        let transport = Arc::new(CountingZeroDeliveryTransport::default());
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "local"})],
                origin_device_id: "dev-a".to_string(),
                ..Default::default()
            })),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            Some(consent),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_erasure_store(MockErasureStore::new());

        // Cycle 1: the erasure is attempted (zero delivery) → arms the backoff.
        engine.run_cycle().await.unwrap();
        assert_eq!(transport.count(), 1, "cycle 1 attempts the deletion");

        // Cycle 2 (immediate, backed off): the deletion is deferred, so normal sync
        // runs and pushes the local changeset — it is NOT blocked by the backoff.
        engine.run_cycle().await.unwrap();
        assert_eq!(
            transport.count(),
            2,
            "during backoff, normal sync still pushes local changes"
        );
    }

    #[tokio::test]
    async fn record_id_is_deterministic_for_same_push() {
        // Two independent pushes of the SAME changeset (same watermark) to the
        // same destination must yield the SAME record_id, so the store's
        // INSERT OR IGNORE dedups a crash/restart re-push (#5147).
        async fn push_once(cs: ChangeSet) -> String {
            let sink = MockEgressSink::new();
            let engine = SyncEngine::new(
                Arc::new(MockExtractor::new(cs)),
                Arc::new(MockMerger {
                    apply_count: AtomicUsize::new(0),
                }),
                Arc::new(MockTransport {
                    pull_result: std::sync::Mutex::new(vec![]),
                    push_count: AtomicUsize::new(0),
                }),
                make_consent_manager(true),
                None,
                "dev-a".to_string(),
                "Test".to_string(),
            )
            .await
            .with_egress(sink.clone(), "sync.test".to_string());
            engine.run_cycle().await.unwrap();
            sink.records()[0].record_id.clone()
        }

        // NON-default watermark so the test proves the watermark genuinely flows
        // into the id (a default Hlc serializes to a constant and would pass even
        // if the watermark were dropped from the key).
        let wm = Hlc {
            wall_ms: 1_717_000_000_000,
            counter: 7,
            device_id: "dev-a".to_string(),
        };
        let cs = ChangeSet {
            segments: vec![serde_json::json!({"id": "seg"})],
            origin_device_id: "dev-a".to_string(),
            watermark: wm.clone(),
            ..Default::default()
        };
        let id1 = push_once(cs.clone()).await;
        let id2 = push_once(cs).await;
        assert_eq!(
            id1, id2,
            "same logical push must yield the same record_id (enables dedup)"
        );
        assert!(
            id1.contains(&serde_json::to_string(&wm).unwrap()),
            "the watermark must flow into the record_id: {id1}"
        );
    }

    #[tokio::test]
    async fn different_watermark_yields_different_record_id() {
        // The other half of dedup correctness: two DISTINCT batches (different
        // watermarks) to the same destination must get DIFFERENT record_ids, so a
        // genuine second egress is never silently collapsed by INSERT OR IGNORE.
        async fn push_with_watermark(wall_ms: u64) -> String {
            let sink = MockEgressSink::new();
            let engine = SyncEngine::new(
                Arc::new(MockExtractor::new(ChangeSet {
                    segments: vec![serde_json::json!({"id": "seg"})],
                    origin_device_id: "dev-a".to_string(),
                    watermark: Hlc {
                        wall_ms,
                        counter: 0,
                        device_id: "dev-a".to_string(),
                    },
                    ..Default::default()
                })),
                Arc::new(MockMerger {
                    apply_count: AtomicUsize::new(0),
                }),
                Arc::new(MockTransport {
                    pull_result: std::sync::Mutex::new(vec![]),
                    push_count: AtomicUsize::new(0),
                }),
                make_consent_manager(true),
                None,
                "dev-a".to_string(),
                "Test".to_string(),
            )
            .await
            .with_egress(sink.clone(), "sync.test".to_string());
            engine.run_cycle().await.unwrap();
            sink.records()[0].record_id.clone()
        }
        let id1 = push_with_watermark(1000).await;
        let id2 = push_with_watermark(2000).await;
        assert_ne!(
            id1, id2,
            "distinct watermarks must yield distinct record_ids (no false collision)"
        );
    }

    #[tokio::test]
    async fn no_delivery_records_no_egress() {
        // Deep-review regression: a transport confirming ZERO recipients (LAN
        // no-peers / all-fail) must NOT write an `uploaded` row — the ledger
        // must not assert an egress that did not occur.
        let sink = MockEgressSink::new();
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "x"})],
                ..Default::default()
            })),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            Arc::new(ZeroDeliveryTransport),
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_egress(sink.clone(), "sync.test".to_string());

        engine.run_cycle().await.unwrap();

        assert!(
            sink.records().is_empty(),
            "zero confirmed deliveries must record no egress"
        );
    }

    #[tokio::test]
    async fn egress_sink_failure_does_not_fail_the_sync() {
        // Best-effort contract: a ledger-write Err is swallowed; the push and the
        // cycle still succeed (a regression swapping warn-only for `?` fails here).
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "x"})],
                ..Default::default()
            })),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_egress(Arc::new(FailingEgressSink), "sync.test".to_string());

        engine
            .run_cycle()
            .await
            .expect("a ledger-write failure must NOT fail the sync it audits");
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "the push still happened despite the ledger failure"
        );
    }

    #[tokio::test]
    async fn consent_blocked_cycle_records_no_egress() {
        let sink = MockEgressSink::new();
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "x"})],
                ..Default::default()
            })),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            make_consent_manager(false), // cross_device_sync NOT granted
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await
        .with_egress(sink.clone(), "sync.test".to_string());

        engine.run_cycle().await.unwrap();

        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            0,
            "the consent gate must block the push"
        );
        assert!(
            sink.records().is_empty(),
            "nothing left the device, so nothing is audited"
        );
    }

    #[tokio::test]
    async fn empty_pull_results_in_push_only() {
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let merger = Arc::new(MockMerger {
            apply_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "local-seg"})],
                origin_device_id: "dev-a".to_string(),
                ..Default::default()
            })),
            merger.clone(),
            transport.clone(),
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_none()); // no merge happened
        assert_eq!(merger.apply_count.load(Ordering::SeqCst), 0);
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn deletion_event_pushed_when_pending() {
        let consent_mgr = make_consent_manager_with_pending_deletion();

        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            Some(consent_mgr),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(result.is_none());
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);

        // Second cycle should NOT re-push the deletion event (local flag cleared)
        let result2 = engine.run_cycle().await.unwrap();
        assert!(result2.is_none());
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "deletion should only be pushed once"
        );
    }

    #[tokio::test]
    async fn erasure_request_triggers_deletion_event() {
        // #4478 G3: a local "Delete all data" sets `erasure_requested`; the next
        // sync cycle must propagate a device-wide DeletionEvent to peers (once),
        // even though consent is GRANTED with no pending consent-revocation delete.
        let erasure = Arc::new(AtomicBool::new(true));
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet::default())),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            make_consent_manager(true), // consent granted, NO pending revocation
            Some(erasure.clone()),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        let result = engine.run_cycle().await.unwrap();
        assert!(
            result.is_none(),
            "deletion-event cycle returns no merge result"
        );
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "erasure request propagates exactly one DeletionEvent"
        );
        // Fires once: a second cycle does NOT re-push (deduped by `deletion_pushed`).
        engine.run_cycle().await.unwrap();
        assert_eq!(
            transport.push_count.load(Ordering::SeqCst),
            1,
            "deletion event pushed only once"
        );
    }

    #[tokio::test]
    async fn no_deletion_without_erasure_or_pending() {
        // Negative: consent granted, erasure_requested=false, no pending → normal
        // push path, never a DeletionEvent short-circuit.
        let erasure = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });
        let engine = SyncEngine::new(
            Arc::new(MockExtractor::new(ChangeSet {
                segments: vec![serde_json::json!({"id": "local-seg"})],
                origin_device_id: "dev-a".to_string(),
                ..Default::default()
            })),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport.clone(),
            make_consent_manager(true),
            Some(erasure),
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        engine.run_cycle().await.unwrap();
        // One push, but it is the NORMAL extract+push (non-empty changeset), not a
        // DeletionEvent — the gate did not short-circuit.
        assert_eq!(transport.push_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn push_watermark_advances_after_successful_push() {
        let watermark = Hlc {
            wall_ms: 5000,
            counter: 3,
            device_id: "dev-a".to_string(),
        };
        let extractor = Arc::new(MockExtractor::new(ChangeSet {
            segments: vec![serde_json::json!({"id": "seg-1"})],
            origin_device_id: "dev-a".to_string(),
            watermark: watermark.clone(),
            ..Default::default()
        }));
        let transport = Arc::new(MockTransport {
            pull_result: std::sync::Mutex::new(vec![]),
            push_count: AtomicUsize::new(0),
        });

        let engine = SyncEngine::new(
            extractor.clone(),
            Arc::new(MockMerger {
                apply_count: AtomicUsize::new(0),
            }),
            transport,
            make_consent_manager(true),
            None,
            "dev-a".to_string(),
            "Test".to_string(),
        )
        .await;

        // First cycle: extractor is called with the initial watermark (seeded from local_watermark)
        engine.run_cycle().await.unwrap();
        // Second cycle: extractor should receive the advanced watermark, not Hlc::default()
        engine.run_cycle().await.unwrap();

        let log = extractor.since_log.lock().unwrap();
        assert_eq!(log.len(), 2);
        // Both calls should use the same watermark since the changeset watermark
        // equals the initial local_watermark.
        assert_eq!(log[0], watermark, "first push should use seeded watermark");
        assert_eq!(
            log[1], watermark,
            "second push should use advanced watermark"
        );
    }
}
