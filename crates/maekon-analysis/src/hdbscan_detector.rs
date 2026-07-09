//! HDBSCAN-based regime detector.
//!
//! Wraps the `hdbscan` crate for density-based clustering with automatic k
//! selection and native noise detection. Custom nearest-centroid classification
//! and constraint pre/post-processing are built on top.

use std::collections::HashMap;
use std::sync::Mutex;

use maekon_core::models::recalibration::ClusterConstraint;
use maekon_core::models::tiered_memory::{euclidean_distance, RegimeFeatures};

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
    /// Cluster label for each entry in `cluster_centroids`, parallel by index.
    ///
    /// Centroids are stored densely (one per distinct non-noise label that was
    /// actually present), so the centroid array index is NOT the cluster id.
    /// This remap lets `classify()` translate a nearest-centroid array index
    /// back to the real cluster label.
    centroid_labels: Mutex<Vec<i32>>,
    /// Stored labels from the last `detect()` call.
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
            centroid_labels: Mutex::new(Vec::new()),
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
            centroid_labels: Mutex::new(Vec::new()),
            cluster_labels: Mutex::new(Vec::new()),
        }
    }

    /// Compute a DENSE centroid list (mean feature vector) keyed only on the
    /// distinct non-noise labels actually present.
    ///
    /// Returns `(centroids, centroid_labels)` where `centroid_labels[i]` is the
    /// real cluster label of `centroids[i]`. Labels are visited in ascending
    /// order, and no `0..=max_label` padding is performed, so there are no
    /// default-filled gaps for absent labels (#6120).
    fn compute_centroids(
        features: &[RegimeFeatures],
        labels: &[i32],
    ) -> (Vec<RegimeFeatures>, Vec<i32>) {
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

        // Dense centroids: one per present label, sorted ascending.
        let mut present_labels: Vec<i32> = sums.keys().copied().collect();
        present_labels.sort_unstable();

        let mut centroids = Vec::with_capacity(present_labels.len());
        for &label in &present_labels {
            // `present_labels` comes from `sums` keys, so both lookups succeed.
            let sum = &sums[&label];
            let cnt = counts[&label];
            let mut arr = [0.0f32; RegimeFeatures::DIMENSIONS];
            for (d, &s) in sum.iter().enumerate() {
                arr[d] = (s / cnt as f64) as f32;
            }
            centroids.push(RegimeFeatures::from_array(arr));
        }

        (centroids, present_labels)
    }

    /// Store centroids, the index->label remap, and labels for `classify()`.
    fn store_state(&self, centroids: &[RegimeFeatures], centroid_labels: &[i32], labels: &[i32]) {
        if let Ok(mut c) = self.cluster_centroids.lock() {
            *c = centroids.to_vec();
        }
        if let Ok(mut cl) = self.centroid_labels.lock() {
            *cl = centroid_labels.to_vec();
        }
        if let Ok(mut l) = self.cluster_labels.lock() {
            *l = labels.to_vec();
        }
    }

    /// Build a ClusteringResult from labels and features.
    ///
    /// Returns the result alongside the dense `centroid_labels` remap so the
    /// caller can persist it for `classify()`.
    fn build_result(features: &[RegimeFeatures], labels: Vec<i32>) -> (ClusteringResult, Vec<i32>) {
        let noise_count = labels.iter().filter(|&&l| l < 0).count();
        let (centroids, centroid_labels) = Self::compute_centroids(features, &labels);
        // cluster_count == centroids.len(): each dense centroid is exactly one
        // present cluster, with no phantom padding (#6120).
        let cluster_count = centroids.len();

        let result = ClusteringResult {
            labels,
            centroids,
            cluster_count,
            noise_count,
            probabilities: None, // hdbscan crate doesn't expose probabilities
        };

        (result, centroid_labels)
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

        let (result, centroid_labels) = Self::build_result(features, labels.clone());
        self.store_state(&result.centroids, &centroid_labels, &labels);

        Ok(result)
    }

    fn classify(&self, point: &RegimeFeatures) -> Option<ClusterAssignment> {
        let centroids = self.cluster_centroids.lock().ok()?;
        if centroids.is_empty() {
            return None;
        }
        let centroid_labels = self.centroid_labels.lock().ok()?;

        let mut best_idx: Option<usize> = None;
        let mut best_dist = f32::INFINITY;

        for (i, centroid) in centroids.iter().enumerate() {
            let d = euclidean_distance(point, centroid);
            if d < best_dist {
                best_dist = d;
                best_idx = Some(i);
            }
        }

        best_idx.map(|i| ClusterAssignment {
            // Map the dense centroid array index back to the real cluster label
            // through the remap; fall back to the index if the remap is missing.
            cluster_id: centroid_labels.get(i).copied().unwrap_or(i as i32),
            probability: 1.0, // nearest-centroid is hard assignment
        })
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

        let (result, centroid_labels) = Self::build_result(features, full_labels.clone());
        self.store_state(&result.centroids, &centroid_labels, &full_labels);

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

// Dense-centroid regression tests. These exercise the feature-independent
// `compute_centroids` / `build_result` helpers and therefore run regardless of
// whether the `hdbscan` feature is enabled (#6120).
#[cfg(test)]
mod dense_centroid_tests {
    use super::*;

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
    fn compute_centroids_is_dense_for_noncontiguous_labels() {
        // Non-contiguous labels {0, 2, 5} (plus a noise -1) must yield exactly
        // 3 centroids with no phantom padding for absent labels 1, 3, 4.
        let features = vec![point(0.10), point(0.20), point(0.30), point(0.40)];
        let labels = vec![0, 2, 5, -1];

        let (centroids, centroid_labels) = HdbscanDetector::compute_centroids(&features, &labels);

        assert_eq!(
            centroids.len(),
            3,
            "expected one centroid per present label"
        );
        assert_eq!(centroid_labels, vec![0, 2, 5]);
        assert!((centroids[0].avg_event_rate - 0.10).abs() < 1e-5);
        assert!((centroids[1].avg_event_rate - 0.20).abs() < 1e-5);
        assert!((centroids[2].avg_event_rate - 0.30).abs() < 1e-5);
    }

    #[test]
    fn build_result_cluster_count_matches_centroid_len() {
        // cluster_count must equal centroids.len() even for non-contiguous
        // labels (hdbscan_detector.rs:110 fix).
        let features = vec![point(0.10), point(0.20), point(0.30)];
        let labels = vec![0, 2, 5];

        let (result, centroid_labels) = HdbscanDetector::build_result(&features, labels);

        assert_eq!(result.cluster_count, result.centroids.len());
        assert_eq!(result.cluster_count, 3);
        assert_eq!(centroid_labels, vec![0, 2, 5]);
        assert_eq!(result.noise_count, 0);
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
