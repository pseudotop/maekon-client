//! Periodic regime detection and constrained re-clustering.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use oneshim_core::models::work_session::AppCategory;
use oneshim_core::types::TimeWindow;
use std::collections::HashMap;
use tracing::{debug, info, warn};

use super::super::AdaptiveTriggerState;

/// Run regime detection periodically (default: every 2h) or on demand
/// when `recluster_requested` flag is set (drift or user-triggered).
///
/// When a `RegimeAnalysisFacade` is available, constrained re-clustering is used
/// (loading user overrides from `OverrideStore`). Otherwise falls back to the
/// legacy `RegimeDetector` (k-means).
pub(in crate::scheduler) async fn run_periodic_regime_detection(
    ts: &mut AdaptiveTriggerState,
    now: DateTime<Utc>,
) {
    let on_demand = ts
        .recluster_requested
        .swap(false, std::sync::atomic::Ordering::Relaxed);

    let interval = ts.regime_detection_interval_hours;
    let should_detect = on_demand
        || ts
            .last_detection_time
            .map(|last| (now - last).num_hours() >= interval)
            .unwrap_or(true);

    if !should_detect {
        return;
    }

    ts.last_detection_time = Some(now);

    if on_demand {
        info!("on-demand re-clustering triggered");
    }

    let reader = ts.calibration_reader.clone();
    let lookback = now - ChronoDuration::days(7);
    let window = TimeWindow::new(lookback, now).expect("lookback (now - 7d) is always before now");

    match reader.get_entries(&window, true).await {
        Ok(entries) if !entries.is_empty() => {
            // Build feature vectors from calibration entries
            let features: Vec<oneshim_core::models::tiered_memory::RegimeFeatures> = entries
                .iter()
                .map(|e| {
                    let cat = &e.app_category;
                    oneshim_core::models::tiered_memory::RegimeFeatures {
                        category_coding: if *cat == AppCategory::Development {
                            1.0
                        } else {
                            0.0
                        },
                        category_communication: if *cat == AppCategory::Communication {
                            1.0
                        } else {
                            0.0
                        },
                        category_browser: if *cat == AppCategory::Browser {
                            1.0
                        } else {
                            0.0
                        },
                        avg_event_rate: e.density_signal,
                        avg_importance: e.importance_signal,
                        context_activity_signal: e.context_signal,
                        communication_ratio: if *cat == AppCategory::Communication {
                            1.0
                        } else {
                            0.0
                        },
                    }
                })
                .collect();

            // Quality gate: skip detection if insufficient feature vectors
            if features.len() < 50 {
                debug!(
                    count = features.len(),
                    "regime detection skipped — insufficient samples (need 50)"
                );
                return;
            }

            // Try constrained re-clustering via RegimeAnalysisFacade if available
            let has_strategy = ts.regime_analysis.is_some();
            if has_strategy {
                run_constrained_clustering(ts, &features, now).await;
            } else {
                // Offload heavy k-means to blocking thread to avoid stalling monitor loop
                let detector = ts.regime_detector.clone();
                let features_owned = features;
                let detected =
                    tokio::task::spawn_blocking(move || detector.detect(&features_owned))
                        .await
                        .unwrap_or_else(|e| {
                            warn!("regime detection task panicked: {e}");
                            vec![]
                        });
                if !detected.is_empty() {
                    info!(
                        count = detected.len(),
                        "regime detection completed (legacy)"
                    );
                    ts.regime_manager.lock().update_from_detection(detected);
                }
            }

            ts.regime_manager.lock().run_maintenance(now);

            // Update classifier with active regimes (clone out of the lock
            // scope so the RegimeClassifier lock is acquired separately).
            let active: Vec<_> = ts
                .regime_manager
                .lock()
                .active_regimes()
                .into_iter()
                .cloned()
                .collect();
            ts.regime_classifier.lock().update_regimes(active);

            // Reset drift detector after successful re-clustering
            ts.drift_detector.reset();
        }
        Ok(_) => {
            debug!("regime detection skipped: insufficient data");
        }
        Err(e) => {
            warn!("regime detection failure: {e}");
        }
    }
}

