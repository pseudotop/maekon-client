//! HDBSCAN-based regime detector.
//!
//! Wraps the `hdbscan` crate for density-based clustering with automatic k
//! selection and native noise detection. Custom nearest-centroid classification
//! and constraint pre/post-processing are built on top.

use std::collections::HashMap;
use std::sync::Mutex;

use oneshim_core::models::recalibration::ClusterConstraint;
use oneshim_core::models::tiered_memory::{euclidean_distance, RegimeFeatures};

use crate::clustering_strategy::{
    apply_link_constraints, filter_features, parse_constraints, reconstruct_labels,
    ClusterAssignment, ClusteringResult, ClusteringStrategy,
};
use crate::error::AnalysisError;

/// HDBSCAN clustering detector for regime detection.
///
/// Uses the `hdbscan` crate for the core `cluster()` call. Classification of
/// new points and constraint handling are custom implementations.
pub struct HdbscanDetector {
    min_cluster_size: usize,
    min_samples: Option<usize>,
    /// Stored centroids from the last `detect()` call, for `classify()`.
    cluster_centroids: Mutex<Vec<RegimeFeatures>>,
    /// Stored labels from the last `detect()` call.
    #[allow(dead_code)]
    cluster_labels: Mutex<Vec<i32>>,
}

impl HdbscanDetector {
    /// Create a new detector with the given HDBSCAN parameters.
    ///
    /// - `min_cluster_size`: minimum number of points to form a cluster (default: 5)
    /// - `min_samples`: core distance neighbor count (default: None = auto)
    #[cfg(feature = "hdbscan")]
    pub fn new(min_cluster_size: usize, min_samples: Option<usize>) -> Self {
        Self {
            min_cluster_size,
            min_samples,
            cluster_centroids: Mutex::new(Vec::new()),
            cluster_labels: Mutex::new(Vec::new()),
        }
    }

    /// Stub constructor when hdbscan feature is disabled.
    #[cfg(not(feature = "hdbscan"))]
    pub fn new(_min_cluster_size: usize, _min_samples: Option<usize>) -> Self {
        Self {
            min_cluster_size: 5,
            min_samples: None,
            cluster_centroids: Mutex::new(Vec::new()),
            cluster_labels: Mutex::new(Vec::new()),
        }
    }

    /// Compute centroids (mean feature vector) per cluster label.
    fn compute_centroids(features: &[RegimeFeatures], labels: &[i32]) -> Vec<RegimeFeatures> {
        let mut sums: HashMap<i32, [f64; RegimeFeatures::DIMENSIONS]> = HashMap::new();
        let mut counts: HashMap<i32, usize> = HashMap::new();

        for (feat, &label) in features.iter().zip(labels.iter()) {
            if label < 0 {
                continue; // skip noise
            }
            let arr = feat.to_array();
            let entry = sums
                .entry(label)
                .or_insert([0.0; RegimeFeatures::DIMENSIONS]);
            for (d, &val) in arr.iter().enumerate() {
                entry[d] += val as f64;
            }
            *counts.entry(label).or_insert(0) += 1;
        }

        // Build centroids ordered by cluster ID (0, 1, 2, ...)
        let max_label = sums.keys().copied().max().unwrap_or(-1);
        let mut centroids = Vec::new();
        for label in 0..=max_label {
            if let (Some(sum), Some(&cnt)) = (sums.get(&label), counts.get(&label)) {
                let mut arr = [0.0f32; RegimeFeatures::DIMENSIONS];
                for (d, &s) in sum.iter().enumerate() {
                    arr[d] = (s / cnt as f64) as f32;
                }
                centroids.push(RegimeFeatures::from_array(arr));
            } else {
                centroids.push(RegimeFeatures::default());
            }
        }

        centroids
    }

    /// Store centroids and labels in internal state for `classify()`.
    fn store_state(&self, centroids: &[RegimeFeatures], labels: &[i32]) {
        if let Ok(mut c) = self.cluster_centroids.lock() {
            *c = centroids.to_vec();
        }
        if let Ok(mut l) = self.cluster_labels.lock() {
            *l = labels.to_vec();
        }
    }

