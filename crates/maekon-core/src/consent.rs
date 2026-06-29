use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::CoreError;

pub const CURRENT_POLICY_VERSION: &str = "1.0.0";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsentPermissions {
    // --- Tier 1 ---
    #[serde(default)]
    pub screen_capture: bool,
    #[serde(default)]
    pub ocr_processing: bool,
    #[serde(default)]
    pub telemetry: bool,
    #[serde(default)]
    pub process_monitoring: bool,
    #[serde(default)]
    pub input_activity: bool,

    // --- Tier 2 ---
    #[serde(default)]
    pub window_title_collection: bool,
    #[serde(default)]
    pub app_usage_analytics: bool,

    // --- Tier 3 ---
    #[serde(default)]
    pub clipboard_monitoring: bool,
    #[serde(default)]
    pub file_access_monitoring: bool,

    // --- Tier 4: Tiered Memory ---
    #[serde(default)]
    pub activity_pattern_learning: bool,

    // --- Tier 5: Cross-Device Sync ---
    /// Permits cross-device synchronization of activity data.
    /// GDPR Article 6 -- processing requires explicit consent for data
    /// transfer between devices, even when both are owned by the same user.
    #[serde(default)]
    pub cross_device_sync: bool,

    // --- Tier 6: Text Intelligence ---
    /// Permits extraction of full text content from focused UI elements.
    /// Required only when pii_extraction_level is set to Off.
    /// GDPR Article 6 -- explicit consent for processing text content
    /// that may contain personal data.
    #[serde(default)]
    pub full_text_extraction: bool,

    // --- Tier 7: Memory-Graph Enrichment ---
    /// Permits feeding durable, activity-derived memory-graph claim text to a
    /// LOCAL LLM for relation/contradiction inference (ADR-023 Phase-2 D1/D2).
    /// GDPR Article 6 -- explicit consent. Default false (fail-closed); this is a
    /// dedicated permission, NOT borrowed from `full_text_extraction` (which gates
    /// the active-window external-LLM path) or `activity_pattern_learning`.
    #[serde(default)]
    pub memory_graph_enrichment: bool,

    // --- Tier 8: Audio/Voice ---
    /// Permits microphone capture (continuous voice-activity listening + push-to-talk)
    /// and the resulting speech-to-text. Higher sensitivity than screen: captures
    /// audio + transcripts, and -- if `audio.stt_provider` is Cloud with an API key --
    /// sends raw audio off-device, unfiltered, to a third-party endpoint (#4568).
    /// GDPR Article 6 -- explicit consent. Default false (fail-closed); this is a
    /// dedicated permission, NOT borrowed from `screen_capture` (granting screen
    /// consent must never silently authorize the mic).
    #[serde(default)]
    pub microphone: bool,

    // --- Tier 9: Raw Off-Device OCR ---
    /// Permits sending unredacted screenshots to an external OCR provider when
    /// the `bypass_pii_filter_for_external_ocr` config flag is explicitly enabled.
    /// This is separate from generic OCR processing consent because it bypasses
    /// local PII filtering before off-device transfer.
    #[serde(default)]
    pub unredacted_external_ocr: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub consent_id: String,
    pub version: String,
    pub granted_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    /// Timestamp recorded when the user revokes consent (GDPR Article 17 audit trail).
    #[serde(default)]
    pub revoked_at: Option<DateTime<Utc>>,
    /// Set to true after revocation to signal that queued data must be purged
    /// before the next upload cycle (GDPR Article 17 — right to erasure).
    #[serde(default)]
    pub data_deletion_requested: bool,
    /// #5165: a collision-proof per-revoke nonce (a ULID), minted at revoke time so
    /// the per-erasure id never rests on wall-clock distinctness. `None` for a
    /// granted record and for any erasure revoked before this field existed — in the
    /// latter case `pending_erasure_id` falls back to the `revoked_at` timestamp so
    /// an in-flight pre-upgrade erasure keeps its restart identity (no re-announce).
    #[serde(default)]
    pub erasure_nonce: Option<String>,
    pub permissions: ConsentPermissions,
    pub data_retention_days: u32,
}

