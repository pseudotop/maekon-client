//! K-means adapter implementing `ClusteringStrategy`.
//!
//! Wraps the existing hand-rolled `RegimeDetector` to provide a unified
//! clustering interface alongside `HdbscanDetector`.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use maekon_core::models::recalibration::ClusterConstraint;
use maekon_core::models::tiered_memory::{euclidean_distance, RegimeFeatures};

use crate::clustering_strategy::{
    apply_link_constraints, filter_features, parse_constraints, reconstruct_labels,
    ClusterAssignment, ClusteringResult, ClusteringStrategy,
};
use crate::error::AnalysisError;
use crate::regime_detector::RegimeDetector;

/// K-means clustering adapter for `ClusteringStrategy`.
///
/// Delegates to the existing `RegimeDetector` (hand-rolled k-means) and
/// converts its output to the unified `ClusteringResult` format.
pub struct KmeansDetector {
    max_k: usize,
    #[allow(dead_code)]
    max_iterations: usize,
    min_cluster_samples: u64,
    /// Stored centroids from the last `detect()` call, for `classify()`.
    centroids: Mutex<Vec<RegimeFeatures>>,
    /// Cluster label for each stored centroid, parallel by index.
    ///
    /// Centroids are stored densely (one per distinct non-noise label that was
    /// actually present), so the array index is not necessarily the cluster id
    /// after constraint-driven relabeling. This remap lets `classify()`
    /// translate a nearest-centroid array index back to the real cluster label.
    centroid_labels: Mutex<Vec<i32>>,
}

impl KmeansDetector {
    /// Create a new k-means detector with default settings.
    pub fn new() -> Self {
        Self {
            max_k: 7,
            max_iterations: 50,
            min_cluster_samples: 50,
            centroids: Mutex::new(Vec::new()),
            centroid_labels: Mutex::new(Vec::new()),
        }
    }

    /// Override the maximum number of clusters.
    pub fn with_max_k(mut self, max_k: usize) -> Self {
        self.max_k = max_k;
        self
    }

    /// Override the minimum sample count.
    pub fn with_min_samples(mut self, min: u64) -> Self {
        self.min_cluster_samples = min;
        self
    }

    /// Build the internal RegimeDetector with current settings.
    fn build_detector(&self) -> RegimeDetector {
        RegimeDetector::new()
            .with_max_k(self.max_k)
            .with_min_samples(self.min_cluster_samples as usize)
    }

    /// Convert RegimeDetector output (Vec<Regime>) to ClusteringResult.
    fn regimes_to_result(
        &self,
        features: &[RegimeFeatures],
        detector: &RegimeDetector,
    ) -> ClusteringResult {
        let regimes = detector.detect(features);

        if regimes.is_empty() {
            return ClusteringResult {
                labels: vec![0; features.len()],
                centroids: vec![],
                cluster_count: 0,
                noise_count: 0,
                probabilities: None,
            };
        }

        let centroids: Vec<RegimeFeatures> = regimes.iter().map(|r| r.centroid.clone()).collect();

        // Assign each point to the nearest centroid
        let labels: Vec<i32> = features
            .iter()
            .map(|f| {
                let mut best_id = 0i32;
                let mut best_dist = f32::INFINITY;
                for (i, c) in centroids.iter().enumerate() {
                    let d = euclidean_distance(f, c);
                    if d < best_dist {
                        best_dist = d;
                        best_id = i as i32;
                    }
                }
                best_id
            })
            .collect();

        // Store centroids for classify(). In the unconstrained path the label
        // assigned to each point is the centroid array index, so the remap is
        // the identity 0..centroids.len().
        if let Ok(mut stored) = self.centroids.lock() {
            *stored = centroids.clone();
        }
        if let Ok(mut stored_labels) = self.centroid_labels.lock() {
            *stored_labels = (0..centroids.len() as i32).collect();
        }

        ClusteringResult {
            labels,
            centroids,
            cluster_count: regimes.len(),
            noise_count: 0,      // k-means has no noise concept
            probabilities: None, // k-means is hard assignment
        }
    }
}

