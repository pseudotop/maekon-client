//! #7918: self-resource-budget sampling for the scheduler health-check loop.
//!
//! Extracted into its own helper module (like the other `loops/*_helper.rs`
//! files) so `spawn_health_check_loop` stays a thin select-loop and this
//! logic is independently unit-testable.
//!
//! Each health tick records a self-RSS/CPU snapshot on a shared
//! `MemoryTracker` (making the previously-dead `memory_profiler` module LIVE)
//! and logs a warning when the current RSS/CPU exceeds the provisional
//! resource budget, or when the RSS growth trend suggests a leak.
//!
//! LOCAL diagnostics only: nothing here egresses a measured value (ADR-016).
//! The numbers land in the local tracing log for the dashboard / bug-report
//! surface and CI evidence — never on an upload path.

use std::sync::Arc;
use std::time::Duration;

use maekon_core::resource_budget;
use tracing::{debug, info, warn};

use crate::memory_profiler::{MemoryAnalysis, MemorySnapshot, MemoryTracker};

/// Record one self-resource snapshot on the shared tracker and log budget /
/// leak signals. Called once per health tick.
///
/// The blocking `sysinfo` refresh is isolated via
/// `MemoryTracker::record_snapshot_async` (`spawn_blocking`), so this never
/// stalls a tokio worker.
async fn sample_and_log_resource_budget(
    tracker: Arc<MemoryTracker>,
    signal_state: &mut ResourceSignalState,
) {
    let Some(snapshot) = MemoryTracker::record_snapshot_async(Arc::clone(&tracker)).await else {
        // Unsupported platform or a failed measurement — nothing to assert.
        return;
    };
    let analysis = tracker.analyze();
    log_resource_signals(&snapshot, &analysis, signal_state);
}

#[derive(Debug, Default)]
struct ResourceSignalState {
    rss_over_budget: bool,
    cpu_over_budget: bool,
    /// #9635: transition tracker for the ASPIRATIONAL (product-target) band.
    /// The enforcement warn only fires at the loose provisional ceiling
    /// (200MB/200%), leaving the entire 100–200MB / 2–200% band — exactly
    /// where a regression incubates — without any local signal. This
    /// info-level breadcrumb closes that band without touching the
    /// CI-asserted ceiling.
    rss_over_aspirational: bool,
    leak_suspected: bool,
}

/// #7947: spawn the periodic self-resource-budget sampling as its OWN loop.
///
/// The #7918/#7927 sampling previously ran ONLY inside `spawn_health_check_loop`,
/// which is spawned only when the server/llm/cli health flags AND the tray app
/// handle are all `Some`. In a minimal / OSS configuration those are `None`, so
/// the periodic budget/leak logging never ran (only the on-demand diagnostics
/// IPC did). Running this as a standalone interval task decouples the sampling
/// from the health-probe wiring so it runs in EVERY configuration, without
/// touching any server/llm/cli health-probe behavior (that stays in
/// `spawn_health_check_loop`; the sampling is no longer duplicated there).
///
/// LOCAL diagnostics only — logs breaches/leaks, never egresses a value
/// (ADR-016).
pub(crate) fn spawn_resource_health_loop(
    interval: Duration,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = super::intervals::coalescing_interval(interval);
        // Owned by this task — the diagnostics IPC samples independently, so no
        // cross-task state is threaded through AppState.
        let tracker = Arc::new(MemoryTracker::new());
        let mut signal_state = ResourceSignalState::default();
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    sample_and_log_resource_budget(Arc::clone(&tracker), &mut signal_state).await;
                }
                _ = shutdown_rx.changed() => {
                    info!("resource health loop shutdown");
                    break;
                }
            }
        }
    })
}