// F-RC-C37-04: explicit PascalCase wire contract. Serde default would also produce
// PascalCase for these variants, but the attribute makes the contract undeniable.
// Existing consent JSON files use PascalCase (written by ConsentManager::save_to_file)
// so this is fully backward-compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ConsentStatus {
    NotGranted,
    Valid,
    Expired,
    UpdateRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserDataExport {
    pub exported_at: DateTime<Utc>,
    pub consent: Option<ConsentRecord>,
    pub settings: serde_json::Value,
    pub event_count: u64,
    pub frame_count: u64,
    pub export_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletionResult {
    pub deleted_at: DateTime<Utc>,
    pub events_deleted: u64,
    pub frames_deleted: u64,
    pub metrics_deleted: u64,
    pub settings_reset: bool,
    pub consent_revoked: bool,
}

// ConsentManager

/// Internal mutable state. Wrapped in a `RwLock` so writes are possible even
/// though `ConsentManager` is distributed as a shared immutable `Arc` (writers
/// take `&self`).
struct ConsentState {
    current_consent: Option<ConsentRecord>,
    /// Stays true after a revoke_consent() call until data erasure completes.
    /// Managed as a separate in-memory flag so the GDPR Article 17 signal is not
    /// lost even after current_consent = None.
    pending_deletion: bool,
    /// #5156: the instant of the currently pending Art. 17 erasure (a mirror of
    /// the persisted `ConsentRecord.revoked_at`). Provides a restart-stable,
    /// per-erasure-distinct identifier that changes on every revoke. Recovered
    /// from the persisted record on load so the REMOTE propagation signal
    /// survives restart. Cleared together with `pending_deletion`, and left
    /// untouched by grant (#4630-b). Distinct from the LOCAL write gate
    /// (`deletion_flag`) — on restart `deletion_flag` stays false (local erasure
    /// completes within the revoke session).
    pending_erasure_at: Option<DateTime<Utc>>,
    /// #5165: collision-proof per-revoke nonce (mirror of `ConsentRecord.erasure_nonce`),
    /// the preferred per-erasure id. Set/cleared in lockstep with `pending_erasure_at`;
    /// recovered on load. `None` for a pre-upgrade in-flight erasure → `pending_erasure_id`
    /// falls back to the `revoked_at` timestamp.
    pending_erasure_nonce: Option<String>,
}

pub struct ConsentManager {
    storage_path: PathBuf,
    state: parking_lot::RwLock<ConsentState>,
    /// #4928: the LIVE signal of the consent-revoke erasure chokepoint.
    ///
    /// Mirrors `pending_deletion` — revoke → `true`, grant/clear → `false`. When
    /// the composition root installs this same `Arc` into
    /// `SqliteStorage::set_deletion_flag` and
    /// `FrameFileStorage::set_deletion_flag`, any in-flight writer that entered
    /// after a revoke is automatically skipped at the funnel (`write_lock` / the
    /// frame barrier). The consent gate is the steady-state protection, and this
    /// flag is the backstop for the in-flight race during the erase window
    /// (spec §2/§3bis).
    deletion_flag: Arc<AtomicBool>,
    /// #4928 round-3 (FIX B): the signal that blocks the grant_consent-during-erase TOCTOU.
    ///
    /// `grant_consent` can flip `deletion_flag` back to `false`, so if a re-grant
    /// slips into the erase window (after the Phase-1 commit, while Phase-2 is in
    /// progress) an in-flight writer could see the flag cleared and write rows
    /// that survive the wipe. `erasing` is set/cleared via RAII by
    /// `erase_all_local_data` and is NEVER touched by
    /// `grant_consent`/`clear_pending_deletion`. The composition root installs
    /// this same `Arc` into `SqliteStorage::set_erasing` /
    /// `FrameFileStorage::set_erasing`. The write-skip predicate is
    /// `deletion_flag || erasing`.
    erasing: Arc<AtomicBool>,
}

impl ConsentManager {
    pub fn new(storage_path: PathBuf) -> Self {
        // #5156: recover a pending erasure's identity (`revoked_at`) from a
        // revoked-but-not-erased record so the per-erasure id (`pending_erasure_id`)
        // is available across restart for the sync engine's gate.
        //
        // ⚠️ DURABILITY DEFERRED (#5156 stage-1 guard): we do NOT re-arm
        // `pending_deletion` on restart here. Re-arming it makes `has_pending_deletion()`
        // true after restart, which the (current) sync gate turns into a re-announced
        // DeletionEvent — and `handle_deletion_event` is an UNBOUNDED
        // `DELETE WHERE origin_device_id` (sync_merger.rs), so a re-announce after a
        // revoke→re-grant→sync-new-data sequence would DELETE the new data on peers
        // (data loss). Restart durability is re-enabled SAFELY in stage 2 via a
        // *persisted, sender-side fire-once* gate keyed on `pending_erasure_id` (the
        // sender propagates each distinct erasure exactly once, never re-announcing).
        // Public sync/erasure semantics: docs/guides/sync-conflict-resolution.md.
        let (current_consent, pending_erasure_at, pending_erasure_nonce) =
            Self::load_from_file(&storage_path);
        let pending_deletion = false;
        Self {
            storage_path,
            state: parking_lot::RwLock::new(ConsentState {
                current_consent,
                pending_deletion,
                pending_erasure_at,
                pending_erasure_nonce,
            }),
            // #4928: initialize the erasure-blocking flag to false (writes
            // allowed). Even if a pending erasure exists on restart, LOCAL erasure
            // already completed within the revoke session, so we do not re-block
            // local writes (only REMOTE propagation may still be incomplete).
            deletion_flag: Arc::new(AtomicBool::new(false)),
            // #4928 round-3: initialize the erase-window blocking signal to false too.
            erasing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// #4928: returns the shared `deletion_flag` of the erasure-blocking chokepoint.
    ///
    /// The composition root installs this `Arc` into
    /// `SqliteStorage`/`FrameFileStorage` so the same signal is shared (ptr-eq).
    /// Mirrored to `true` on revoke and `false` on grant/clear.
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.clone()
    }

    /// #4928 round-3 (FIX B): returns the erase-window blocking signal `erasing`.
    ///
    /// The composition root installs this `Arc` into
    /// `SqliteStorage::set_erasing` / `FrameFileStorage::set_erasing` so the same
    /// signal is shared (ptr-eq). Only `erase_all_local_data` sets/clears it via
    /// RAII; `grant_consent` cannot clear it.
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.erasing.clone()
    }

    /// Evaluates consent validity against an already-acquired guard's state (no
    /// lock re-entry). This is the single validity-determination logic shared by
    /// both `check_consent` and `effective_permissions` — evaluating within one
    /// guard means there is no TOCTOU window between the two methods.
    fn check_consent_locked(st: &ConsentState) -> ConsentStatus {
        match &st.current_consent {
            None => ConsentStatus::NotGranted,
            Some(record) => {
                if let Some(expires) = record.expires_at {
                    if Utc::now() > expires {
                        return ConsentStatus::Expired;
                    }
                }
                if record.version != CURRENT_POLICY_VERSION {
                    return ConsentStatus::UpdateRequired;
                }
                ConsentStatus::Valid
            }
        }
    }

    pub fn check_consent(&self) -> ConsentStatus {
        Self::check_consent_locked(&self.state.read())
    }

    /// An owned snapshot of the current consent record (the previous signature
    /// was `Option<&ConsentRecord>`). Cloned and returned under the read lock.
    pub fn current_consent(&self) -> Option<ConsentRecord> {
        self.state.read().current_consent.clone()
    }

    /// Returns the permissions only when consent is currently Valid, and returns
    /// all-false otherwise. Every gate uses this method so that the
    /// Expired/UpdateRequired/absent states fail closed.
    ///
    /// Validity check and permission extraction happen under a single read guard,
    /// so there is no torn-state window between the two operations.
    pub fn effective_permissions(&self) -> ConsentPermissions {
        let st = self.state.read(); // single guard — both validity check and permission extraction within it
        if Self::check_consent_locked(&st) == ConsentStatus::Valid {
            st.current_consent
                .as_ref()
                .map(|r| r.permissions.clone())
                .unwrap_or_default()
        } else {
            ConsentPermissions::default()
        }
    }

    /// Atomically reads the status and the (raw) permission set under a single
    /// read guard (for UI snapshots, removing the TOCTOU).
    ///
    /// Calling `check_consent()` + `current_consent()` separately lets a
    /// grant/revoke slip in between the two read locks, yielding a snapshot where
    /// status and permissions disagree. This method reads both within one guard
    /// to remove that window.
    ///
    /// Unlike `effective_permissions`, it does NOT zero the permissions in a
    /// non-Valid state; it returns the **raw granted permissions**
    /// (`current_consent.permissions`) as-is — because the UI must show "what was
    /// granted" alongside the status (e.g. Expired). Never use these permissions
    /// for gate decisions (fail-closed is `effective_permissions`).
    pub fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
        let st = self.state.read(); // single guard — status determination and raw permission extraction together
        let status = Self::check_consent_locked(&st);
        let permissions = st
            .current_consent
            .as_ref()
            .map(|r| r.permissions.clone())
            .unwrap_or_default();
        (status, permissions)
    }

    pub fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError> {
        // ADR-022: client-generated IDs follow prefix+ULID convention (generate_id).
        // consent_id is a String field used locally for audit trail; not validated
        // against UUID format anywhere in the codebase (grep confirmed 2026-05-28).
        let record = ConsentRecord {
            consent_id: crate::id_generation::generate_id("consent"),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions,
            data_retention_days,
        };

        // #6102-2: acquire the writer guard FIRST and persist while holding it, mirroring
        // revoke_consent, so the on-disk rename and the in-memory transition are atomic
        // with respect to other writers. Previously save_to_file ran OUTSIDE the lock and
        // could interleave with revoke_consent's locked tmp+rename, diverging disk from
        // in-memory state. save_to_file only reads the immutable storage_path (no
        // self.state access — verified), so holding the guard across it cannot deadlock,
        // and grant_consent is synchronous so no .await is crossed under the guard.
        let mut st = self.state.write();
        self.save_to_file(&record)?;
        // #4630 (decision b — erasure is irrevocable): we intentionally do NOT clear
        // `pending_deletion` here. If the user revoked (requesting an Art. 17 erasure)
        // and re-grants before that erasure has propagated, the prior erasure still
        // stands — it still fires the cross-device DeletionEvent on the next sync — and
        // this grant only opens a fresh local collection window. Retracting a pending
        // erasure here is the rejected option (a) in #4630.
        st.current_consent = Some(record);
        // #4928: a re-grant opens a fresh LOCAL collection window — clear the
        // erasure-blocking flag to resume funnel writes. (The REMOTE propagation
        // signal `pending_deletion` is left untouched by grant per #4630(b) — the
        // two are distinct signals: the flag is the in-flight local write gate,
        // and pending_deletion is the not-yet-propagated erasure marker.)
        self.deletion_flag.store(false, Ordering::Release);
        Ok(())
    }

    /// Revokes user consent (GDPR Article 7 §3).
    ///
    /// Sets `data_deletion_requested = true` and `revoked_at` on the record,
    /// then atomically writes the updated record to disk (write to .tmp, rename)
    /// so that a crash between write and rename never leaves a partially-written
    /// or deleted file.  `load_from_file()` treats `data_deletion_requested =
    /// true` as revoked, so the file's presence does not re-activate consent.
    pub fn revoke_consent(&self) -> Result<(), CoreError> {
        // Guard the entire read-mutate-persist-clear span under a single write
        // guard. (Releasing and re-acquiring the guard mid-way would open a torn-
        // state window for readers.)
        // #5156: capture the revocation instant once so the persisted `revoked_at`
        // and the in-memory per-erasure id are the SAME value (restart recovery
        // must round-trip to an identical id).
        let now = Utc::now();
        // #5165: a collision-proof per-revoke nonce (ULID), independent of wall-clock,
        // becomes the per-erasure id so two same-instant or clock-rewound revokes never
        // collide. Persisted on the record so it round-trips across restart.
        let nonce = crate::id_generation::generate_id("erasure");
        let mut st = self.state.write();
        // #6114: persist-then-commit, mirroring grant_consent. Build the revoked
        // record into a LOCAL clone WITHOUT mutating st.current_consent, persist it
        // first, and only commit the in-memory transition AFTER the write succeeds.
        // Previously the in-memory record was mutated (revoked_at /
        // data_deletion_requested / erasure_nonce) BEFORE the tmp+rename, so a `?`
        // failure on the write returned Err while leaving consent FAIL-OPEN:
        // check_consent_locked only inspects expires_at/version (not revoked_at /
        // data_deletion_requested), so the still-Some record reported Valid and
        // effective_permissions returned the full granted set, while the un-revoked
        // record stayed on disk (a restart reloaded it as Valid, losing the revoke).
        let mut revoked_active_consent = false;
        if let Some(record) = st.current_consent.as_ref() {
            // Build the revoked record from a clone; do NOT touch st.current_consent.
            let mut revoked = record.clone();
            revoked.revoked_at = Some(now);
            revoked.data_deletion_requested = true;
            revoked.erasure_nonce = Some(nonce.clone());
            revoked_active_consent = true;
            // Atomic write: serialize to a .tmp file, then rename into place.
            // This eliminates the TOCTOU window that existed when we saved and
            // then deleted the file in two separate syscalls. On any `?` failure
            // here we return Err with the in-memory state UNCHANGED (still the
            // pre-revoke Some(record)), so the gate stays fail-closed at its prior
            // verdict, and disk still holds the pre-revoke record.
            let tmp_path = self.consent_file_path().with_extension("tmp");
            if let Some(parent) = tmp_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(&revoked)?;
            // #6175: owner-only tmp before the rename so the published consent.json
            // is never world/group-readable.
            crate::secure_file::write_owner_only(&tmp_path, json.as_bytes())?;
            std::fs::rename(&tmp_path, self.consent_file_path())?;
        }
        // The write (if any) succeeded — commit the in-memory transition.
        st.current_consent = None;
        // Keep the in-memory flag true so the erasure-request signal is not lost
        // even after current_consent is set to None (GDPR Article 17).
        st.pending_deletion = true;
        // #5156: advance the per-erasure id ONLY when an ACTIVE consent was revoked
        // (a genuinely new erasure of newly-collected data). A repeat revoke with no
        // active consent collects nothing new → keep the existing pending id (and the
        // already-persisted revoked_at), so it is not mistaken for a distinct erasure.
        if revoked_active_consent {
            st.pending_erasure_at = Some(now);
            st.pending_erasure_nonce = Some(nonce);
        }
        // #4928: set the erasure-blocking flag right before erase (it must be set
        // before erase so an in-flight writer does not race the wipe). Any write
        // that enters the funnel after this point is skipped as a no-op.
        self.deletion_flag.store(true, Ordering::Release);
        Ok(())
    }

    /// Returns true when consent was previously revoked and local data is
    /// pending erasure (GDPR Article 17).  Callers should purge stored events,
    /// frames, and metrics before the next server sync when this returns true.
    ///
    /// The `pending_deletion` flag stays true even after `current_consent`
    /// becomes None following `revoke_consent()`. Once data erasure completes,
    /// `clear_pending_deletion()` must be called to reset the flag.
    pub fn has_pending_deletion(&self) -> bool {
        // Check the in-memory flag first — the erasure signal is preserved even
        // when current_consent is None after a revoke.
        let st = self.state.read();
        st.pending_deletion
            // Defensive/legacy branch: since #5156, `load_from_file` never yields a
            // `current_consent` with `data_deletion_requested == true` (a revoked
            // record returns `(None, Some(revoked_at))` and drives `pending_deletion`
            // instead), and `grant`/`revoke` keep it false/None — so this is now
            // effectively dead but kept as a harmless guard.
            || st
                .current_consent
                .as_ref()
                .map(|r| r.data_deletion_requested)
                .unwrap_or(false)
    }

    /// #5156/#5165: a restart-stable, per-erasure-distinct id for the currently
    /// pending Art. 17 erasure, or `None` when none is pending. Prefers the
    /// collision-proof per-revoke nonce (#5165); falls back to the `revoked_at`
    /// instant (rfc3339) for an in-flight erasure revoked before the nonce existed,
    /// so its restart identity / fire-once-gate match stays intact across the upgrade.
    /// Changes per revoke (a new erasure) and survives restart (recovered from the
    /// persisted revoked record in `new`). Used by the sync engine to re-propagate a
    /// new erasure + dedup a restart re-fire of the same one.
    pub fn pending_erasure_id(&self) -> Option<String> {
        let st = self.state.read();
        st.pending_erasure_nonce
            .clone()
            .or_else(|| st.pending_erasure_at.map(|at| at.to_rfc3339()))
    }

    /// Call after data erasure completes. Resets the GDPR Article 17 erasure signal.
    ///
    /// This method must be called only right after actual data erasure has
    /// completed. Calling it before erasure would drop the deletion request.
    pub fn clear_pending_deletion(&self) {
        {
            let mut st = self.state.write();
            st.pending_deletion = false;
            // #5156/#5165: drop the per-erasure id (timestamp + nonce) too, so a
            // completed erasure is not re-propagated/re-audited as if still pending.
            st.pending_erasure_at = None;
            st.pending_erasure_nonce = None;
        }
        // #4928: after erasure completes (or remote propagation completes), clear
        // the blocking flag to resume subsequent writes. Since erase is done, the
        // in-flight race window has also closed.
        self.deletion_flag.store(false, Ordering::Release);
    }

    pub fn is_permitted(&self, check: impl Fn(&ConsentPermissions) -> bool) -> bool {
        self.state
            .read()
            .current_consent
            .as_ref()
            .map(|r| check(&r.permissions))
            .unwrap_or(false)
    }

    /// Returns the canonical path of the consent file (same as `storage_path`).
    fn consent_file_path(&self) -> PathBuf {
        self.storage_path.clone()
    }

    /// Loads the persisted consent state, returning `(active_consent,
    /// pending_erasure_at, pending_erasure_nonce)`. A revoked-but-not-yet-erased
    /// record yields `(None, Some(revoked_at), record.erasure_nonce)`: no active
    /// consent, but the per-erasure id (the #5165 nonce, or the `revoked_at`
    /// timestamp for a pre-upgrade record without a nonce) + the REMOTE propagation
    /// signal survive restart (#5156). A clean granted record yields
    /// `(Some(record), None, None)`.
    fn load_from_file(
        path: &PathBuf,
    ) -> (Option<ConsentRecord>, Option<DateTime<Utc>>, Option<String>) {
        let Ok(data) = std::fs::read_to_string(path) else {
            return (None, None, None);
        };
        let record = match serde_json::from_str::<ConsentRecord>(&data) {
            Ok(record) => record,
            Err(e) => {
                // #5978: the file EXISTS but is corrupt or schema-drifted. Failing
                // closed to "no consent" is the safe default for a privacy product,
                // but the corruption must be VISIBLE in telemetry — silently
                // discarding a user's GDPR grant on a transient disk/parse fault is
                // the actual defect. (The absent-file case above stays silent: that
                // is a legitimate first-run.)
                tracing::error!(
                    err.code = "internal.serialization",
                    path = %path.display(),
                    error = %e,
                    "consent.json present but failed to parse — treating as no-consent; grant not recoverable, investigate corruption"
                );
                return (None, None, None);
            }
        };
        // When the data-deletion request is set, treat it as no consent.
        // Treat a revoked-but-not-yet-erased record as absent consent so that a
        // process restart after revoke_consent() does not re-activate consent —
        // but recover `revoked_at` + the erasure nonce so the pending erasure id is
        // not lost (#5156/#5165).
        if record.data_deletion_requested {
            return (None, record.revoked_at, record.erasure_nonce);
        }
        (Some(record), None, None)
    }

    fn save_to_file(&self, record: &ConsentRecord) -> Result<(), CoreError> {
        // Atomic write: serialize to a .tmp file, then rename into place — a crash
        // mid-write cannot leave a truncated consent.json that load_from_file would
        // reject (silently dropping a consent the user believes they granted).
        // Mirrors revoke_consent's tmp+rename.
        let tmp_path = self.storage_path.with_extension("tmp");
        if let Some(parent) = tmp_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(record)?;
        // #6175: owner-only tmp before the rename so the published consent.json is
        // never world/group-readable (it carries user consent metadata).
        crate::secure_file::write_owner_only(&tmp_path, json.as_bytes())?;
        std::fs::rename(&tmp_path, &self.storage_path)?;
        Ok(())
    }
}

