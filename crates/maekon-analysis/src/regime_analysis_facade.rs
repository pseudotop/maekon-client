//! High-level facade for regime analysis.
//!
//! Hides the `ClusteringStrategy` trait from external consumers.
//! src-tauri should use this facade instead of importing `ClusteringStrategy` directly.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use maekon_core::config::ClusteringAlgorithm;
use maekon_core::error::CoreError;
use maekon_core::models::recalibration::ClusterConstraint;
use maekon_core::models::tiered_memory::{Regime, RegimeFeatures, RegimeStatus, TriggerParams};

use crate::clustering_strategy::ClusteringStrategy;
#[cfg(feature = "hdbscan")]
use crate::hdbscan_detector::HdbscanDetector;
use crate::kmeans_adapter::KmeansDetector;

/// Opaque regime analysis facade. Wraps a clustering algorithm and exposes
/// domain-level operations that return `Vec<Regime>` instead of raw
/// `ClusteringResult`.
pub struct RegimeAnalysisFacade {
    strategy: Box<dyn ClusteringStrategy>,
    algorithm_name: String,
}

impl RegimeAnalysisFacade {
    /// Create a new facade with the specified algorithm.
    pub fn new(algorithm: ClusteringAlgorithm) -> Self {
        let (strategy, name): (Box<dyn ClusteringStrategy>, &str) = match algorithm {
            ClusteringAlgorithm::Kmeans => (Box::new(KmeansDetector::new()), "kmeans"),
            ClusteringAlgorithm::Gmm => (Box::new(crate::gmm_detector::GmmDetector::new()), "gmm"),
            ClusteringAlgorithm::Hdbscan => {
                #[cfg(feature = "hdbscan")]
                {
                    (Box::new(HdbscanDetector::new(5, None)), "hdbscan")
                }
                #[cfg(not(feature = "hdbscan"))]
                {
                    tracing::warn!("HDBSCAN requested but not compiled; falling back to k-means");
                    (Box::new(KmeansDetector::new()), "kmeans-fallback")
                }
            }
        };
        Self {
            strategy,
            algorithm_name: name.to_string(),
        }
    }

    /// Detect regimes from feature vectors.
    pub fn detect_regimes(
        &self,
        features: &[RegimeFeatures],
        now: DateTime<Utc>,
    ) -> Result<Vec<Regime>, CoreError> {
        let result = self.strategy.detect(features)?;
        Ok(build_regimes(&result, features, now))
    }

    /// Re-detect with user override constraints applied.
    pub fn recluster_with_constraints(
        &self,
        features: &[RegimeFeatures],
        constraints: &[ClusterConstraint],
        now: DateTime<Utc>,
    ) -> Result<Vec<Regime>, CoreError> {
        let result = if constraints.is_empty() {
            self.strategy.detect(features)?
        } else {
            self.strategy
                .detect_with_constraints(features, constraints)?
        };
        Ok(build_regimes(&result, features, now))
    }

    /// Algorithm name for logging.
    pub fn algorithm_name(&self) -> &str {
        &self.algorithm_name
    }
}

// `RegimeAnalysisFacade` auto-derives `Send + Sync` from its fields:
// `Box<dyn ClusteringStrategy>` (the trait is declared `: Send + Sync`) and
// `String`. The previous hand-written `unsafe impl Send/Sync` was redundant and
// would have masked a future non-thread-safe field as silent UB instead of a
// compile error, so it is removed to restore the compiler's auto-trait check.
// (review4 F8)

fn build_regimes(
    result: &crate::clustering_strategy::ClusteringResult,
    features: &[RegimeFeatures],
    now: DateTime<Utc>,
) -> Vec<Regime> {
    let mut cluster_points: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, &label) in result.labels.iter().enumerate() {
        if label >= 0 {
            cluster_points.entry(label).or_default().push(i);
        }
    }

    cluster_points
        .iter()
        .map(|(&cluster_id, indices)| {
            // Recompute each regime centroid from its OWN member points instead
            // of indexing `result.centroids` by the real cluster label. The
            // detector centroid array is now stored densely (one entry per
            // present label, not `0..=max_label`), so a real label is no longer
            // a valid index into it: for non-contiguous labels (e.g. {0, 2, 5}
            // produced by ForceCluster/CannotLink) label-indexing would read a
            // different cluster's centroid or fall out of bounds. Recomputing
            // from members is layout-independent and always correct (#6120).
            let centroid = compute_centroid(features, indices);

            let auto_label = if centroid.category_coding > 0.5 {
                "Deep Work".to_string()
            } else if centroid.category_communication > 0.5 {
                "Communication".to_string()
            } else if centroid.category_browser > 0.5 {
                "Browsing".to_string()
            } else {
                format!("Regime-{}", cluster_id)
            };

            Regime {
                regime_id: format!("cluster-{}", cluster_id),
                name: None,
                auto_label,
                centroid,
                optimal_params: TriggerParams::default(),
                sample_count: indices.len() as u64,
                first_seen: now,
                last_seen: now,
                status: RegimeStatus::Active,
            }
        })
        .collect()
}

