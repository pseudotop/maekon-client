//! Regime detection via hand-rolled k-means clustering (ADR-012 §3).
//!
//! Aggregates calibration data feature buckets into activity regimes.
//! No external clustering dependency — 7 features, max 7 clusters.

use chrono::Utc;
use oneshim_core::models::tiered_memory::{
    euclidean_distance, Regime, RegimeFeatures, RegimeStatus, TriggerParams,
};

/// Hand-rolled k-means regime detector.
///
/// Discovers activity regimes by clustering `RegimeFeatures` vectors.
/// Uses silhouette score to select optimal k in [2, max_k].
#[derive(Clone)]
pub struct RegimeDetector {
    max_k: usize,
    max_iterations: usize,
    convergence_threshold: f32,
    min_cluster_samples: usize,
}

impl Default for RegimeDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl RegimeDetector {
    /// Create a detector with default settings (max 7 clusters, 50 iterations,
    /// convergence threshold 0.001, min 50 samples).
    pub fn new() -> Self {
        Self {
            max_k: 7,
            max_iterations: 50,
            convergence_threshold: 0.001,
            min_cluster_samples: 50,
        }
    }

    /// Override the maximum number of clusters.
    pub fn with_max_k(mut self, max_k: usize) -> Self {
        self.max_k = max_k;
        self
    }

    /// Override the minimum number of feature samples required.
    pub fn with_min_samples(mut self, min: usize) -> Self {
        self.min_cluster_samples = min;
        self
    }

    /// Detect regimes from feature buckets.
    ///
    /// Returns discovered regimes with auto-generated labels and default
    /// `TriggerParams`. Returns an empty vec if fewer than `min_cluster_samples`
    /// feature vectors are provided.
    pub fn detect(&self, features: &[RegimeFeatures]) -> Vec<Regime> {
        if features.len() < self.min_cluster_samples {
            return vec![];
        }

        let upper_k = self.max_k.min(features.len());
        if upper_k < 2 {
            return vec![];
        }

        let mut best_score = f32::NEG_INFINITY;
        let mut best_centroids: Vec<RegimeFeatures> = vec![];
        let mut best_assignments: Vec<usize> = vec![];

        for k in 2..=upper_k {
            let (centroids, assignments) = self.kmeans(features, k);
            let score = self.silhouette_score(features, &assignments, &centroids);
            if score > best_score {
                best_score = score;
                best_centroids = centroids;
                best_assignments = assignments;
            }
        }

        self.build_regimes(&best_centroids, &best_assignments, features)
    }

    /// Run k-means with k-means++ initialization.
    fn kmeans(&self, data: &[RegimeFeatures], k: usize) -> (Vec<RegimeFeatures>, Vec<usize>) {
        let mut centroids = self.kmeans_pp_init(data, k);
        let mut assignments = vec![0usize; data.len()];

        for _iter in 0..self.max_iterations {
            // Assignment step
            for (i, point) in data.iter().enumerate() {
                let mut best_c = 0;
                let mut best_d = f32::INFINITY;
                for (c, centroid) in centroids.iter().enumerate() {
                    let d = euclidean_distance(point, centroid);
                    if d < best_d {
                        best_d = d;
                        best_c = c;
                    }
                }
                assignments[i] = best_c;
            }

            // Update step
            let new_centroids = Self::recompute_centroids(data, &assignments, k);

            // Convergence check
            let max_shift = centroids
                .iter()
                .zip(new_centroids.iter())
                .map(|(old, new)| euclidean_distance(old, new))
                .fold(0.0f32, f32::max);

            centroids = new_centroids;

            if max_shift < self.convergence_threshold {
                break;
            }
        }

        (centroids, assignments)
    }

