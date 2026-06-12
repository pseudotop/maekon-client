//! ADR-023 AC8 invariant guard: belief revision operates on `memory_claims`/
//! `memory_edges` ONLY and must never reach the behavioral-regime lifecycle.
//!
//! This is a structural (source-grep) guard — if a future refactor wires a
//! regime handle into `belief_revision.rs`, this test fails before the change
//! can weaken the separation. Mirrors the `async_safety_app_runtime` precedent.

#[test]
fn belief_revision_holds_no_regime_reference() {
    let src = include_str!("../src/belief_revision.rs");
    for forbidden in [
        "RegimeManager",
        "RegimeStorage",
        "RegimeClassifier",
        "RegimeDetector",
        "regime_manager",
        "regime_storage",
        "regime_classifier",
        "Regime::",
    ] {
        assert!(
            !src.contains(forbidden),
            "belief_revision.rs must not reference `{forbidden}` — ADR-023 AC8 forbids \
             belief revision from touching the regime lifecycle (memory_claims/edges only)"
        );
    }
}