/// Run constrained re-clustering using `RegimeAnalysisFacade` and user overrides.
///
/// Assumes `ts.regime_analysis.is_some()` — caller must verify.
async fn run_constrained_clustering(
    ts: &mut AdaptiveTriggerState,
    features: &[oneshim_core::models::tiered_memory::RegimeFeatures],
    now: DateTime<Utc>,
) {
    use oneshim_analysis::constraint_builder;

    // Temporarily take the facade out to avoid borrow conflict
    let Some(facade) = ts.regime_analysis.take() else {
        tracing::warn!("regime_analysis missing, skipping constrained clustering");
        return;
    };

    // Load user overrides if OverrideStore is available
    let overrides = if let Some(ref store) = ts.override_store {
        let lookback = now - ChronoDuration::days(7);
        match store.list_overrides(lookback, now).await {
            Ok(ovrs) => ovrs,
            Err(e) => {
                warn!("failed to load overrides for re-clustering: {e}");
                vec![]
            }
        }
    } else {
        vec![]
    };

    // Build constraints (fast, uses async I/O)
    let constraints = if overrides.is_empty() {
        vec![]
    } else {
        let lookback = now - ChronoDuration::days(7);
        let window =
            TimeWindow::new(lookback, now).expect("lookback (now - 7d) is always before now");
        let segment_ranges = match ts
            .calibration_reader
            .list_segment_time_ranges(&window)
            .await
        {
            Ok(ranges) => ranges,
            Err(e) => {
                warn!("failed to load segment ranges for feature mapping: {e}");
                vec![]
            }
        };

        let entries_with_ts = match ts.calibration_reader.get_entries(&window, true).await {
            Ok(entries) => entries,
            Err(e) => {
                warn!("failed to re-fetch calibration entries for index mapping: {e}");
                vec![]
            }
        };

        let feature_indices: HashMap<String, usize> = segment_ranges
            .iter()
            .filter_map(|(seg_id, seg_window)| {
                entries_with_ts
                    .iter()
                    .position(|e| seg_window.contains(e.timestamp))
                    .map(|idx| (seg_id.clone(), idx))
            })
            .collect();

        let regime_cluster_map: HashMap<String, i32> = ts
            .regime_manager
            .lock()
            .active_regimes()
            .iter()
            .enumerate()
            .map(|(i, r)| (r.regime_id.clone(), i as i32))
            .collect();

        constraint_builder::build_constraints(&overrides, &feature_indices, &regime_cluster_map)
    };

    if !constraints.is_empty() {
        info!(
            count = constraints.len(),
            "applying constraints to re-clustering"
        );
    }

    // Offload heavy clustering to blocking thread to avoid stalling monitor loop
    let features_owned = features.to_vec();
    let blocking_result = tokio::task::spawn_blocking(move || {
        let r = facade.recluster_with_constraints(&features_owned, &constraints, now);
        let algo = facade.algorithm_name().to_string();
        (facade, r, algo)
    })
    .await;

    let (facade_back, result, algo_name) = match blocking_result {
        Ok(tuple) => tuple,
        Err(e) => {
            warn!("constrained clustering task panicked: {e}");
            return;
        }
    };

    // Put facade back
    ts.regime_analysis = Some(facade_back);

    match result {
        Ok(detected) if !detected.is_empty() => {
            info!(
                count = detected.len(),
                algorithm = algo_name,
                "constrained regime detection completed"
            );
            ts.regime_manager.lock().update_from_detection(detected);
        }
        Ok(_) => {
            debug!(
                algorithm = algo_name,
                "clustering produced 0 regimes — skipping update"
            );
        }
        Err(e) => {
            warn!(
                algorithm = algo_name,
                "constrained clustering failure: {e} — falling back to legacy"
            );
            // Fallback to legacy k-means via RegimeDetector
            let detector_clone = ts.regime_detector.clone();
            let features_for_fallback = features.to_vec();
            let detected =
                tokio::task::spawn_blocking(move || detector_clone.detect(&features_for_fallback))
                    .await
                    .unwrap_or_else(|e| {
                        warn!("fallback k-means task panicked: {e}");
                        vec![]
                    });
            if !detected.is_empty() {
                info!(
                    count = detected.len(),
                    "regime detection completed (fallback)"
                );
                ts.regime_manager.lock().update_from_detection(detected);
            }
        }
    }
}