    /// k-means++ initialization: deterministic spread selection.
    ///
    /// First centroid is data[0]; subsequent centroids are chosen as the point
    /// with maximum minimum distance to existing centroids (greedy farthest-first).
    /// This is deterministic and produces well-spread initial centroids.
    fn kmeans_pp_init(&self, data: &[RegimeFeatures], k: usize) -> Vec<RegimeFeatures> {
        let mut centroids = Vec::with_capacity(k);
        centroids.push(data[0].clone());

        // Track minimum distance from each point to any chosen centroid
        let mut min_dists: Vec<f32> = data
            .iter()
            .map(|p| euclidean_distance(p, &centroids[0]))
            .collect();

        for _ in 1..k {
            // Pick the point with the largest min-distance to existing centroids
            let mut best_idx = 0;
            let mut best_dist = f32::NEG_INFINITY;
            for (i, &d) in min_dists.iter().enumerate() {
                if d > best_dist {
                    best_dist = d;
                    best_idx = i;
                }
            }
            centroids.push(data[best_idx].clone());

            // Update min_dists with the newly added centroid
            let new_c = &centroids[centroids.len() - 1];
            for (i, p) in data.iter().enumerate() {
                let d = euclidean_distance(p, new_c);
                if d < min_dists[i] {
                    min_dists[i] = d;
                }
            }
        }

        centroids
    }

    /// Recompute centroids as the mean of assigned points.
    fn recompute_centroids(
        data: &[RegimeFeatures],
        assignments: &[usize],
        k: usize,
    ) -> Vec<RegimeFeatures> {
        let dim = RegimeFeatures::DIMENSIONS;
        let mut sums = vec![[0.0f32; RegimeFeatures::DIMENSIONS]; k];
        let mut counts = vec![0usize; k];

        for (i, point) in data.iter().enumerate() {
            let c = assignments[i];
            counts[c] += 1;
            let arr = point.to_array();
            for d in 0..dim {
                sums[c][d] += arr[d];
            }
        }

        sums.into_iter()
            .zip(counts.iter())
            .map(|(s, &cnt)| {
                if cnt == 0 {
                    RegimeFeatures::default()
                } else {
                    let mut arr = [0.0f32; RegimeFeatures::DIMENSIONS];
                    for d in 0..dim {
                        arr[d] = s[d] / cnt as f32;
                    }
                    RegimeFeatures::from_array(arr)
                }
            })
            .collect()
    }

    /// Compute mean silhouette score.
    ///
    /// For each point: a(i) = avg distance to same cluster,
    /// b(i) = min avg distance to other clusters.
    /// s(i) = (b(i) - a(i)) / max(a(i), b(i)).
    fn silhouette_score(
        &self,
        data: &[RegimeFeatures],
        assignments: &[usize],
        centroids: &[RegimeFeatures],
    ) -> f32 {
        let k = centroids.len();
        let n = data.len();
        if n <= 1 || k <= 1 {
            return 0.0;
        }

        // Pre-group indices by cluster
        let mut clusters: Vec<Vec<usize>> = vec![vec![]; k];
        for (i, &c) in assignments.iter().enumerate() {
            clusters[c].push(i);
        }

        let mut total = 0.0f32;
        let mut valid_count = 0usize;

        for i in 0..n {
            let ci = assignments[i];
            let cluster_size = clusters[ci].len();

            // Singleton cluster: silhouette = 0
            if cluster_size <= 1 {
                continue;
            }

            // a(i): average distance to same-cluster points
            let a_i: f32 = clusters[ci]
                .iter()
                .filter(|&&j| j != i)
                .map(|&j| euclidean_distance(&data[i], &data[j]))
                .sum::<f32>()
                / (cluster_size - 1) as f32;

            // b(i): minimum average distance to another cluster
            let mut b_i = f32::INFINITY;
            for (c, members) in clusters.iter().enumerate() {
                if c == ci || members.is_empty() {
                    continue;
                }
                let avg_d: f32 = members
                    .iter()
                    .map(|&j| euclidean_distance(&data[i], &data[j]))
                    .sum::<f32>()
                    / members.len() as f32;
                if avg_d < b_i {
                    b_i = avg_d;
                }
            }

            let denom = a_i.max(b_i);
            if denom > f32::EPSILON {
                total += (b_i - a_i) / denom;
            }
            valid_count += 1;
        }

        if valid_count == 0 {
            0.0
        } else {
            total / valid_count as f32
        }
    }