/// ADR-026 PR-1: `ConsentManager` implements the object-safe
/// `ConsentManagerPort` by delegating each method to its inherent counterpart.
///
/// Purely additive — the existing concrete `Arc<ConsentManager>` consumers are
/// untouched; this only opens the `Arc<dyn ConsentManagerPort>` DI path for
/// future migration (ADR-001 §3). `is_permitted` is deliberately absent from
/// the port (it is not dyn-compatible, `E0038`) and stays inherent above.
impl crate::ports::consent_manager::ConsentManagerPort for ConsentManager {
    fn check_consent(&self) -> ConsentStatus {
        ConsentManager::check_consent(self)
    }

    fn current_consent(&self) -> Option<ConsentRecord> {
        ConsentManager::current_consent(self)
    }

    fn effective_permissions(&self) -> ConsentPermissions {
        ConsentManager::effective_permissions(self)
    }

    fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
        ConsentManager::status_and_permissions(self)
    }

    fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError> {
        ConsentManager::grant_consent(self, permissions, data_retention_days)
    }

    fn revoke_consent(&self) -> Result<(), CoreError> {
        ConsentManager::revoke_consent(self)
    }

    fn has_pending_deletion(&self) -> bool {
        ConsentManager::has_pending_deletion(self)
    }

    fn pending_erasure_id(&self) -> Option<String> {
        ConsentManager::pending_erasure_id(self)
    }

    fn clear_pending_deletion(&self) {
        ConsentManager::clear_pending_deletion(self)
    }

    fn deletion_flag(&self) -> Arc<AtomicBool> {
        ConsentManager::deletion_flag(self)
    }

    fn erasing(&self) -> Arc<AtomicBool> {
        ConsentManager::erasing(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #4684: `save_to_file` (the grant persistence path) is atomic (tmp+rename),
    /// matching `revoke_consent` — after a grant the consent file exists, no `.tmp`
    /// leftover remains, and the record reloads as Valid. Regression: the prior
    /// single-call `std::fs::write` could leave a truncated file on a mid-write crash,
    /// which `load_from_file` rejects → silently dropping a consent the user granted.
    #[test]
    fn grant_consent_writes_atomically_no_tmp_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path.clone());
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent should succeed in a writable temp dir");
        assert!(
            path.exists(),
            "consent.json must be written by save_to_file"
        );
        assert!(
            !path.with_extension("tmp").exists(),
            ".tmp must be renamed into place (atomic write), not left behind"
        );
        let reloaded = ConsentManager::new(path);
        assert_eq!(reloaded.check_consent(), ConsentStatus::Valid);
        assert!(reloaded.effective_permissions().screen_capture);
    }

    /// F-RC-C37-04: pin exact wire strings for ConsentStatus so any change to
    /// rename_all or variant names is caught immediately.
    #[test]
    fn consent_status_wire_strings_pinned() {
        let cases = [
            (ConsentStatus::NotGranted, "\"NotGranted\""),
            (ConsentStatus::Valid, "\"Valid\""),
            (ConsentStatus::Expired, "\"Expired\""),
            (ConsentStatus::UpdateRequired, "\"UpdateRequired\""),
        ];
        for (status, expected_json) in &cases {
            let json = serde_json::to_string(status).expect("serialize");
            assert_eq!(
                json, *expected_json,
                "ConsentStatus wire string mismatch for {:?}",
                status
            );
            let deser: ConsentStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                deser, *status,
                "ConsentStatus round-trip failed for {:?}",
                status
            );
        }
    }

    #[test]
    fn consent_permissions_default_all_false() {
        let perms = ConsentPermissions::default();
        assert!(!perms.screen_capture);
        assert!(!perms.telemetry);
        assert!(!perms.clipboard_monitoring);
    }

    #[test]
    fn consent_record_serde_roundtrip() {
        let record = ConsentRecord {
            consent_id: "test-001".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: ConsentRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.consent_id, "test-001");
        assert_eq!(deserialized.data_retention_days, 30);
        assert!(deserialized.revoked_at.is_none());
        assert!(!deserialized.data_deletion_requested);
    }

    #[test]
    fn consent_record_serde_legacy_compat() {
        // Records written before revoked_at / data_deletion_requested were added
        // must still deserialize correctly (both fields have #[serde(default)]).
        let legacy_json = r#"{
            "consent_id": "legacy-001",
            "version": "1.0.0",
            "granted_at": "2025-01-01T00:00:00Z",
            "expires_at": null,
            "permissions": {},
            "data_retention_days": 30
        }"#;
        let record: ConsentRecord = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(record.consent_id, "legacy-001");
        assert!(record.revoked_at.is_none());
        assert!(!record.data_deletion_requested);
    }

    #[test]
    fn consent_status_not_granted_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
    }

    #[test]
    fn consent_grant_and_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        let perms = ConsentPermissions {
            screen_capture: true,
            ..Default::default()
        };
        manager.grant_consent(perms, 30).unwrap();

        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(manager.is_permitted(|p| p.screen_capture));
        assert!(!manager.is_permitted(|p| p.clipboard_monitoring));
    }

    #[test]
    fn consent_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        let perms = ConsentPermissions::default();
        manager.grant_consent(perms, 30).unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);

        manager.revoke_consent().unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
    }

    /// #6114: a revoke whose persist (tmp+rename) FAILS must NOT fail-open.
    /// Regression for the prior bug: revoke_consent mutated the in-memory record
    /// (revoked_at / data_deletion_requested / erasure_nonce) BEFORE persisting,
    /// so a `?` failure on the write returned Err while check_consent stayed Valid
    /// and effective_permissions returned the full granted set (consent FAIL-OPEN),
    /// with the un-revoked record still on disk. After the persist-then-commit fix,
    /// a failed revoke leaves the in-memory state exactly as it was before.
    #[test]
    fn revoke_persist_failure_does_not_fail_open() {
        // Grant against a writable dir, then sabotage the path so the subsequent
        // revoke's atomic write (tmp+rename) fails via `?`. The consent file's
        // PARENT is replaced by a regular file so create_dir_all(parent) fails —
        // portable across macOS/Linux/Windows (no permission-bit or root needed).
        let live = tempfile::tempdir().unwrap();
        let consent_dir = live.path().join("consent_dir");
        std::fs::create_dir_all(&consent_dir).unwrap();
        let consent_path = consent_dir.join("consent.json");
        let mgr = ConsentManager::new(consent_path.clone());
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                clipboard_monitoring: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant must succeed in a writable dir");
        assert_eq!(mgr.check_consent(), ConsentStatus::Valid);
        assert!(mgr.effective_permissions().screen_capture);

        // Sabotage: delete the consent file + its directory, then recreate the
        // directory path component AS A FILE so the revoke's create_dir_all(parent)
        // (and thus the whole tmp+rename) fails with `?`.
        std::fs::remove_file(&consent_path).unwrap();
        std::fs::remove_dir(&consent_dir).unwrap();
        std::fs::write(&consent_dir, b"now a file").unwrap();

        // The sabotaged parent (a regular file where a directory must be) makes the
        // revoke's `create_dir_all(parent)` / tmp-write / rename fail with a
        // std::io::Error, which is wrapped into CoreError::Io. Assert that specific
        // variant so the test proves the *persist* path failed (not some unrelated
        // error) and that the io failure is surfaced rather than swallowed.
        let err = mgr
            .revoke_consent()
            .expect_err("revoke must surface the persist (tmp+rename) failure as Err");
        assert!(
            matches!(err, CoreError::Io(_)),
            "revoke's persist failure must surface as CoreError::Io, got: {err:?}"
        );

        // The whole point: state must NOT have fail-opened. The in-memory consent is
        // UNCHANGED — still Valid with the full granted permission set — because the
        // write failed before any commit.
        assert_eq!(
            mgr.check_consent(),
            ConsentStatus::Valid,
            "failed revoke must not leave a torn/Valid-but-revoked in-memory record"
        );
        let eff = mgr.effective_permissions();
        assert!(
            eff.screen_capture && eff.clipboard_monitoring,
            "failed revoke must keep the original effective permissions (no fail-open, \
             no silent permission grant/denial mismatch)"
        );
        assert!(
            !mgr.has_pending_deletion(),
            "failed revoke must not arm the GDPR Art.17 erasure signal"
        );
        assert_eq!(
            mgr.pending_erasure_id(),
            None,
            "failed revoke must not mint a pending erasure id"
        );
        // The current_consent snapshot must still carry the un-revoked record.
        let snapshot = mgr
            .current_consent()
            .expect("consent must still be present");
        assert!(
            snapshot.revoked_at.is_none() && !snapshot.data_deletion_requested,
            "in-memory record must be byte-for-byte un-revoked after a failed write"
        );
    }

    #[test]
    fn consent_expired() {
        let dir = tempfile::tempdir().unwrap();

        let record = ConsentRecord {
            consent_id: "expired-001".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        // grant_consent can't synthesize Expired (it hardcodes expires_at: None), so
        // craft the record onto disk and reconstruct the manager from it.
        let path = dir.path().join("consent_expired.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::Expired);
    }

    #[test]
    fn consent_update_required_on_version_mismatch() {
        let dir = tempfile::tempdir().unwrap();

        let record = ConsentRecord {
            consent_id: "old-001".to_string(),
            version: "0.9.0".to_string(), // previous version
            granted_at: Utc::now(),
            expires_at: None,
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        // grant_consent can't synthesize UpdateRequired (it hardcodes the current
        // version), so craft the record onto disk and reconstruct the manager.
        let path = dir.path().join("consent_version_mismatch.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager = ConsentManager::new(path);
        assert_eq!(manager.check_consent(), ConsentStatus::UpdateRequired);
    }

    /// #4631: the sub-tier consent reads (ocr / full-text / cross-device / memory-graph)
    /// now flow through `effective_permissions()`, so a non-Valid (Expired) consent denies
    /// them even though the raw record still carries the bits. Locks the defense-in-depth
    /// Valid-gate the 8 migrated call sites rely on (raw `is_permitted` would have permitted).
    #[test]
    fn effective_permissions_valid_gates_sub_tier_fields_when_expired() {
        let dir = tempfile::tempdir().unwrap();
        let record = ConsentRecord {
            consent_id: "expired-subtier".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                ocr_processing: true,
                cross_device_sync: true,
                full_text_extraction: true,
                memory_graph_enrichment: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        let path = dir.path().join("consent_expired_subtier.json");
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager = ConsentManager::new(path);
        // The manager is Expired, and the RAW (non-gated) read still sees the granted bit…
        assert_eq!(manager.check_consent(), ConsentStatus::Expired);
        assert!(
            manager.is_permitted(|p| p.ocr_processing),
            "raw is_permitted still reflects the on-disk bit (this is why the call-site swap matters)"
        );
        // …but the Valid-gated effective_permissions denies every migrated sub-tier field.
        let eff = manager.effective_permissions();
        assert!(!eff.ocr_processing, "expired consent must not permit OCR");
        assert!(
            !eff.cross_device_sync,
            "expired consent must not permit cross-device sync"
        );
        assert!(
            !eff.full_text_extraction,
            "expired consent must not permit full-text extraction"
        );
        assert!(
            !eff.memory_graph_enrichment,
            "expired consent must not permit memory-graph enrichment"
        );
    }

    #[test]
    fn has_pending_deletion_false_before_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(!manager.has_pending_deletion());
    }

    #[test]
    fn consent_revoke_records_audit_trail() {
        // After revoking consent, has_pending_deletion() must return true
        // (verifies the GDPR Article 17 erasure signal is preserved even after
        // current_consent = None).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);

        manager.revoke_consent().unwrap();
        // After revoke: no active consent.
        assert_eq!(manager.check_consent(), ConsentStatus::NotGranted);
        // The pending_deletion flag must stay true after revoke.
        assert!(manager.has_pending_deletion());
    }

    #[test]
    fn has_pending_deletion_true_after_revoke() {
        // Verifies the full lifecycle: revoke_consent() → has_pending_deletion() ==
        // true → clear_pending_deletion() → has_pending_deletion() == false.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        // Grant consent.
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !manager.has_pending_deletion(),
            "there must be no pending erasure right after granting consent"
        );

        // Revoke consent.
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "has_pending_deletion() must be true after revoke_consent() (GDPR Article 17)"
        );

        // Reset the flag after erasure completes.
        manager.clear_pending_deletion();
        assert!(
            !manager.has_pending_deletion(),
            "has_pending_deletion() must be false after clear_pending_deletion()"
        );
    }

    // ── #5156: per-erasure id (restart-stable, per-erasure-distinct) ──────────

    #[test]
    fn pending_erasure_id_none_until_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "no erasure before anything"
        );
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "a grant is not an erasure"
        );
    }

    #[test]
    fn pending_erasure_id_is_the_persisted_collision_proof_nonce() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path.clone());
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();

        let id = manager
            .pending_erasure_id()
            .expect("revoke sets a pending erasure id");
        // #5165: the id is the collision-proof per-revoke nonce (NOT wall-clock), and
        // it must equal the persisted record's `erasure_nonce` so a restart recovers an
        // identical id (the fire-once gate depends on that).
        let persisted: ConsentRecord =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(id, persisted.erasure_nonce.unwrap());
        assert!(
            id.starts_with("erasure_"),
            "the id is a ULID nonce, not a timestamp: {id}"
        );
    }

    #[test]
    fn pre_upgrade_erasure_without_nonce_falls_back_to_timestamp() {
        // Migration safety (#5165): a record revoked BEFORE the `erasure_nonce` field
        // existed (no nonce in consent.json) must keep its timestamp-based id on load,
        // so its in-flight fire-once-gate identity is unchanged across the upgrade (no
        // spurious re-announce → no unbounded peer delete of post-re-grant data).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let revoked_at = Utc::now();
        let record = ConsentRecord {
            consent_id: "consent_legacy".to_string(),
            version: "1".to_string(),
            granted_at: revoked_at,
            expires_at: None,
            revoked_at: Some(revoked_at),
            data_deletion_requested: true,
            erasure_nonce: None, // pre-upgrade record: no nonce
            permissions: ConsentPermissions::default(),
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();

        let manager = ConsentManager::new(path);
        assert_eq!(
            manager.pending_erasure_id(),
            Some(revoked_at.to_rfc3339()),
            "a pre-upgrade erasure (no nonce) keeps its timestamp id across the upgrade"
        );
    }

    #[test]
    fn pending_erasure_id_recovered_on_restart_but_durability_deferred() {
        // The per-erasure id MUST survive restart (the sync engine's stage-2
        // fire-once gate keys on it). But durability is DEFERRED (#5156 stage-1
        // guard): `has_pending_deletion()` stays false on restart so the current
        // sync gate does NOT re-announce a DeletionEvent (which the unbounded peer
        // delete would turn into post-re-grant data loss). Stage 2 re-enables
        // durability safely via a persisted, fire-once, sender-side gate.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let before = ConsentManager::new(path.clone());
        before
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        before.revoke_consent().unwrap();
        let id_before = before.pending_erasure_id();
        assert!(id_before.is_some());

        // Simulate restart: a fresh manager loads the persisted revoked file.
        let after = ConsentManager::new(path);
        assert_eq!(
            after.pending_erasure_id(),
            id_before,
            "the per-erasure id must be recovered identically on restart (for stage 2)"
        );
        assert!(
            !after.has_pending_deletion(),
            "durability deferred: the old gate must NOT re-arm on restart (no re-announce \
             → no unbounded peer delete of post-re-grant data)"
        );
        // The LOCAL write gate must NOT be re-engaged on restart either.
        assert!(
            !after.deletion_flag().load(Ordering::Acquire),
            "restart must not re-block local writes"
        );
    }

    #[test]
    fn pending_erasure_id_changes_for_a_distinct_second_erasure() {
        // revoke → re-grant (new collection window + a disk write, so time advances)
        // → revoke again. The second erasure is genuinely distinct and must get a
        // DIFFERENT id so the sync engine re-propagates it and the ledger records it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        let id1 = manager.pending_erasure_id().unwrap();

        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        let id2 = manager.pending_erasure_id().unwrap();

        assert_ne!(id1, id2, "a distinct second erasure must get a distinct id");
    }

    #[test]
    fn regrant_keeps_the_pending_erasure_id() {
        // #4630-b: re-grant does not retract the pending erasure → the id is kept
        // (no second revoke yet, so it must NOT change).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        let id1 = manager.pending_erasure_id();

        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(
            manager.pending_erasure_id(),
            id1,
            "re-grant must keep the pending erasure id (#4630-b)"
        );
    }

    #[test]
    fn revoke_without_active_consent_has_no_erasure_id() {
        // Degenerate path (no production caller — the UI requires an active consent
        // before revoke): revoking with no active consent sets the pending_deletion
        // signal but NO per-erasure id (nothing was collected → no distinct
        // erasure). Locks the documented asymmetry so a refactor can't change it
        // silently.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "revoke still arms the pending-deletion signal"
        );
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "no active consent revoked → no per-erasure id"
        );
    }

    #[test]
    fn clear_pending_deletion_resets_the_erasure_id() {
        // Post-erasure-completion: clear must drop the id so a completed erasure is
        // not re-propagated/re-audited as if still pending.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        assert!(manager.pending_erasure_id().is_some());

        manager.clear_pending_deletion();
        assert_eq!(
            manager.pending_erasure_id(),
            None,
            "clear_pending_deletion must drop the per-erasure id"
        );
    }

    /// #4630 (decision b — erasure is irrevocable): re-granting consent after a revoke
    /// does NOT retract a not-yet-propagated Art. 17 erasure. `grant_consent` leaves
    /// `pending_deletion` set, so revoke→re-grant still reports pending — the prior
    /// erasure stands (and still fires the cross-device DeletionEvent on next sync)
    /// while a fresh local collection window begins. Locks the rejection of option (a).
    #[test]
    fn pending_deletion_survives_regrant_erasure_is_irrevocable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        manager.revoke_consent().unwrap();
        assert!(
            manager.has_pending_deletion(),
            "must be pending erasure right after revoke (GDPR Article 17)"
        );

        // Re-grant BEFORE the erasure has propagated (e.g. no LAN peer online / sync off).
        manager
            .grant_consent(
                ConsentPermissions {
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        // The fresh grant is active — a new collection window opens…
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(manager.effective_permissions().screen_capture);
        // …but the prior erasure is irrevocable: still pending (decision b, not a).
        assert!(
            manager.has_pending_deletion(),
            "#4630(b): a re-grant does not retract a not-yet-propagated erasure — has_pending_deletion() stays true"
        );
    }

    #[test]
    fn consent_permissions_cross_device_sync_default_false() {
        let perms = ConsentPermissions::default();
        assert!(
            !perms.cross_device_sync,
            "cross_device_sync must default to false (GDPR Article 6)"
        );
    }

    #[test]
    fn consent_cross_device_sync_permission_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        // Without cross_device_sync
        let perms = ConsentPermissions::default();
        manager.grant_consent(perms, 30).unwrap();
        assert!(!manager.is_permitted(|p| p.cross_device_sync));

        // With cross_device_sync
        let perms_with_sync = ConsentPermissions {
            cross_device_sync: true,
            ..Default::default()
        };
        manager.grant_consent(perms_with_sync, 30).unwrap();
        assert!(manager.is_permitted(|p| p.cross_device_sync));
    }

    #[test]
    fn consent_without_full_text_extraction_deserializes() {
        let json = r#"{"screen_capture":true,"activity_pattern_learning":true}"#;
        let perms: ConsentPermissions = serde_json::from_str(json).unwrap();
        assert!(!perms.full_text_extraction);
        assert!(perms.activity_pattern_learning);
    }

    #[test]
    fn revoke_persists_file_and_new_manager_sees_not_granted() {
        // After revoke_consent(), the consent file must still exist on disk (no
        // TOCTOU delete), but a freshly constructed ConsentManager pointing at
        // the same path must report NotGranted because data_deletion_requested
        // is set in the persisted record.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path.clone());
        manager
            .grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert_eq!(manager.check_consent(), ConsentStatus::Valid);
        assert!(path.exists(), "consent file must exist after grant");

        manager.revoke_consent().unwrap();
        assert!(
            path.exists(),
            "revoke must keep the file (atomic flag, not delete)"
        );

        // Verify the on-disk record has data_deletion_requested = true.
        let raw = std::fs::read_to_string(&path).unwrap();
        let record: ConsentRecord = serde_json::from_str(&raw).unwrap();
        assert!(
            record.data_deletion_requested,
            "file must have deletion flag set"
        );

        // A new manager reading the same file must treat it as revoked.
        let new_manager = ConsentManager::new(path.clone());
        assert_eq!(
            new_manager.check_consent(),
            ConsentStatus::NotGranted,
            "new manager must see NotGranted for a revoked record"
        );
    }

    #[test]
    fn consent_permissions_legacy_json_without_cross_device_sync() {
        // Records written before cross_device_sync was added must deserialize.
        let legacy_json = r#"{
            "screen_capture": true,
            "ocr_processing": false,
            "telemetry": true,
            "process_monitoring": true,
            "input_activity": false,
            "window_title_collection": false,
            "app_usage_analytics": false,
            "clipboard_monitoring": false,
            "file_access_monitoring": false,
            "activity_pattern_learning": false
        }"#;
        let perms: ConsentPermissions = serde_json::from_str(legacy_json).unwrap();
        assert!(perms.screen_capture);
        assert!(
            !perms.cross_device_sync,
            "missing field must default to false"
        );
    }

    #[test]
    fn consent_permissions_legacy_json_without_microphone() {
        // Records written before later high-sensitivity tiers were added must
        // deserialize, defaulting each new permission to false (fail-closed).
        let legacy_json = r#"{
            "screen_capture": true,
            "ocr_processing": false,
            "telemetry": true,
            "process_monitoring": true,
            "input_activity": false,
            "window_title_collection": false,
            "app_usage_analytics": false,
            "clipboard_monitoring": false,
            "file_access_monitoring": false,
            "activity_pattern_learning": false,
            "cross_device_sync": true,
            "full_text_extraction": false,
            "memory_graph_enrichment": false
        }"#;
        let perms: ConsentPermissions = serde_json::from_str(legacy_json).unwrap();
        assert!(perms.screen_capture);
        assert!(
            !perms.microphone,
            "missing microphone field must default to false (fail-closed)"
        );
        assert!(
            !perms.unredacted_external_ocr,
            "missing unredacted_external_ocr field must default to false (fail-closed)"
        );
    }

    #[test]
    fn shared_arc_observes_writes_through_other_clone() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = Arc::new(ConsentManager::new(path));
        let clone_a = Arc::clone(&mgr);
        let clone_b = Arc::clone(&mgr);
        // Write through clone_a (now &self), observe through clone_b — proves the
        // shared immutable Arc is interior-mutable (the whole point).
        clone_a
            .grant_consent(
                ConsentPermissions {
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        assert!(clone_b.is_permitted(|p| p.screen_capture));
        assert_eq!(clone_b.check_consent(), ConsentStatus::Valid);
        clone_a.revoke_consent().unwrap();
        assert_eq!(clone_b.check_consent(), ConsentStatus::NotGranted);
        assert!(clone_b.has_pending_deletion());
    }

    #[test]
    fn effective_permissions_all_false_unless_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path.clone());
        // Absent → all false.
        assert!(!mgr.effective_permissions().screen_capture);
        // Granted (Valid) → live.
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        assert!(mgr.effective_permissions().screen_capture);

        // Expired → all false even though the boolean is true. Craft a record on disk
        // (grant_consent can't synthesize Expired: it hardcodes expires_at:None).
        let expired = ConsentRecord {
            consent_id: "exp-1".into(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(2),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr2 = ConsentManager::new(path.clone());
        assert_eq!(mgr2.check_consent(), ConsentStatus::Expired);
        assert!(
            !mgr2.effective_permissions().screen_capture,
            "Expired → effective all-false"
        );

        // UpdateRequired (version mismatch) → all false. Clear expires_at so the
        // version-mismatch branch is reached: check_consent evaluates expiry BEFORE
        // version, and `expired` above is already past its expiry (would short-
        // circuit to Expired and never test the UpdateRequired path).
        let stale = ConsentRecord {
            version: "0.0.1".into(),
            expires_at: None,
            ..expired
        };
        std::fs::write(&path, serde_json::to_string(&stale).unwrap()).unwrap();
        let mgr3 = ConsentManager::new(path);
        assert_eq!(mgr3.check_consent(), ConsentStatus::UpdateRequired);
        assert!(
            !mgr3.effective_permissions().screen_capture,
            "UpdateRequired → effective all-false"
        );
    }

    /// #4928: verifies that deletion_flag mirrors the pending_deletion lifecycle
    /// as revoke→true / grant→false / clear→false.
    #[test]
    fn deletion_flag_mirrors_revoke_grant_clear_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        let flag = mgr.deletion_flag();

        // Initial: false (writes allowed).
        assert!(
            !flag.load(Ordering::Acquire),
            "a new ConsentManager's deletion_flag must be false"
        );

        // grant: stays false.
        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !flag.load(Ordering::Acquire),
            "deletion_flag must be false after grant"
        );

        // revoke: true (set right before erase).
        mgr.revoke_consent().unwrap();
        assert!(
            flag.load(Ordering::Acquire),
            "deletion_flag must be true after revoke (#4928 erasure backstop)"
        );

        // clear_pending_deletion: false (writes resume after erasure completes).
        mgr.clear_pending_deletion();
        assert!(
            !flag.load(Ordering::Acquire),
            "deletion_flag must be false after clear_pending_deletion"
        );
    }

    /// #4928: a re-grant after revoke clears the LOCAL deletion_flag to resume
    /// writes, but the REMOTE propagation signal `pending_deletion` stays true per
    /// #4630(b) — asserts that the two signals are distinct.
    #[test]
    fn deletion_flag_clears_on_regrant_but_pending_deletion_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        let flag = mgr.deletion_flag();

        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        mgr.revoke_consent().unwrap();
        assert!(flag.load(Ordering::Acquire));
        assert!(mgr.has_pending_deletion());

        // Re-grant: the local write gate opens…
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        assert!(
            !flag.load(Ordering::Acquire),
            "deletion_flag is false after re-grant — local collection window resumes"
        );
        // …but the not-yet-propagated erasure is still alive (#4630 b).
        assert!(
            mgr.has_pending_deletion(),
            "a re-grant does not retract the not-yet-propagated erasure (pending_deletion)"
        );
    }

    /// #4928 round-3 (FIX B): none of
    /// `grant_consent`/`clear_pending_deletion`/`revoke_consent` may touch the
    /// `erasing` signal — only erase sets/clears `erasing`. So even if a re-grant
    /// slips into the erase window (`erasing=true`), `erasing` stays true and the
    /// write funnel's `deletion_flag || erasing` predicate keeps skipping writes.
    #[test]
    fn erasing_is_not_cleared_by_grant_or_clear_or_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = ConsentManager::new(path);
        let erasing = mgr.erasing();
        let deletion = mgr.deletion_flag();

        // Simulate erase having started: erasing=true (the signal the RAII guard sets).
        erasing.store(true, Ordering::Release);

        // Re-grant: deletion_flag is cleared but erasing must stay as-is.
        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        assert!(
            !deletion.load(Ordering::Acquire),
            "deletion_flag is cleared after grant"
        );
        assert!(
            erasing.load(Ordering::Acquire),
            "grant_consent cannot clear erasing (blocks the TOCTOU)"
        );

        // clear_pending_deletion does not touch erasing either.
        mgr.clear_pending_deletion();
        assert!(
            erasing.load(Ordering::Acquire),
            "clear_pending_deletion cannot clear erasing"
        );

        // revoke does not set erasing either (only erase sets it). Already true, so unchanged.
        mgr.revoke_consent().unwrap();
        assert!(
            erasing.load(Ordering::Acquire),
            "revoke does not touch erasing (value preserved)"
        );
    }

    /// #4928 round-3: a new ConsentManager's `erasing` is false, and `erasing()`
    /// returns the same Arc (the shared cell the composition root installs into
    /// SQLite/frames).
    #[test]
    fn erasing_default_false_and_shared_ptr_eq() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = ConsentManager::new(dir.path().join("consent.json"));
        let a = mgr.erasing();
        let b = mgr.erasing();
        assert!(!a.load(Ordering::Acquire), "a new erasing must be false");
        assert!(
            Arc::ptr_eq(&a, &b),
            "erasing() must return the same Arc (ptr-eq)"
        );
    }

    /// #4928: through the shared Arc, a flag set by one clone is observed by
    /// another clone (verifies deletion_flag() points to the same cell — ptr-eq).
    #[test]
    fn deletion_flag_shared_through_arc_clones() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mgr = Arc::new(ConsentManager::new(path));
        let flag_a = mgr.deletion_flag();
        let flag_b = Arc::clone(&mgr).deletion_flag();
        assert!(
            Arc::ptr_eq(&flag_a, &flag_b),
            "deletion_flag() must return the same Arc (ptr-eq)"
        );
        mgr.grant_consent(ConsentPermissions::default(), 30)
            .unwrap();
        mgr.revoke_consent().unwrap();
        assert!(
            flag_b.load(Ordering::Acquire),
            "a revoke through one handle must be reflected in the same flag held by another handle"
        );
    }

    #[test]
    fn status_and_permissions_atomic_returns_raw_not_effective() {
        // status_and_permissions() returns the status + raw permissions within a
        // single guard. Key assertion: even in a non-Valid (Expired) state it must
        // NOT zero the permissions and must return the raw granted permissions
        // as-is (in contrast to effective_permissions).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");

        // (1) no consent → (NotGranted, default-all-false).
        let mgr = ConsentManager::new(path.clone());
        let (status, perms) = mgr.status_and_permissions();
        assert_eq!(status, ConsentStatus::NotGranted);
        assert!(
            !perms.screen_capture,
            "no consent → permissions default to all-false"
        );

        // (2) Valid + granted → (Valid, granted perms).
        mgr.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        let (status, perms) = mgr.status_and_permissions();
        assert_eq!(status, ConsentStatus::Valid);
        assert!(perms.screen_capture, "Valid → live granted permissions");

        // (3) Expired record → (Expired, RAW granted perms — NOT zeroed).
        // grant_consent can't synthesize Expired (hardcodes expires_at: None), so
        // craft the record onto disk and reconstruct the manager from it.
        let expired = ConsentRecord {
            consent_id: "exp-sp-1".into(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(2),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&expired).unwrap()).unwrap();
        let mgr_expired = ConsentManager::new(path);
        let (status, perms) = mgr_expired.status_and_permissions();
        assert_eq!(status, ConsentStatus::Expired);
        assert!(
            perms.screen_capture,
            "Expired → status_and_permissions returns RAW granted perms (NOT zeroed like effective_permissions)"
        );
        // Contrast: effective_permissions zeroes on non-Valid.
        assert!(
            !mgr_expired.effective_permissions().screen_capture,
            "sanity: effective_permissions DOES zero on Expired (the distinction we preserve)"
        );
    }
}