    /// Build a ClusteringResult from labels and features.
    fn build_result(features: &[RegimeFeatures], labels: Vec<i32>) -> ClusteringResult {
        let noise_count = labels.iter().filter(|&&l| l < 0).count();
        let centroids = Self::compute_centroids(features, &labels);
        let cluster_count = centroids.len();

        ClusteringResult {
            labels,
            centroids,
            cluster_count,
            noise_count,
            probabilities: None, // hdbscan crate doesn't expose probabilities
        }
    }
}

#[cfg(feature = "hdbscan")]
impl ClusteringStrategy for HdbscanDetector {
    fn detect(&self, features: &[RegimeFeatures]) -> Result<ClusteringResult, AnalysisError> {
        if features.is_empty() {
            return Ok(ClusteringResult {
                labels: vec![],
                centroids: vec![],
                cluster_count: 0,
                noise_count: 0,
                probabilities: None,
            });
        }

        // Convert to Vec<Vec<f64>> as required by hdbscan crate
        let data: Vec<Vec<f64>> = features
            .iter()
            .map(|f| f.to_array().iter().map(|&v| v as f64).collect())
            .collect();

        // Build hyperparams
        let mut builder =
            hdbscan::HdbscanHyperParams::builder().min_cluster_size(self.min_cluster_size);

        if let Some(ms) = self.min_samples {
            builder = builder.min_samples(ms);
        }

        let params = builder.build();
        let clusterer = hdbscan::Hdbscan::new(&data, params);

        let labels = clusterer
            .cluster()
            .map_err(|e| AnalysisError::Clustering(format!("HDBSCAN clustering failed: {e:?}")))?;

        let result = Self::build_result(features, labels.clone());
        self.store_state(&result.centroids, &labels);

        Ok(result)
    }

    fn classify(&self, point: &RegimeFeatures) -> Option<ClusterAssignment> {
        let centroids = self.cluster_centroids.lock().ok()?;
        if centroids.is_empty() {
            return None;
        }

        let mut best_id = -1i32;
        let mut best_dist = f32::INFINITY;

        for (i, centroid) in centroids.iter().enumerate() {
            let d = euclidean_distance(point, centroid);
            if d < best_dist {
                best_dist = d;
                best_id = i as i32;
            }
        }

        if best_id >= 0 {
            Some(ClusterAssignment {
                cluster_id: best_id,
                probability: 1.0, // nearest-centroid is hard assignment
            })
        } else {
            None
        }
    }

    fn detect_with_constraints(
        &self,
        features: &[RegimeFeatures],
        constraints: &[ClusterConstraint],
    ) -> Result<ClusteringResult, AnalysisError> {
        if features.is_empty() {
            return Ok(ClusteringResult {
                labels: vec![],
                centroids: vec![],
                cluster_count: 0,
                noise_count: 0,
                probabilities: None,
            });
        }

        let parsed = parse_constraints(constraints, self.algorithm_name());
        let (filtered_features, original_indices) =
            filter_features(features, &parsed.noise_indices);

        // Run clustering on filtered data
        let sub_result = if filtered_features.is_empty() {
            ClusteringResult {
                labels: vec![],
                centroids: vec![],
                cluster_count: 0,
                noise_count: 0,
                probabilities: None,
            }
        } else {
            self.detect(&filtered_features)?
        };

        let mut full_labels = reconstruct_labels(
            features.len(),
            &sub_result.labels,
            &original_indices,
            &parsed.force_clusters,
        );

        // Apply MustLink / CannotLink post-processing
        apply_link_constraints(features, &mut full_labels, &parsed);

        let result = Self::build_result(features, full_labels.clone());
        self.store_state(&result.centroids, &full_labels);

        Ok(result)
    }

    fn algorithm_name(&self) -> &str {
        "hdbscan"
    }
}

#[cfg(not(feature = "hdbscan"))]
impl ClusteringStrategy for HdbscanDetector {
    fn detect(&self, _features: &[RegimeFeatures]) -> Result<ClusteringResult, AnalysisError> {
        Err(AnalysisError::Clustering(
            "HDBSCAN feature is not enabled. Use k-means fallback.".to_string(),
        ))
    }

