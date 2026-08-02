//! Object-safe (dyn-compatible) consent authority port (ADR-026 Decision 1).
//!
//! `ConsentManager` (kept in `maekon-core` per ADR-021 — consent state is core
//! product policy, deliberately NOT moved to an adapter) implements this port.
//! The trait is **synchronous**: `ConsentManager` is pure in-memory
//! `parking_lot::RwLock` state + sync local JSON file I/O with no `.await`
//! anywhere, and ADR-021 forbids it from growing async external side effects.
//! ADR-001 §2's `#[async_trait]` rule targets I/O-bound ports; a pure consent
//! **policy** authority is correctly a sync port.
//!
//! Object-safety is mandatory for the `Arc<dyn ConsentManagerPort>` DI pattern
//! (ADR-001 §3). The prior spec's `is_permitted(&self, check: impl Fn(...))`
//! method is intentionally NOT on this trait — a generic type parameter on a
//! trait method is not dyn-compatible (`E0038`), so it would break the vtable.
//! `is_permitted` stays an inherent method on `ConsentManager` (test-only;
//! production gating goes through `effective_permissions()`). Callers that need
//! "is permission X granted" inspect the `ConsentPermissions` snapshot returned
//! by `effective_permissions()`, or use the non-generic `*_permitted()` helpers.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::consent::{ConsentPermissions, ConsentRecord, ConsentStatus};
use crate::error::CoreError;

/// Object-safe (dyn-compatible) consent authority port.
///
/// Implemented by `ConsentManager` (kept in `maekon-core` per ADR-021). Every
/// method mirrors the corresponding inherent method on `ConsentManager`.
pub trait ConsentManagerPort: Send + Sync {
    /// Current consent status (NotGranted / Valid / Expired / UpdateRequired).
    fn check_consent(&self) -> ConsentStatus;

    /// Owned snapshot of the current consent record (read under a read guard).
    fn current_consent(&self) -> Option<ConsentRecord>;

    /// Fail-closed: returns permissions ONLY when consent is currently `Valid`,
    /// otherwise `ConsentPermissions::default()` (all false). This is the
    /// canonical gating accessor — use it instead of the removed generic
    /// `is_permitted`.
    fn effective_permissions(&self) -> ConsentPermissions;

