//! Unified clustering interface for regime detection.
//!
//! Both `HdbscanDetector` and `KmeansDetector` implement this trait,
//! allowing the regime detection pipeline to be algorithm-agnostic.
//!
//! ## Algorithm Selection Guide
//!
//! | Criterion | k-means (`KmeansDetector`) | HDBSCAN (`HdbscanDetector`) |
//! |-----------|--------------------------|----------------------------|
//! | Cluster count | Fixed range [2, max_k=7] | Auto-discovered |
//! | Noise handling | None (all points assigned) | Noise label (-1) for outliers |
//! | Best for | Bounded feature set, real-time detection | High-noise data, variable cluster counts |
//! | Performance | O(n*k*iter) + O(n^2) silhouette | O(n^2) core distance + dendrogram |
//! | Default | Yes (fallback in `regime.rs`) | Opt-in via `ClusteringStrategy` |
//!
//! **Default behavior**: The regime detection pipeline uses k-means (`RegimeDetector`)
//! when no `ClusteringStrategy` is configured. HDBSCAN is used when explicitly set via
//! `AdaptiveTriggerState.clustering_strategy`.
//!
//! Shared constraint preprocessing helpers are provided so that each
//! detector does not duplicate the NoiseLabel / ForceCluster / MustLink /
//! CannotLink logic.

use std::collections::{HashMap, HashSet};

use oneshim_core::models::recalibration::ClusterConstraint;
use oneshim_core::models::tiered_memory::{euclidean_distance, RegimeFeatures};
use tracing::{debug, warn};

use crate::error::AnalysisError;

/// Result of a clustering operation.
#[derive(Debug, Clone)]
pub struct ClusteringResult {
    /// Cluster label per input point. -1 = noise (HDBSCAN), non-negative = cluster ID.
    pub labels: Vec<i32>,
    /// Centroid (mean feature vector) per cluster, indexed by cluster ID.
    pub centroids: Vec<RegimeFeatures>,
    /// Number of distinct clusters discovered (excluding noise).
    pub cluster_count: usize,
    /// Number of points labeled as noise (-1).
    pub noise_count: usize,
    /// Soft membership probabilities (HDBSCAN only). `None` for hard-assignment algorithms.
    pub probabilities: Option<Vec<f32>>,
}

/// Assignment of a single point to a cluster.
#[derive(Debug, Clone)]
pub struct ClusterAssignment {
    /// The cluster this point belongs to.
    pub cluster_id: i32,
    /// Confidence of the assignment (1.0 for k-means, soft for HDBSCAN).
    pub probability: f32,
}

/// Unified interface for clustering algorithms used in regime detection.
///
/// This is NOT a port trait — it is a pure algorithm interface within
/// `oneshim-analysis`. Both `HdbscanDetector` and `KmeansDetector` implement it.
pub trait ClusteringStrategy: Send + Sync {
    /// Detect regimes from feature vectors.
    fn detect(&self, features: &[RegimeFeatures]) -> Result<ClusteringResult, AnalysisError>;

    /// Classify a single new point against existing clusters (nearest-centroid).
    fn classify(&self, point: &RegimeFeatures) -> Option<ClusterAssignment>;

    /// Re-detect with user override constraints applied.
    fn detect_with_constraints(
        &self,
        features: &[RegimeFeatures],
        constraints: &[ClusterConstraint],
    ) -> Result<ClusteringResult, AnalysisError>;

