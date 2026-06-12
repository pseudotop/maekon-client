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
}

// NOTE: `impl ConsentManagerPort for ConsentManager` lives in `consent.rs`
// (next to the concrete type), per ADR-026 PR-1. The orphan rule is satisfied
// either way (both trait and type are crate-local); co-locating the impl with
// the type keeps the inherent API and its port-shim adjacent for review.

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
}
