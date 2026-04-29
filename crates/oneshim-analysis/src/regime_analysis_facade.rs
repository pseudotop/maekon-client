//! High-level facade for regime analysis.
//!
//! Hides the `ClusteringStrategy` trait from external consumers.
//! src-tauri should use this facade instead of importing `ClusteringStrategy` directly.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use oneshim_core::config::ClusteringAlgorithm;
use oneshim_core::error::CoreError;
use oneshim_core::models::recalibration::ClusterConstraint;
use oneshim_core::models::tiered_memory::{Regime, RegimeFeatures, RegimeStatus, TriggerParams};

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

// Send + Sync because ClusteringStrategy requires Send + Sync
unsafe impl Send for RegimeAnalysisFacade {}
unsafe impl Sync for RegimeAnalysisFacade {}

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
            let centroid = if (cluster_id as usize) < result.centroids.len() {
                result.centroids[cluster_id as usize].clone()
            } else {
                compute_centroid(features, indices)
            };

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
        let result = facade.detect_regimes(&[], Utc::now());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