    /// Human-readable algorithm name for config/logging.
    fn algorithm_name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Shared constraint preprocessing helpers
// ---------------------------------------------------------------------------

/// Parsed constraint directives ready for the clustering pipeline.
///
/// Produced by [`parse_constraints`], consumed by [`filter_features`],
/// [`reconstruct_labels`], [`apply_must_link_constraints`], and
/// [`apply_cannot_link_constraints`].
pub struct ParsedConstraints {
    /// Point indices that should be excluded from clustering and labeled as noise.
    pub noise_indices: HashSet<usize>,
    /// Point indices that should be force-assigned to a specific cluster ID.
    pub force_clusters: HashMap<usize, i32>,
    /// Pairs of point indices that must end up in the same cluster.
    pub must_links: Vec<(usize, usize)>,
    /// Pairs of point indices that must end up in different clusters.
    pub cannot_links: Vec<(usize, usize)>,
}

/// Parse a slice of [`ClusterConstraint`] into noise exclusions, force-cluster
/// assignments, must-link pairs, and cannot-link pairs.
///
/// `algorithm` is included in diagnostic log messages (e.g. "hdbscan", "k-means").
pub fn parse_constraints(constraints: &[ClusterConstraint], algorithm: &str) -> ParsedConstraints {
    let mut noise_indices = HashSet::new();
    let mut force_clusters: HashMap<usize, i32> = HashMap::new();
    let mut must_links: Vec<(usize, usize)> = Vec::new();
    let mut cannot_links: Vec<(usize, usize)> = Vec::new();

    for constraint in constraints {
        match constraint {
            ClusterConstraint::NoiseLabel(idx) => {
                noise_indices.insert(*idx);
            }
            ClusterConstraint::ForceCluster(idx, cluster_id) => {
                force_clusters.insert(*idx, *cluster_id);
            }
            ClusterConstraint::MustLink(a, b) => {
                debug!("{algorithm}: MustLink({a}, {b}) constraint registered");
                must_links.push((*a, *b));
            }
            ClusterConstraint::CannotLink(a, b) => {
                debug!("{algorithm}: CannotLink({a}, {b}) constraint registered");
                cannot_links.push((*a, *b));
            }
        }
    }

    ParsedConstraints {
        noise_indices,
        force_clusters,
        must_links,
        cannot_links,
    }
}

/// Filter out noise-labeled points and return the surviving features together
/// with a mapping back to their original indices.
pub fn filter_features(
    features: &[RegimeFeatures],
    noise_indices: &HashSet<usize>,
) -> (Vec<RegimeFeatures>, Vec<usize>) {
    let mut filtered = Vec::new();
    let mut original_indices = Vec::new();
    for (i, feat) in features.iter().enumerate() {
        if !noise_indices.contains(&i) {
            filtered.push(feat.clone());
            original_indices.push(i);
        }
    }
    (filtered, original_indices)
}

/// Reconstruct a full-length label vector from the sub-result produced by
/// clustering on filtered data, then apply ForceCluster overrides.
///
/// Points that were excluded (noise) retain label -1.
pub fn reconstruct_labels(
    total_len: usize,
    sub_labels: &[i32],
    original_indices: &[usize],
    force_clusters: &HashMap<usize, i32>,
) -> Vec<i32> {
    let mut full_labels = vec![-1i32; total_len];
    for (sub_idx, &orig_idx) in original_indices.iter().enumerate() {
        if sub_idx < sub_labels.len() {
            full_labels[orig_idx] = sub_labels[sub_idx];
        }
    }
    for (&idx, &cluster_id) in force_clusters {
        if idx < full_labels.len() {
            full_labels[idx] = cluster_id;
        }
    }
    full_labels
}

// ---------------------------------------------------------------------------
// MustLink / CannotLink post-processing (Phase 2)
// ---------------------------------------------------------------------------

/// Apply MustLink constraints by merging clusters.
///
/// For each `(a, b)` pair: if points `a` and `b` are in different clusters,
/// the smaller cluster is merged into the larger one (all its members are
/// relabeled). Noise points (label -1) are skipped.
pub fn apply_must_link_constraints(labels: &mut [i32], must_links: &[(usize, usize)]) {
    for &(a, b) in must_links {
        if a >= labels.len() || b >= labels.len() {
            warn!(
                "MustLink({a}, {b}) out of bounds (len={}), skipped",
                labels.len()
            );
            continue;
        }
        let la = labels[a];
        let lb = labels[b];

        // Skip if already same cluster, or either is noise
        if la == lb || la < 0 || lb < 0 {
            continue;
        }

        // Count members of each cluster to decide merge direction
        let count_a = labels.iter().filter(|&&l| l == la).count();
        let count_b = labels.iter().filter(|&&l| l == lb).count();

        // Merge smaller into larger
        let (keep, merge) = if count_a >= count_b {
            (la, lb)
        } else {
            (lb, la)
        };

        debug!("MustLink({a}, {b}): merging cluster {merge} into {keep}");
        for label in labels.iter_mut() {
            if *label == merge {
                *label = keep;
            }
        }
    }
}

/// Apply CannotLink constraints by splitting clusters.
///
/// For each `(a, b)` pair: if points `a` and `b` are in the same cluster,
/// that cluster is split by running k-means(k=2) on its members. The two
/// resulting sub-clusters receive the original label and a fresh label
/// (max existing label + 1). Noise points (label -1) are skipped.
pub fn apply_cannot_link_constraints(
    features: &[RegimeFeatures],
    labels: &mut [i32],
    cannot_links: &[(usize, usize)],
) {
    for &(a, b) in cannot_links {
        if a >= labels.len() || b >= labels.len() {
            warn!(
                "CannotLink({a}, {b}) out of bounds (len={}), skipped",
                labels.len()
            );
            continue;
        }
        let la = labels[a];
        let lb = labels[b];

        // Only act if both are in the same non-noise cluster
        if la != lb || la < 0 {
            continue;
        }

        let target_cluster = la;

        // Collect members of the target cluster
        let members: Vec<usize> = labels
            .iter()
            .enumerate()
            .filter(|(_, &l)| l == target_cluster)
            .map(|(i, _)| i)
            .collect();

        if members.len() < 2 {
            continue;
        }

        // Run k-means(k=2) on the member features
        let member_features: Vec<RegimeFeatures> =
            members.iter().map(|&i| features[i].clone()).collect();

        let sub_labels = kmeans_split(&member_features, a, b, &members);

        // Allocate a new cluster ID for the second sub-cluster
        let new_cluster_id = labels.iter().copied().max().unwrap_or(0) + 1;

        debug!(
            "CannotLink({a}, {b}): splitting cluster {target_cluster} into {target_cluster} and {new_cluster_id}"
        );

        // Apply sub-labels: group 0 keeps the original ID, group 1 gets new ID
        for (sub_idx, &orig_idx) in members.iter().enumerate() {
            if sub_labels[sub_idx] == 1 {
                labels[orig_idx] = new_cluster_id;
            }
            // group 0 retains target_cluster (already set)
        }
    }
}

/// Mini k-means(k=2) for CannotLink cluster splitting.
///
/// Seeds the two centroids from points `a` and `b` (the constrained pair),
/// then runs up to 20 iterations of Lloyd's algorithm. Returns 0/1 labels
/// for each member point.
fn kmeans_split(
    features: &[RegimeFeatures],
    constraint_a: usize,
    constraint_b: usize,
    members: &[usize],
) -> Vec<u8> {
    const MAX_ITER: usize = 20;
    let dim = RegimeFeatures::DIMENSIONS;

    // Find a and b within the members list to use as initial centroids
    let local_a = members.iter().position(|&i| i == constraint_a);
    let local_b = members.iter().position(|&i| i == constraint_b);

    let (seed_a, seed_b) = match (local_a, local_b) {
        (Some(ia), Some(ib)) => (ia, ib),
        // Fallback: use first and last member
        _ => (0, features.len() - 1),
    };

    let mut centroids = [
        features[seed_a].to_array().map(|v| v as f64),
        features[seed_b].to_array().map(|v| v as f64),
    ];

    let mut assignments = vec![0u8; features.len()];

    for _ in 0..MAX_ITER {
        // Assignment step
        let mut changed = false;
        for (i, feat) in features.iter().enumerate() {
            let d0 = euclidean_distance(
                feat,
                &RegimeFeatures::from_array(centroids[0].map(|v| v as f32)),
            );
            let d1 = euclidean_distance(
                feat,
                &RegimeFeatures::from_array(centroids[1].map(|v| v as f32)),
            );
            let new_label = if d0 <= d1 { 0u8 } else { 1u8 };
            if new_label != assignments[i] {
                changed = true;
                assignments[i] = new_label;
            }
        }

        if !changed {
            break;
        }

        // Update step
        let mut sums = [[0.0f64; RegimeFeatures::DIMENSIONS]; 2];
        let mut counts = [0usize; 2];

        for (i, feat) in features.iter().enumerate() {
            let c = assignments[i] as usize;
            counts[c] += 1;
            let arr = feat.to_array();
            for d in 0..dim {
                sums[c][d] += arr[d] as f64;
            }
        }

        for c in 0..2 {
            if counts[c] > 0 {
                for d in 0..dim {
                    centroids[c][d] = sums[c][d] / counts[c] as f64;
                }
            }
        }
    }

    // Ensure a and b end up in different clusters. If they ended up in the
    // same cluster (degenerate case), force b into the other cluster.
    if let (Some(ia), Some(ib)) = (local_a, local_b) {
        if assignments[ia] == assignments[ib] {
            assignments[ib] = 1 - assignments[ia];
        }
    }

    assignments
}

// ---------------------------------------------------------------------------
// Convenience: apply all link constraints in order
// ---------------------------------------------------------------------------

/// Apply MustLink and CannotLink constraints to a label vector.
///
/// This is the standard post-processing entry point that both detectors call
/// after initial clustering and NoiseLabel/ForceCluster reconstruction.
/// MustLink is applied first (merges), then CannotLink (splits).
pub fn apply_link_constraints(
    features: &[RegimeFeatures],
    labels: &mut [i32],
    parsed: &ParsedConstraints,
) {
    if !parsed.must_links.is_empty() {
        apply_must_link_constraints(labels, &parsed.must_links);
    }
    if !parsed.cannot_links.is_empty() {
        apply_cannot_link_constraints(features, labels, &parsed.cannot_links);
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

    // -----------------------------------------------------------------------
    // MustLink tests
    // -----------------------------------------------------------------------

    #[test]
    fn must_link_merges_different_clusters() {
        // Points 0-2 in cluster 0, points 3-5 in cluster 1
        let mut labels = vec![0, 0, 0, 1, 1, 1];
        let must_links = vec![(0, 3)]; // force cluster 0 and 1 to merge

        apply_must_link_constraints(&mut labels, &must_links);

        // All points should now share the same cluster
        let first = labels[0];
        assert!(labels.iter().all(|&l| l == first));
    }

    #[test]
    fn must_link_smaller_cluster_merges_into_larger() {
        // Cluster 0 has 4 members, cluster 1 has 2 members
        let mut labels = vec![0, 0, 0, 0, 1, 1];
        let must_links = vec![(0, 4)];

        apply_must_link_constraints(&mut labels, &must_links);

        // Cluster 1 (smaller) should be merged into cluster 0 (larger)
        assert!(labels.iter().all(|&l| l == 0));
    }

    #[test]
    fn must_link_same_cluster_is_noop() {
        let mut labels = vec![0, 0, 0, 1, 1, 1];
        let original = labels.clone();
        let must_links = vec![(0, 1)]; // already in same cluster

        apply_must_link_constraints(&mut labels, &must_links);

        assert_eq!(labels, original);
    }

    #[test]
    fn must_link_skips_noise_points() {
        let mut labels = vec![-1, 0, 0, 1, 1, 1];
        let original = labels.clone();
        let must_links = vec![(0, 3)]; // point 0 is noise

        apply_must_link_constraints(&mut labels, &must_links);

        assert_eq!(labels, original);
    }

    #[test]
    fn must_link_out_of_bounds_skipped() {
        let mut labels = vec![0, 0, 1, 1];
        let original = labels.clone();
        let must_links = vec![(0, 99)]; // index 99 out of bounds

        apply_must_link_constraints(&mut labels, &must_links);

        assert_eq!(labels, original);
    }

    #[test]
    fn must_link_chain_merges_all() {
        // Three clusters merged via chain: 0-1, 1-2
        let mut labels = vec![0, 0, 1, 1, 2, 2];
        let must_links = vec![(0, 2), (2, 4)];

        apply_must_link_constraints(&mut labels, &must_links);

        let first = labels[0];
        assert!(labels.iter().all(|&l| l == first));
    }

    // -----------------------------------------------------------------------
    // CannotLink tests
    // -----------------------------------------------------------------------

    #[test]
    fn cannot_link_splits_same_cluster() {
        // All points in cluster 0, constrain points 0 and 5 to be separated
        let features = vec![
            coding_point(0.1, 0.9),
            coding_point(0.15, 0.85),
            coding_point(0.12, 0.88),
            comm_point(0.7, 0.3),
            comm_point(0.75, 0.35),
            comm_point(0.72, 0.32),
        ];
        let mut labels = vec![0, 0, 0, 0, 0, 0];
        let cannot_links = vec![(0, 3)];

        apply_cannot_link_constraints(&features, &mut labels, &cannot_links);

        // Points 0 and 3 must now be in different clusters
        assert_ne!(labels[0], labels[3]);
    }

    #[test]
    fn cannot_link_different_clusters_is_noop() {
        let features = vec![
            coding_point(0.1, 0.9),
            coding_point(0.15, 0.85),
            comm_point(0.7, 0.3),
            comm_point(0.75, 0.35),
        ];
        let mut labels = vec![0, 0, 1, 1];
        let original = labels.clone();
        let cannot_links = vec![(0, 2)]; // already in different clusters

        apply_cannot_link_constraints(&features, &mut labels, &cannot_links);

        assert_eq!(labels, original);
    }

    #[test]
    fn cannot_link_skips_noise_points() {
        let features = vec![coding_point(0.1, 0.9), coding_point(0.15, 0.85)];
        let mut labels = vec![-1, -1];
        let original = labels.clone();
        let cannot_links = vec![(0, 1)];

        apply_cannot_link_constraints(&features, &mut labels, &cannot_links);

        assert_eq!(labels, original);
    }

    #[test]
    fn cannot_link_out_of_bounds_skipped() {
        let features = vec![coding_point(0.1, 0.9), coding_point(0.15, 0.85)];
        let mut labels = vec![0, 0];
        let original = labels.clone();
        let cannot_links = vec![(0, 99)];

        apply_cannot_link_constraints(&features, &mut labels, &cannot_links);

        assert_eq!(labels, original);
    }

    #[test]
    fn cannot_link_preserves_other_clusters() {
        let features = vec![
            coding_point(0.1, 0.9),
            coding_point(0.15, 0.85),
            comm_point(0.7, 0.3),
            comm_point(0.75, 0.35),
        ];
        let mut labels = vec![0, 0, 1, 1];
        let cannot_links = vec![(0, 1)]; // split cluster 0

        apply_cannot_link_constraints(&features, &mut labels, &cannot_links);

        // Points 0 and 1 must be in different clusters
        assert_ne!(labels[0], labels[1]);
        // Cluster 1 should remain untouched
        assert_eq!(labels[2], 1);
        assert_eq!(labels[3], 1);
    }

    // -----------------------------------------------------------------------
    // Combined apply_link_constraints tests
    // -----------------------------------------------------------------------

    #[test]
    fn apply_link_constraints_must_then_cannot() {
        let features = vec![
            coding_point(0.1, 0.9),
            coding_point(0.12, 0.88),
            comm_point(0.7, 0.3),
            comm_point(0.72, 0.32),
        ];
        let mut labels = vec![0, 0, 1, 1];
        let parsed = ParsedConstraints {
            noise_indices: HashSet::new(),
            force_clusters: HashMap::new(),
            must_links: vec![(0, 2)],   // merge clusters 0 and 1
            cannot_links: vec![(0, 2)], // then split them apart again
        };

        apply_link_constraints(&features, &mut labels, &parsed);

        // After merge, all are in same cluster. After split, 0 and 2 differ.
        assert_ne!(labels[0], labels[2]);
    }

    // -----------------------------------------------------------------------
    // parse_constraints tests
    // -----------------------------------------------------------------------

    #[test]
    fn parse_constraints_collects_must_and_cannot_links() {
        let constraints = vec![
            ClusterConstraint::NoiseLabel(0),
            ClusterConstraint::MustLink(1, 2),
            ClusterConstraint::CannotLink(3, 4),
            ClusterConstraint::ForceCluster(5, 99),
        ];

        let parsed = parse_constraints(&constraints, "test");

        assert!(parsed.noise_indices.contains(&0));
        assert_eq!(parsed.must_links, vec![(1, 2)]);
        assert_eq!(parsed.cannot_links, vec![(3, 4)]);
        assert_eq!(parsed.force_clusters.get(&5), Some(&99));
    }

    // -----------------------------------------------------------------------
    // kmeans_split tests
    // -----------------------------------------------------------------------

    #[test]
    fn kmeans_split_separates_distinct_groups() {
        let features = vec![
            coding_point(0.1, 0.9),
            coding_point(0.12, 0.88),
            comm_point(0.7, 0.3),
            comm_point(0.72, 0.32),
        ];
        let members = vec![0, 1, 2, 3];

        let sub_labels = kmeans_split(&features, 0, 2, &members);

        // Points 0,1 (coding) should be in one group; 2,3 (comm) in the other
        assert_eq!(sub_labels[0], sub_labels[1]);
        assert_eq!(sub_labels[2], sub_labels[3]);
        assert_ne!(sub_labels[0], sub_labels[2]);
    }
}