    /// Atomic (status, raw-permissions) snapshot for UI. NOT fail-closed-gated:
    /// in a non-`Valid` state it returns the RAW granted permissions (so the UI
    /// can show "what was granted" alongside the status), never the zeroed set.
    /// Gate decisions must use `effective_permissions`, never this.
    fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions);

    /// Grant consent with the given permission set and retention window.
    ///
    /// # Errors
    /// Returns `CoreError` if persisting the consent record to disk fails.
    fn grant_consent(
        &self,
        permissions: ConsentPermissions,
        data_retention_days: u32,
    ) -> Result<(), CoreError>;

    /// Revoke consent (GDPR Article 7 §3). Sets the pending-deletion signal and
    /// the #4928 erase-barrier `deletion_flag`.
    ///
    /// # Errors
    /// Returns `CoreError` if persisting the revoked record to disk fails.
    fn revoke_consent(&self) -> Result<(), CoreError>;

    /// True when consent was revoked and local data is pending erasure
    /// (GDPR Article 17).
    fn has_pending_deletion(&self) -> bool;

    /// A restart-stable, per-erasure-distinct identity for the currently pending
    /// Art. 17 erasure (the `revoked_at` instant as rfc3339), or `None` when none
    /// is pending. Changes per revoke and survives restart (recovered from the
    /// persisted revoked record). Used to (a) re-propagate a genuinely NEW erasure
    /// and (b) dedup a restart re-fire of the SAME erasure in the egress audit
    /// ledger (#5156).
    fn pending_erasure_id(&self) -> Option<String>;

    /// Clear the pending-deletion signal after local erasure has completed.
    fn clear_pending_deletion(&self);

    /// #4928 erasure-barrier signal: the shared `deletion_flag` `Arc` installed
    /// into the storage adapters (ptr-eq). revoke → `true`, grant/clear →
    /// `false`.
    fn deletion_flag(&self) -> Arc<AtomicBool>;

    /// #4928 round-3 erase-window signal: the shared `erasing` `Arc` installed
    /// into the storage adapters (ptr-eq). Only the erase path set/clears it;
    /// grant/clear/revoke never touch it (TOCTOU backstop).
    fn erasing(&self) -> Arc<AtomicBool>;

    /// Convenience: non-generic, object-safe replacement for the common
    /// `is_permitted(|p| p.telemetry)` idiom. Default-implemented over
    /// `effective_permissions()` so impls get it for free; it stays in the
    /// vtable (no generic params).
    fn telemetry_permitted(&self) -> bool {
        self.effective_permissions().telemetry
    }

    /// Convenience: non-generic, object-safe replacement for the common
    /// `is_permitted(|p| p.screen_capture)` idiom. Default-implemented over
    /// `effective_permissions()`.
    fn screen_capture_permitted(&self) -> bool {
        self.effective_permissions().screen_capture
    }

    /// Convenience: `effective_permissions().full_text_extraction`, for callers
    /// that already hold a guaranteed-present `dyn ConsentManagerPort` (no
    /// `Option` to default — see [`ConsentGate`] for the `Option`-defaulting
    /// case). Used by the external-LLM/OCR probe egress paths (#7728).
    fn full_text_extraction_permitted(&self) -> bool {
        self.effective_permissions().full_text_extraction
    }

    /// Convenience: `effective_permissions().ocr_processing`.
    fn ocr_processing_permitted(&self) -> bool {
        self.effective_permissions().ocr_processing
    }

    /// Convenience: `effective_permissions().cross_device_sync`.
    fn cross_device_sync_permitted(&self) -> bool {
        self.effective_permissions().cross_device_sync
    }

    /// Convenience: `effective_permissions().microphone`.
    fn microphone_permitted(&self) -> bool {
        self.effective_permissions().microphone
    }

    /// Convenience: `effective_permissions().memory_graph_enrichment`.
    fn memory_graph_enrichment_permitted(&self) -> bool {
        self.effective_permissions().memory_graph_enrichment
    }

    /// Convenience: `effective_permissions().activity_pattern_learning`.
    fn activity_pattern_learning_permitted(&self) -> bool {
        self.effective_permissions().activity_pattern_learning
    }

    /// Convenience: `effective_permissions().memory_graph_retrieval_ranking`
    /// (ADR-032 Mode A — Tier 10).
    fn memory_graph_retrieval_ranking_permitted(&self) -> bool {
        self.effective_permissions().memory_graph_retrieval_ranking
    }

    /// Convenience: `effective_permissions().memory_vault_mirror`
    /// (ADR-033 — Tier 13).
    fn memory_vault_mirror_permitted(&self) -> bool {
        self.effective_permissions().memory_vault_mirror
    }
}

// NOTE: `impl ConsentManagerPort for ConsentManager` lives in `consent.rs`
// (next to the concrete type), per ADR-026 PR-1. The orphan rule is satisfied
// either way (both trait and type are crate-local); co-locating the impl with
// the type keeps the inherent API and its port-shim adjacent for review.

/// Fail-closed policy gate over an OPTIONALLY-present `ConsentManagerPort`
/// (#7728 — ctd-W2 E7).
///
/// `ConsentManagerPort::effective_permissions()` already answers "what is
/// granted while a manager IS present" in one fail-closed way (all-false
/// unless `Valid`). What it cannot answer is the question every consumer that
/// holds `Option<Arc<dyn ConsentManagerPort>>` (rather than a guaranteed
/// instance) actually faces: **"what happens when there is no manager at
/// all?"** — e.g. before the consent subsystem finishes wiring, or in a
/// stripped-down test/tool build.
///
/// Before this type, 17 non-test src-tauri call sites answered that question
/// by hand, and answered it THREE different ways for the telemetry
/// permission alone: `scheduler/config.rs` defaulted OPEN (`is_none_or` →
/// `true`, the actual bug), `integration_runtime.rs` defaulted CLOSED, and
/// several other sites defaulted to an all-false `ConsentPermissions`
/// snapshot. `ConsentGate` gives every call site the SAME, single, tested
/// answer per permission tier — and per ADR-021/ADR-026 (ONESHIM is a privacy
/// product), that answer is **always fail-closed**: a missing manager permits
/// nothing.
///
/// `src-tauri` code must reach `ConsentManagerPort::effective_permissions()`
/// ONLY through this type (or, for a call site that already holds a
/// guaranteed-present manager, through the `*_permitted()` default methods
/// above) — enforced by the `maekon-lint` composition gate
/// (`no_raw_effective_permissions_composition_in_src_tauri`).
#[derive(Clone, Default)]
pub struct ConsentGate {
    manager: Option<Arc<dyn ConsentManagerPort>>,
}

impl ConsentGate {
    /// Builds a gate from an owned, optionally-present manager.
    pub fn new(manager: Option<Arc<dyn ConsentManagerPort>>) -> Self {
        Self { manager }
    }