/// Pure logging policy over an already-taken snapshot + analysis. Separated
/// from the sampling so it can be unit-tested without touching sysinfo.
fn log_resource_signals(
    snapshot: &MemorySnapshot,
    analysis: &MemoryAnalysis,
    signal_state: &mut ResourceSignalState,
) {
    let rss_mb = snapshot.rss_bytes as f64 / 1024.0 / 1024.0;
    let rss_over_budget = !resource_budget::rss_within_budget(snapshot.rss_bytes);

    if rss_over_budget && !signal_state.rss_over_budget {
        warn!(
            rss_mb,
            budget_mb = resource_budget::RSS_BUDGET_BYTES as f64 / 1024.0 / 1024.0,
            "self RSS exceeds the provisional resource budget"
        );
    } else if !rss_over_budget && signal_state.rss_over_budget {
        info!(
            rss_mb,
            "self RSS returned within the provisional resource budget"
        );
    }
    signal_state.rss_over_budget = rss_over_budget;

    // #9635: aspirational-band breadcrumb (info, transition-edged like the
    // budget warn above — one line per crossing, not per tick).
    let rss_over_aspirational = snapshot.rss_bytes > resource_budget::ASPIRATIONAL_RSS_BYTES;
    if rss_over_aspirational && !signal_state.rss_over_aspirational {
        info!(
            rss_mb,
            target_mb = resource_budget::ASPIRATIONAL_RSS_BYTES as f64 / 1024.0 / 1024.0,
            "self RSS crossed the aspirational product target (still under the enforcement ceiling)"
        );
    } else if !rss_over_aspirational && signal_state.rss_over_aspirational {
        info!(
            rss_mb,
            "self RSS returned within the aspirational product target"
        );
    }
    signal_state.rss_over_aspirational = rss_over_aspirational;

    // A 0.0 CPU reading means "no baseline yet" (first tick) or an idle
    // process — never treat it as an over-budget breach.
    let cpu_over_budget =
        snapshot.cpu_percent > 0.0 && !resource_budget::cpu_within_budget(snapshot.cpu_percent);
    if cpu_over_budget && !signal_state.cpu_over_budget {
        warn!(
            cpu_percent = snapshot.cpu_percent,
            budget_percent = resource_budget::CPU_BUDGET_PERCENT,
            "self CPU exceeds the provisional resource budget"
        );
    } else if !cpu_over_budget && signal_state.cpu_over_budget {
        info!(
            cpu_percent = snapshot.cpu_percent,
            "self CPU returned within the provisional resource budget"
        );
    }
    signal_state.cpu_over_budget = cpu_over_budget;

    if analysis.leak_suspected && !signal_state.leak_suspected {
        warn!(
            growth_kb_s = analysis.growth_rate_bytes_per_sec / 1024.0,
            "self RSS growth trend suggests a possible leak"
        );
    } else if !analysis.leak_suspected && signal_state.leak_suspected {
        info!(
            growth_kb_s = analysis.growth_rate_bytes_per_sec / 1024.0,
            "self RSS growth trend returned below the leak threshold"
        );
    }
    signal_state.leak_suspected = analysis.leak_suspected;

    if !rss_over_budget && !cpu_over_budget && !analysis.leak_suspected {
        // Healthy path stays at debug — a per-tick info line would be noisy.
        debug!(
            rss_mb,
            cpu_percent = snapshot.cpu_percent,
            "self resource sample within budget"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn snapshot(rss_bytes: u64, cpu_percent: f32) -> MemorySnapshot {
        MemorySnapshot {
            rss_bytes,
            cpu_percent,
            heap_bytes: 0,
            timestamp: Instant::now(),
        }
    }

    fn analysis(growth_rate_bytes_per_sec: f64, leak_suspected: bool) -> MemoryAnalysis {
        MemoryAnalysis {
            initial_rss: 0,
            current_rss: 0,
            peak_rss: 0,
            elapsed: Duration::from_secs(1),
            growth_rate_bytes_per_sec,
            snapshot_count: 2,
            leak_suspected,
        }
    }

    // These exercise the classification branches without asserting on emitted
    // log output — they prove the policy function accepts each shape (over-RSS,
    // over-CPU, zero-CPU baseline, leak, healthy) without panicking and that the
    // budget predicates it depends on behave as expected at each input.

    #[test]
    fn over_rss_snapshot_is_classified_over_budget() {
        let over = snapshot(resource_budget::RSS_BUDGET_BYTES + 1, 10.0);
        assert!(!resource_budget::rss_within_budget(over.rss_bytes));
        let mut state = ResourceSignalState::default();
        log_resource_signals(&over, &analysis(0.0, false), &mut state);
        assert!(state.rss_over_budget);

        log_resource_signals(&snapshot(1024, 10.0), &analysis(0.0, false), &mut state);
        assert!(!state.rss_over_budget);
    }

    #[test]
    fn zero_cpu_first_tick_is_not_a_breach() {
        // cpu_percent == 0.0 must never be flagged, even though 0.0 is trivially
        // within budget — the guard is `> 0.0` specifically to skip the
        // no-baseline first tick.
        let baseline = snapshot(1024, 0.0);
        assert!(resource_budget::cpu_within_budget(baseline.cpu_percent));
        let mut state = ResourceSignalState::default();
        log_resource_signals(&baseline, &analysis(0.0, false), &mut state);
        assert!(!state.cpu_over_budget);
    }

    #[test]
    fn over_cpu_snapshot_is_classified_over_budget() {
        let over = snapshot(1024, resource_budget::CPU_BUDGET_PERCENT + 1.0);
        assert!(!resource_budget::cpu_within_budget(over.cpu_percent));
        let mut state = ResourceSignalState::default();
        log_resource_signals(&over, &analysis(0.0, false), &mut state);
        assert!(state.cpu_over_budget);

        log_resource_signals(&snapshot(1024, 1.0), &analysis(0.0, false), &mut state);
        assert!(!state.cpu_over_budget);
    }

    #[test]
    fn leak_suspected_analysis_takes_the_warn_branch() {
        let healthy_rss = snapshot(1024, 1.0);
        let mut state = ResourceSignalState::default();
        log_resource_signals(&healthy_rss, &analysis(50_000.0, true), &mut state);
        assert!(state.leak_suspected);

        // A persistent breach stays active and is not treated as a new warning
        // transition on every five-second sample.
        log_resource_signals(&healthy_rss, &analysis(50_000.0, true), &mut state);
        assert!(state.leak_suspected);

        log_resource_signals(&healthy_rss, &analysis(0.0, false), &mut state);
        assert!(!state.leak_suspected);
    }

    #[test]
    fn healthy_snapshot_takes_the_debug_branch() {
        let healthy = snapshot(1024, 1.0);
        assert!(resource_budget::rss_within_budget(healthy.rss_bytes));
        assert!(resource_budget::cpu_within_budget(healthy.cpu_percent));
        let mut state = ResourceSignalState::default();
        log_resource_signals(&healthy, &analysis(0.0, false), &mut state);
        assert!(!state.rss_over_budget);
        assert!(!state.cpu_over_budget);
        assert!(!state.leak_suspected);
    }
}