/// Recompute a DENSE centroid list from labels after link-constraint
/// modifications.
///
/// Returns `(centroids, centroid_labels)` where `centroid_labels[i]` is the
/// real cluster label of `centroids[i]`. Only distinct non-noise labels that
/// are actually present produce a centroid (sorted ascending); there is no
/// `0..=max_label` padding, so the array has no default-filled gaps and its
/// length equals the real cluster count (#6120).
fn recompute_centroids_from_labels(
    features: &[RegimeFeatures],
    labels: &[i32],
) -> (Vec<RegimeFeatures>, Vec<i32>) {
    let mut sums: HashMap<i32, [f64; RegimeFeatures::DIMENSIONS]> = HashMap::new();
    let mut counts: HashMap<i32, usize> = HashMap::new();

    for (feat, &label) in features.iter().zip(labels.iter()) {
        if label < 0 {
            continue;
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

impl Default for KmeansDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusteringStrategy for KmeansDetector {
    fn detect(&self, features: &[RegimeFeatures]) -> Result<ClusteringResult, AnalysisError> {
        let detector = self.build_detector();
        Ok(self.regimes_to_result(features, &detector))
    }

    fn classify(&self, point: &RegimeFeatures) -> Option<ClusterAssignment> {
        let centroids = self.centroids.lock().ok()?;
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
            probability: 1.0,
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

        // Run k-means on filtered data
        let sub_result = self.detect(&filtered_features)?;

        let mut full_labels = reconstruct_labels(
            features.len(),
            &sub_result.labels,
            &original_indices,
            &parsed.force_clusters,
        );

        // Apply MustLink / CannotLink post-processing
        apply_link_constraints(features, &mut full_labels, &parsed);

        let noise_count = full_labels.iter().filter(|&&l| l < 0).count();
        let cluster_count = {
            let ids: HashSet<i32> = full_labels.iter().copied().filter(|&l| l >= 0).collect();
            ids.len()
        };

        // Recompute dense centroids after link-constraint modifications.
        let (centroids, centroid_labels) = recompute_centroids_from_labels(features, &full_labels);

        // Store centroids + index->label remap for classify().
        if let Ok(mut stored) = self.centroids.lock() {
            *stored = centroids.clone();
        }
        if let Ok(mut stored_labels) = self.centroid_labels.lock() {
            *stored_labels = centroid_labels;
        }

        Ok(ClusteringResult {
            labels: full_labels,
            centroids,
            cluster_count,
            noise_count,
            probabilities: None,
        })
    }

    fn algorithm_name(&self) -> &str {
        "kmeans"
    }
}

#[cfg(test)]
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
    fn detect_matches_existing_regime_detector_behavior() {
        let detector = KmeansDetector::new().with_min_samples(5).with_max_k(5);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let result = detector.detect(&features).unwrap();

        // Should find 2 clusters (same as RegimeDetector)
        assert_eq!(result.cluster_count, 2);
        assert_eq!(result.noise_count, 0);
        assert_eq!(result.labels.len(), 60);
        assert!(result.probabilities.is_none());

        // All labels should be non-negative (k-means has no noise)
        assert!(result.labels.iter().all(|&l| l >= 0));
    }

    #[test]
    fn classify_works_after_detect() {
        let detector = KmeansDetector::new().with_min_samples(5).with_max_k(5);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let _result = detector.detect(&features).unwrap();

        // Classify a coding point
        let assignment = detector.classify(&coding_point(0.35, 0.85));
        assert!(assignment.is_some());
        let a = assignment.unwrap();
        assert!(a.cluster_id >= 0);
        assert!((a.probability - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn too_few_samples_returns_empty_clusters() {
        let detector = KmeansDetector::new().with_min_samples(50);

        let features: Vec<RegimeFeatures> = (0..10).map(|_| coding_point(0.5, 0.5)).collect();
        let result = detector.detect(&features).unwrap();

        assert_eq!(result.cluster_count, 0);
    }

    #[test]
    fn constraints_applied() {
        let detector = KmeansDetector::new().with_min_samples(5).with_max_k(5);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let constraints = vec![
            ClusterConstraint::NoiseLabel(0),
            ClusterConstraint::ForceCluster(1, 42),
        ];

        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        assert_eq!(result.labels[0], -1); // noise
        assert_eq!(result.labels[1], 42); // forced
    }

    #[test]
    fn algorithm_name_returns_kmeans() {
        let detector = KmeansDetector::new();
        assert_eq!(detector.algorithm_name(), "kmeans");
    }

    #[test]
    fn recompute_centroids_is_dense_for_noncontiguous_labels() {
        // Non-contiguous labels {0, 2, 5} (plus a noise -1) must produce
        // exactly 3 centroids with no phantom padding for absent labels
        // 1, 3, 4 (#6120).
        let features = vec![
            coding_point(0.10, 0.10), // label 0
            coding_point(0.20, 0.20), // label 2
            coding_point(0.30, 0.30), // label 5
            coding_point(0.40, 0.40), // noise (-1) -> skipped
        ];
        let labels = vec![0, 2, 5, -1];

        let (centroids, centroid_labels) = recompute_centroids_from_labels(&features, &labels);

        assert_eq!(
            centroids.len(),
            3,
            "expected one centroid per present label"
        );
        // Remap is dense and sorted ascending by present label.
        assert_eq!(centroid_labels, vec![0, 2, 5]);

        // Each centroid equals its single member point.
        assert!((centroids[0].avg_event_rate - 0.10).abs() < 1e-5);
        assert!((centroids[1].avg_event_rate - 0.20).abs() < 1e-5);
        assert!((centroids[2].avg_event_rate - 0.30).abs() < 1e-5);
    }

    #[test]
    fn classify_remaps_dense_centroid_index_to_real_label() {
        // Drive the detector into a state with non-contiguous stored labels by
        // injecting dense centroids + remap directly, then verify classify()
        // returns the REAL label, not the array index.
        let detector = KmeansDetector::new();
        {
            let mut c = detector.centroids.lock().unwrap();
            *c = vec![coding_point(0.10, 0.10), coding_point(0.90, 0.90)];
            let mut cl = detector.centroid_labels.lock().unwrap();
            *cl = vec![0, 5]; // index 1 -> real label 5
        }

        // A point nearest the second (index 1) centroid must classify to 5.
        let a = detector.classify(&coding_point(0.88, 0.88)).unwrap();
        assert_eq!(a.cluster_id, 5);

        // A point nearest the first (index 0) centroid must classify to 0.
        let b = detector.classify(&coding_point(0.11, 0.11)).unwrap();
        assert_eq!(b.cluster_id, 0);
    }

    #[test]
    fn default_trait() {
        let detector = KmeansDetector::default();
        assert_eq!(detector.max_k, 7);
        assert_eq!(detector.min_cluster_samples, 50);
    }

    #[test]
    fn must_link_constraint_merges_clusters() {
        let detector = KmeansDetector::new().with_min_samples(5).with_max_k(5);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        // Without constraint, should have 2 clusters
        let result_plain = detector.detect(&features).unwrap();
        assert_eq!(result_plain.cluster_count, 2);

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
        let detector = KmeansDetector::new().with_min_samples(5).with_max_k(5);

        // All coding points — would normally form a single cluster
        let mut features = Vec::new();
        for i in 0..15 {
            features.push(coding_point(0.2 + (i as f32) * 0.005, 0.9));
        }
        for i in 0..15 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.3));
        }

        // CannotLink on two points that would be in same cluster
        let constraints = vec![ClusterConstraint::CannotLink(0, 1)];
        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        // Points 0 and 1 must be in different clusters
        assert_ne!(result.labels[0], result.labels[1]);
    }

    #[test]
    fn mixed_constraints_applied() {
        let detector = KmeansDetector::new().with_min_samples(5).with_max_k(5);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let constraints = vec![
            ClusterConstraint::NoiseLabel(0),
            ClusterConstraint::ForceCluster(1, 42),
            ClusterConstraint::MustLink(2, 31), // merge coding and comm
        ];

        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        assert_eq!(result.labels[0], -1); // noise
        assert_eq!(result.labels[1], 42); // forced
        assert_eq!(result.labels[2], result.labels[31]); // must-linked
    }
}
