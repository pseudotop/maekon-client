//! Resource-budget SSOT — makes the "<2% CPU, you won't notice it running"
//! product claim measurable and enforceable.
//!
//! # Why this is the single source of truth
//!
//! Before this module the only resource-budget numbers lived as test-local
//! `PROVISIONAL_*` constants inside `src-tauri/tests/memory_profile.rs`
//! (#4829). Nothing in production referenced them, so the claim was neither
//! surfaced at runtime nor guarded outside one opt-in test. This module
//! unifies those numbers so every consumer shares ONE definition (#7918):
//!
//! 1. the nightly `test_absolute_resource_budget` opt-in test — RSS/CPU
//!    ceiling assertion (`src-tauri/tests/memory_profile.rs`);
//! 2. the scheduler health-check loop — periodic self-RSS leak + budget-breach
//!    log (`src-tauri/src/scheduler/loops/resource_health.rs`); and
//! 3. the `get_resource_usage_snapshot` diagnostics IPC — local dashboard /
//!    bug-report surface (`src-tauri/src/commands/system.rs`).
//!
//! # Two tiers of numbers, and why they differ
//!
//! - `ASPIRATIONAL_*` is the *product target* the public copy commits to
//!   ("<2% CPU", "<100 MB"). It is the number the agent is designed toward.
//! - `*_BUDGET_*` is the *provisional enforcement ceiling* actually asserted
//!   in CI. It is deliberately looser than the aspirational target so a green
//!   nightly means "no gross regression / leak", NOT "hit the marketing number
//!   on this particular runner". Enforcement is intentionally generous because
//!   self-CPU/RSS on shared CI runners is noisy; tightening the ceiling toward
//!   the aspirational target is follow-up work once real fleet data informs a
//!   defensible number.
//!
//! # No egress
//!
//! No value here is ever egressed. These constants + predicates back LOCAL
//! diagnostics and CI evidence only (ADR-016 posture). Nothing in this module
//! or its callers uploads a measured RSS/CPU value.

/// Provisional RSS enforcement ceiling (bytes) — 200 MB.
///
/// Preserved verbatim from the #4829 `PROVISIONAL_RSS_BUDGET_BYTES` test
/// constant. Deliberately loose versus the 100 MB aspirational target (see the
/// module docs). Enforced by: the nightly budget test, the health-loop warning
/// path, and the diagnostics IPC.
pub const RSS_BUDGET_BYTES: u64 = 200 * 1024 * 1024;

/// Provisional CPU enforcement ceiling (percent, multi-core aggregate) — 200%.
///
/// `sysinfo` reports per-process CPU usage summed across cores, so a value
/// above 100% means more than one core-second consumed per wall-clock second.
/// 200% ≈ two fully-busy cores. Preserved verbatim from #4829's
/// `PROVISIONAL_CPU_BUDGET_PERCENT`.
pub const CPU_BUDGET_PERCENT: f32 = 200.0;

/// Aspirational RSS product target (bytes) — 100 MB.
///
/// The public-copy claim and the design goal — NOT the CI-asserted ceiling.
/// Retained here so the gap between "what we promise" and "what we currently
/// enforce" is explicit and reviewable in one place.
pub const ASPIRATIONAL_RSS_BYTES: u64 = 100 * 1024 * 1024;

/// Aspirational CPU product target (percent) — 2%.
///
/// The public-copy claim ("<2% CPU") and the design goal — NOT the CI-asserted
/// ceiling. See `ASPIRATIONAL_RSS_BYTES`.
pub const ASPIRATIONAL_CPU_PERCENT: f32 = 2.0;

/// Returns `true` when a measured RSS (bytes) is within the provisional
/// enforcement ceiling.
pub fn rss_within_budget(rss_bytes: u64) -> bool {
    rss_bytes <= RSS_BUDGET_BYTES
}

/// Returns `true` when a measured CPU usage (percent, multi-core aggregate) is
/// within the provisional enforcement ceiling.
pub fn cpu_within_budget(cpu_percent: f32) -> bool {
    cpu_percent <= CPU_BUDGET_PERCENT
}

// Compile-time invariant: the provisional enforcement ceiling must stay LOOSER
// than the aspirational product target. The whole two-tier design rests on CI
// asserting against the loose ceiling, not the tight marketing number; if these
// ever invert, the nightly gate would assert the product target on a noisy
// runner. Enforced at compile time so an inverting edit never even builds.
const _: () = assert!(RSS_BUDGET_BYTES > ASPIRATIONAL_RSS_BYTES);
const _: () = assert!(CPU_BUDGET_PERCENT > ASPIRATIONAL_CPU_PERCENT);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rss_predicate_holds_at_and_around_the_ceiling() {
        assert!(rss_within_budget(0));
        assert!(rss_within_budget(RSS_BUDGET_BYTES - 1));
        assert!(rss_within_budget(RSS_BUDGET_BYTES)); // inclusive boundary
        assert!(!rss_within_budget(RSS_BUDGET_BYTES + 1));
    }

    #[test]
    fn cpu_predicate_holds_at_and_around_the_ceiling() {
        assert!(cpu_within_budget(0.0));
        assert!(cpu_within_budget(CPU_BUDGET_PERCENT - 0.1));
        assert!(cpu_within_budget(CPU_BUDGET_PERCENT)); // inclusive boundary
        assert!(!cpu_within_budget(CPU_BUDGET_PERCENT + 0.1));
    }

    #[test]
    fn provisional_values_match_the_4829_test_constants() {
        // Regression guard: these are the numbers migrated out of
        // memory_profile.rs. If someone re-tunes the ceiling, they must do it
        // here (the SSOT), and this assertion documents the migrated values.
        assert_eq!(RSS_BUDGET_BYTES, 200 * 1024 * 1024);
        assert_eq!(CPU_BUDGET_PERCENT, 200.0);
    }
}