    /// Build `Regime` structs from clustering results.
    fn build_regimes(
        &self,
        centroids: &[RegimeFeatures],
        assignments: &[usize],
        _features: &[RegimeFeatures],
    ) -> Vec<Regime> {
        let k = centroids.len();
        let mut counts = vec![0u64; k];
        for &c in assignments {
            counts[c] += 1;
        }

        let now = Utc::now();

        centroids
            .iter()
            .enumerate()
            .filter(|(i, _)| counts[*i] > 0)
            .map(|(i, centroid)| {
                let auto_label = generate_auto_label(centroid, &[]);
                Regime {
                    regime_id: format!("regime-{}", i),
                    name: None,
                    auto_label,
                    centroid: centroid.clone(),
                    optimal_params: TriggerParams::default(),
                    sample_count: counts[i],
                    first_seen: now,
                    last_seen: now,
                    status: RegimeStatus::Active,
                }
            })
            .collect()
    }
}

/// Generate a human-readable auto-label from centroid features.
///
/// Determines the dominant work mode from one-hot category features and
/// appends the top app name hint if available.
pub fn generate_auto_label(centroid: &RegimeFeatures, top_apps: &[String]) -> String {
    let mode = if centroid.category_coding > 0.5 {
        "Deep Focus"
    } else if centroid.category_communication > 0.5 {
        "Communication"
    } else if centroid.category_browser > 0.5 {
        "Research"
    } else {
        "Mixed"
    };

    let app_hint = top_apps.first().map_or("varied", |a| a.as_str());
    format!("{mode} ({app_hint})")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a feature point with coding category dominant.
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

    /// Helper: create a feature point with communication category dominant.
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

    /// Helper: create a feature point with browser category dominant.
    fn browser_point(rate: f32, importance: f32) -> RegimeFeatures {
        RegimeFeatures {
            category_coding: 0.0,
            category_communication: 0.0,
            category_browser: 1.0,
            avg_event_rate: rate,
            avg_importance: importance,
            context_activity_signal: 0.3,
            communication_ratio: 0.15,
        }
    }

    #[test]
    fn too_few_samples_returns_empty() {
        let detector = RegimeDetector::new().with_min_samples(50);
        let features: Vec<RegimeFeatures> = (0..10).map(|_| coding_point(0.5, 0.5)).collect();
        let regimes = detector.detect(&features);
        assert!(regimes.is_empty());
    }

    #[test]
    fn well_separated_clusters_detected() {
        let detector = RegimeDetector::new().with_min_samples(5).with_max_k(5);

        let mut features = Vec::new();
        // Cluster A: coding-heavy (30 points)
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        // Cluster B: communication-heavy (30 points)
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let regimes = detector.detect(&features);

        // Should find exactly 2 regimes
        assert_eq!(regimes.len(), 2);

        // Verify one is coding-dominant and one is communication-dominant
        let labels: Vec<&str> = regimes.iter().map(|r| r.auto_label.as_str()).collect();
        assert!(labels.iter().any(|l| l.contains("Deep Focus")));
        assert!(labels.iter().any(|l| l.contains("Communication")));
    }

    #[test]
    fn three_clusters_detected() {
        let detector = RegimeDetector::new().with_min_samples(5).with_max_k(7);

        let mut features = Vec::new();
        for _ in 0..25 {
            features.push(coding_point(0.3, 0.9));
        }
        for _ in 0..25 {
            features.push(comm_point(0.8, 0.3));
        }
        for _ in 0..25 {
            features.push(browser_point(0.5, 0.5));
        }

        let regimes = detector.detect(&features);
        assert!(regimes.len() >= 2);
        // Total sample count should match input
        let total: u64 = regimes.iter().map(|r| r.sample_count).sum();
        assert_eq!(total, 75);
    }

    #[test]
    fn all_regimes_are_active() {
        let detector = RegimeDetector::new().with_min_samples(5);

        let mut features = Vec::new();
        for _ in 0..20 {
            features.push(coding_point(0.3, 0.8));
        }
        for _ in 0..20 {
            features.push(comm_point(0.7, 0.4));
        }

        let regimes = detector.detect(&features);
        assert!(regimes.iter().all(|r| r.status == RegimeStatus::Active));
    }

    #[test]
    fn kmeans_convergence() {
        let detector = RegimeDetector::new();

        // Two well-separated clusters
        let mut data = Vec::new();
        for _ in 0..20 {
            data.push(RegimeFeatures {
                category_coding: 1.0,
                avg_event_rate: 0.2,
                ..RegimeFeatures::default()
            });
        }
        for _ in 0..20 {
            data.push(RegimeFeatures {
                category_communication: 1.0,
                avg_event_rate: 0.8,
                ..RegimeFeatures::default()
            });
        }

        let (centroids, assignments) = detector.kmeans(&data, 2);
        assert_eq!(centroids.len(), 2);
        assert_eq!(assignments.len(), 40);

        // All points in each half should be in the same cluster
        let first_half_cluster = assignments[0];
        assert!(assignments[0..20].iter().all(|&a| a == first_half_cluster));
        let second_half_cluster = assignments[20];
        assert!(assignments[20..40]
            .iter()
            .all(|&a| a == second_half_cluster));
        assert_ne!(first_half_cluster, second_half_cluster);
    }

    #[test]
    fn silhouette_perfect_separation() {
        let detector = RegimeDetector::new();

        let data = vec![
            RegimeFeatures {
                category_coding: 1.0,
                ..RegimeFeatures::default()
            },
            RegimeFeatures {
                category_coding: 1.0,
                ..RegimeFeatures::default()
            },
            RegimeFeatures {
                category_communication: 1.0,
                ..RegimeFeatures::default()
            },
            RegimeFeatures {
                category_communication: 1.0,
                ..RegimeFeatures::default()
            },
        ];
        let assignments = vec![0, 0, 1, 1];
        let centroids = vec![
            RegimeFeatures {
                category_coding: 1.0,
                ..RegimeFeatures::default()
            },
            RegimeFeatures {
                category_communication: 1.0,
                ..RegimeFeatures::default()
            },
        ];

        let score = detector.silhouette_score(&data, &assignments, &centroids);
        // Perfectly separated identical points → silhouette close to 1.0
        assert!(score > 0.5, "expected high silhouette score, got {score}");
    }

    #[test]
    fn auto_label_coding() {
        let centroid = RegimeFeatures {
            category_coding: 0.8,
            ..RegimeFeatures::default()
        };
        let label = generate_auto_label(&centroid, &["VSCode".to_string()]);
        assert_eq!(label, "Deep Focus (VSCode)");
    }

    #[test]
    fn auto_label_communication() {
        let centroid = RegimeFeatures {
            category_communication: 0.7,
            ..RegimeFeatures::default()
        };
        let label = generate_auto_label(&centroid, &["Slack".to_string()]);
        assert_eq!(label, "Communication (Slack)");
    }

    #[test]
    fn auto_label_browser() {
        let centroid = RegimeFeatures {
            category_browser: 0.6,
            ..RegimeFeatures::default()
        };
        let label = generate_auto_label(&centroid, &[]);
        assert_eq!(label, "Research (varied)");
    }

    #[test]
    fn auto_label_mixed() {
        let centroid = RegimeFeatures {
            category_coding: 0.3,
            category_communication: 0.3,
            category_browser: 0.2,
            ..RegimeFeatures::default()
        };
        let label = generate_auto_label(&centroid, &[]);
        assert_eq!(label, "Mixed (varied)");
    }

    #[test]
    fn detector_default_trait() {
        let d = RegimeDetector::default();
        assert_eq!(d.max_k, 7);
        assert_eq!(d.min_cluster_samples, 50);
    }
}