    fn classify(&self, _point: &RegimeFeatures) -> Option<ClusterAssignment> {
        None
    }

    fn detect_with_constraints(
        &self,
        _features: &[RegimeFeatures],
        _constraints: &[ClusterConstraint],
    ) -> Result<ClusteringResult, AnalysisError> {
        Err(AnalysisError::Clustering(
            "HDBSCAN feature is not enabled. Use k-means fallback.".to_string(),
        ))
    }

    fn algorithm_name(&self) -> &str {
        "hdbscan (disabled)"
    }
}

#[cfg(test)]
#[cfg(feature = "hdbscan")]
mod tests {
    use super::*;

    fn coding_point(rate: f32, importance: f32) -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 1.0,
            category_communication: 0.0,
            category_browser: 0.0,
            avg_event_rate: rate,
            avg_importance: importance,
            context_activity_signal: 0.1,
            communication_ratio: 0.05,
        }
    }

    fn comm_point(rate: f32, importance: f32) -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 0.0,
            category_communication: 1.0,
            category_browser: 0.0,
            avg_event_rate: rate,
            avg_importance: importance,
            context_activity_signal: 0.4,
            communication_ratio: 0.8,
        }
    }

    #[test]
    fn detect_produces_clusters_from_well_separated_data() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        // Cluster A: coding
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        // Cluster B: communication
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let result = detector.detect(&features).unwrap();

        // Should find at least 2 clusters
        assert!(
            result.cluster_count >= 2,
            "expected >= 2 clusters, got {}",
            result.cluster_count
        );
        assert_eq!(result.labels.len(), 60);
    }

    #[test]
    fn classify_matches_nearest_centroid() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let _result = detector.detect(&features).unwrap();

        // A coding point should classify to a cluster
        let assignment = detector.classify(&coding_point(0.35, 0.85));
        assert!(assignment.is_some());
    }

    #[test]
    fn noise_points_labeled_negative() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let result = detector.detect(&features).unwrap();

        // Noise labels are -1; non-noise are >= 0
        for &label in &result.labels {
            assert!(label >= -1);
        }
        // noise_count should match the -1 labels
        let counted = result.labels.iter().filter(|&&l| l < 0).count();
        assert_eq!(result.noise_count, counted);
    }

    #[test]
    fn constraints_noise_label_applied() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let constraints = vec![
            ClusterConstraint::NoiseLabel(0),
            ClusterConstraint::NoiseLabel(1),
        ];

        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        // Points 0 and 1 should be noise
        assert_eq!(result.labels[0], -1);
        assert_eq!(result.labels[1], -1);
    }

    #[test]
    fn constraints_force_cluster_applied() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let constraints = vec![ClusterConstraint::ForceCluster(0, 99)];

        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        // Point 0 should be forced to cluster 99
        assert_eq!(result.labels[0], 99);
    }

    #[test]
    fn empty_features_returns_empty_result() {
        let detector = HdbscanDetector::new(5, None);
        let result = detector.detect(&[]).unwrap();
        assert_eq!(result.cluster_count, 0);
        assert_eq!(result.noise_count, 0);
        assert!(result.labels.is_empty());
    }

    #[test]
    fn algorithm_name_returns_hdbscan() {
        let detector = HdbscanDetector::new(5, None);
        assert_eq!(detector.algorithm_name(), "hdbscan");
    }

    #[test]
    fn must_link_constraint_merges_clusters() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        // MustLink forces coding and comm points into same cluster
        let constraints = vec![ClusterConstraint::MustLink(0, 30)];
        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        // Points 0 and 30 must be in the same cluster after merge
        assert_eq!(result.labels[0], result.labels[30]);
    }

    #[test]
    fn cannot_link_constraint_splits_cluster() {
        let detector = HdbscanDetector::new(5, None);

        let mut features = Vec::new();
        for i in 0..15 {
            features.push(coding_point(0.2 + (i as f32) * 0.005, 0.9));
        }
        for i in 0..15 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.3));
        }

        // CannotLink on two points in the same cluster
        let constraints = vec![ClusterConstraint::CannotLink(0, 1)];
        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        // Points 0 and 1 must be in different clusters
        assert_ne!(result.labels[0], result.labels[1]);
    }
}