    /// Builds a gate from a borrowed `Option<&Arc<dyn ConsentManagerPort>>` —
    /// the shape every existing `consent_manager.as_ref()` call site already
    /// produces. Clones the `Arc` (a cheap atomic refcount bump), not the
    /// underlying manager.
    pub fn from_ref(manager: Option<&Arc<dyn ConsentManagerPort>>) -> Self {
        Self {
            manager: manager.cloned(),
        }
    }

    /// A gate with no manager installed — always fail-closed. Mirrors
    /// `ConsentGate::default()`; named for readability at test/diagnostic call
    /// sites.
    pub fn closed() -> Self {
        Self { manager: None }
    }

    /// The full fail-closed permission snapshot: `ConsentPermissions::default()`
    /// (all-false) when no manager is installed, otherwise the manager's own
    /// fail-closed `effective_permissions()` (all-false unless `Valid`).
    ///
    /// Prefer a named `may_*` query below when gating a single permission —
    /// this exists for the composite gates (`capture_permitted_now` and
    /// siblings) that need the whole struct.
    pub fn permissions_snapshot(&self) -> ConsentPermissions {
        self.manager
            .as_ref()
            .map(|m| m.effective_permissions())
            .unwrap_or_default()
    }

    /// Fail-closed telemetry-upload gate (#7728 — the site that used to
    /// default OPEN in `scheduler/config.rs`).
    pub fn may_upload_telemetry(&self) -> bool {
        self.permissions_snapshot().telemetry
    }

    /// Fail-closed full-text-extraction gate (external LLM / accessibility
    /// text egress).
    pub fn may_extract_full_text(&self) -> bool {
        self.permissions_snapshot().full_text_extraction
    }

    /// Fail-closed OCR-processing gate.
    pub fn may_process_ocr(&self) -> bool {
        self.permissions_snapshot().ocr_processing
    }

    /// Fail-closed cross-device-sync gate.
    pub fn may_sync_cross_device(&self) -> bool {
        self.permissions_snapshot().cross_device_sync
    }

    /// Fail-closed microphone-capture gate.
    pub fn may_capture_microphone(&self) -> bool {
        self.permissions_snapshot().microphone
    }

    /// Fail-closed memory-graph-enrichment gate (ADR-023 Phase-2).
    pub fn may_enrich_memory_graph(&self) -> bool {
        self.permissions_snapshot().memory_graph_enrichment
    }

    /// Fail-closed activity-pattern-learning gate (Tier 4 tiered memory).
    pub fn may_learn_activity_pattern(&self) -> bool {
        self.permissions_snapshot().activity_pattern_learning
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consent::ConsentManager;

    /// `Arc<ConsentManager>` coerces to `Arc<dyn ConsentManagerPort>` (proves
    /// object-safety / dyn-compatibility — the whole point of dropping the
    /// generic `is_permitted` per ADR-026) AND a method is callable through the
    /// trait object.
    #[test]
    fn arc_consent_manager_coerces_to_dyn_port_and_is_callable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let port: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(path));

        // Callable through the trait object (vtable dispatch).
        assert_eq!(port.check_consent(), ConsentStatus::NotGranted);