fn compute_centroid(features: &[RegimeFeatures], indices: &[usize]) -> RegimeFeatures {
    let mut sum = RegimeFeatures::default();
    for &idx in indices {
        if idx < features.len() {
            sum.category_coding += features[idx].category_coding;
            sum.category_communication += features[idx].category_communication;
            sum.category_browser += features[idx].category_browser;
            sum.avg_event_rate += features[idx].avg_event_rate;
            sum.avg_importance += features[idx].avg_importance;
            sum.context_activity_signal += features[idx].context_activity_signal;
            sum.communication_ratio += features[idx].communication_ratio;
        }
    }
    let n = indices.len() as f32;
    if n > 0.0 {
        sum.category_coding /= n;
        sum.category_communication /= n;
        sum.category_browser /= n;
        sum.avg_event_rate /= n;
        sum.avg_importance /= n;
        sum.context_activity_signal /= n;
        sum.communication_ratio /= n;
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facade_creates_kmeans_by_default() {
        let facade = RegimeAnalysisFacade::new(ClusteringAlgorithm::Kmeans);
        assert_eq!(facade.algorithm_name(), "kmeans");
    }

    #[test]
    fn detect_regimes_empty_features() {
        let facade = RegimeAnalysisFacade::new(ClusteringAlgorithm::Kmeans);
        let regimes = facade
            .detect_regimes(&[], Utc::now())
            .expect("empty feature set must not error");
        assert!(
            regimes.is_empty(),
            "no regimes can be detected from zero features"
        );
    }

    fn point(rate: f32) -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 1.0,
            category_communication: 0.0,
            category_browser: 0.0,
            avg_event_rate: rate,
            avg_importance: 0.5,
            context_activity_signal: 0.1,
            communication_ratio: 0.05,
        }
    }

    #[test]
    fn build_regimes_centroids_use_own_members_for_noncontiguous_labels() {
        // Regression for #6120: detector centroids are now stored DENSELY (one
        // entry per present label, no `0..=max_label` padding), so a real
        // cluster label is no longer a valid index into `result.centroids`.
        //
        // Labels {0, 2, 5} are non-contiguous (as produced by
        // ForceCluster/CannotLink). The dense centroid array below is laid out
        // in label-ascending order [c(label0), c(label2), c(label5)], which is
        // exactly the layout the kmeans/hdbscan/gmm detectors now emit. If
        // `build_regimes` indexed by the real label it would read index 2 (the
        // label-5 centroid) for cluster 2 and fall out of bounds for cluster 5.
        //
        // The deliberately WRONG values stored in `result.centroids` ensure the
        // assertion only passes when centroids are recomputed from each
        // regime's own members.
        let features = vec![
            point(0.10), // cluster 0
            point(0.20), // cluster 0  -> mean 0.15
            point(0.50), // cluster 2
            point(0.60), // cluster 2  -> mean 0.55
            point(0.90), // cluster 5
        ];
        let labels = vec![0, 0, 2, 2, 5];

        // Dense, label-ascending centroids that do NOT match the member means,
        // so any label-indexing path would produce a detectably wrong answer.
        let result = crate::clustering_strategy::ClusteringResult {
            labels,
            centroids: vec![point(0.0), point(0.0), point(0.0)],
            cluster_count: 3,
            noise_count: 0,
            probabilities: None,
        };

        let regimes = build_regimes(&result, &features, Utc::now());
        assert_eq!(regimes.len(), 3, "one regime per present label");

        // Look each regime up by its cluster id and assert its centroid equals
        // the mean of its OWN members (not another cluster's centroid).
        let by_id: HashMap<String, &Regime> =
            regimes.iter().map(|r| (r.regime_id.clone(), r)).collect();

        let r0 = by_id.get("cluster-0").expect("cluster-0 regime present");
        let r2 = by_id.get("cluster-2").expect("cluster-2 regime present");
        let r5 = by_id.get("cluster-5").expect("cluster-5 regime present");

        assert!((r0.centroid.avg_event_rate - 0.15).abs() < 1e-5);
        assert!((r2.centroid.avg_event_rate - 0.55).abs() < 1e-5);
        assert!((r5.centroid.avg_event_rate - 0.90).abs() < 1e-5);

        assert_eq!(r0.sample_count, 2);
        assert_eq!(r2.sample_count, 2);
        assert_eq!(r5.sample_count, 1);
    }
}