        // Mutating method through the trait object.
        port.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent should succeed in a writable temp dir");
        assert_eq!(port.check_consent(), ConsentStatus::Valid);
        assert!(
            port.telemetry_permitted(),
            "telemetry_permitted default helper must read the granted bit through the vtable"
        );
        assert!(
            !port.screen_capture_permitted(),
            "screen_capture was not granted → default helper must report false"
        );
    }

    /// `effective_permissions()` through the trait object is fail-closed: all
    /// false unless consent is currently `Valid`.
    #[test]
    fn effective_permissions_fail_closed_through_trait_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let port: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(path));

        // Absent consent → every field false.
        let eff = port.effective_permissions();
        assert!(!eff.screen_capture);
        assert!(!eff.telemetry);
        assert!(!eff.ocr_processing);
        assert!(!eff.cross_device_sync);
        assert!(!eff.microphone);

        // Granted (Valid) → the granted bit is live.
        port.grant_consent(
            ConsentPermissions {
                screen_capture: true,
                ..Default::default()
            },
            30,
        )
        .unwrap();
        assert!(port.effective_permissions().screen_capture);

        // Revoked → fail-closed again (back to all-false).
        port.revoke_consent().unwrap();
        assert!(
            !port.effective_permissions().screen_capture,
            "revoked consent must fail-closed to all-false through the trait object"
        );
    }

    // ── ConsentGate (#7728) ─────────────────────────────────────────────────

    fn granted_manager(permissions: ConsentPermissions) -> Arc<dyn ConsentManagerPort> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);
        manager.grant_consent(permissions, 30).unwrap();
        Arc::new(manager)
    }

    /// This is THE regression test for #7728: with no manager installed at
    /// all, EVERY named query — including telemetry, the field that used to
    /// default OPEN in `scheduler/config.rs`'s hand-composed `is_none_or` —
    /// must answer `false`. Before the fix this would have been `true` for
    /// telemetry alone, diverging from every other permission tier.
    #[test]
    fn consent_gate_with_no_manager_denies_every_named_query() {
        let gate = ConsentGate::new(None);
        assert!(!gate.may_upload_telemetry());
        assert!(!gate.may_extract_full_text());
        assert!(!gate.may_process_ocr());
        assert!(!gate.may_sync_cross_device());
        assert!(!gate.may_capture_microphone());
        assert!(!gate.may_enrich_memory_graph());
        assert!(!gate.may_learn_activity_pattern());

        // The full snapshot is the all-false default, never a partially-open
        // one. `ConsentPermissions` has no `PartialEq`, so compare via the
        // gate's own named queries plus a spot-check field read.
        let snapshot = gate.permissions_snapshot();
        assert!(!snapshot.telemetry);
        assert!(!snapshot.screen_capture);
        assert!(!snapshot.window_title_collection);

        // `ConsentGate::closed()` and `ConsentGate::default()` are equivalent
        // spellings of the same no-manager state.
        assert!(!ConsentGate::closed().may_upload_telemetry());
        assert!(!ConsentGate::default().may_upload_telemetry());
    }

    #[test]
    fn consent_gate_from_ref_reads_a_granted_manager() {
        let manager = granted_manager(ConsentPermissions {
            telemetry: true,
            full_text_extraction: true,
            ocr_processing: true,
            cross_device_sync: true,
            microphone: true,
            memory_graph_enrichment: true,
            activity_pattern_learning: true,
            ..Default::default()
        });
        let owned: Option<Arc<dyn ConsentManagerPort>> = Some(manager);
        let gate = ConsentGate::from_ref(owned.as_ref());

        assert!(gate.may_upload_telemetry());
        assert!(gate.may_extract_full_text());
        assert!(gate.may_process_ocr());
        assert!(gate.may_sync_cross_device());
        assert!(gate.may_capture_microphone());
        assert!(gate.may_enrich_memory_graph());
        assert!(gate.may_learn_activity_pattern());
    }

    /// A granted-but-Expired record must still fail closed through the gate —
    /// mirrors `effective_permissions_valid_gates_sub_tier_fields_when_expired`
    /// in `consent.rs`, but through `ConsentGate` specifically.
    #[test]
    fn consent_gate_fails_closed_on_expired_consent() {
        use crate::consent::{ConsentRecord, CURRENT_POLICY_VERSION};
        use chrono::Utc;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let record = ConsentRecord {
            consent_id: "expired".to_string(),
            version: CURRENT_POLICY_VERSION.to_string(),
            granted_at: Utc::now() - chrono::Duration::days(365),
            expires_at: Some(Utc::now() - chrono::Duration::days(1)),
            revoked_at: None,
            data_deletion_requested: false,
            erasure_nonce: None,
            permissions: ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            data_retention_days: 30,
        };
        std::fs::write(&path, serde_json::to_string(&record).unwrap()).unwrap();
        let manager: Arc<dyn ConsentManagerPort> = Arc::new(ConsentManager::new(path));

        let gate = ConsentGate::new(Some(manager));
        assert!(
            !gate.may_upload_telemetry(),
            "expired consent must fail-closed through ConsentGate, not just through effective_permissions() directly"
        );
    }

    /// The new `*_permitted()` default methods added alongside `ConsentGate`
    /// mirror the pre-existing `telemetry_permitted`/`screen_capture_permitted`
    /// pattern for callers that already hold a guaranteed-present manager.
    #[test]
    fn new_permitted_default_methods_read_through_effective_permissions() {
        let manager = granted_manager(ConsentPermissions {
            full_text_extraction: true,
            ocr_processing: true,
            cross_device_sync: true,
            microphone: true,
            memory_graph_enrichment: true,
            activity_pattern_learning: true,
            ..Default::default()
        });
        assert!(manager.full_text_extraction_permitted());
        assert!(manager.ocr_processing_permitted());
        assert!(manager.cross_device_sync_permitted());
        assert!(manager.microphone_permitted());
        assert!(manager.memory_graph_enrichment_permitted());
        assert!(manager.activity_pattern_learning_permitted());
    }
}
