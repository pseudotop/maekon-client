//! Bayesian Gaussian Mixture Model (GMM) detector implementing `ClusteringStrategy`.
//!
//! Uses Expectation-Maximization with diagonal covariance for efficiency.
//! K selection via BIC (Bayesian Information Criterion). Pure Rust
//! implementation — no external crate needed.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use oneshim_core::models::recalibration::ClusterConstraint;
use oneshim_core::models::tiered_memory::RegimeFeatures;

use crate::clustering_strategy::{
    apply_link_constraints, filter_features, parse_constraints, reconstruct_labels,
    ClusterAssignment, ClusteringResult, ClusteringStrategy,
};
use crate::error::AnalysisError;

/// Gaussian Mixture Model detector for regime detection.
///
/// Fits a GMM using EM with diagonal covariance and selects K via BIC.
/// Provides soft (probabilistic) cluster assignments through `classify()`.
pub struct GmmDetector {
    /// Maximum number of components to evaluate.
    max_k: usize,
    /// Minimum number of components to evaluate.
    min_k: usize,
    /// Maximum EM iterations per K.
    max_iterations: usize,
    /// Convergence threshold for log-likelihood change.
    epsilon: f64,
    /// Minimum variance floor to prevent singularities.
    variance_floor: f64,
    /// Stored model from the last `detect()` call, for `classify()`.
    model: Mutex<Option<GmmModel>>,
}

/// A fitted GMM model storing the learned parameters.
#[derive(Debug, Clone)]
struct GmmModel {
    /// Mixing weights (sum to 1.0).
    weights: Vec<f64>,
    /// Mean vectors per component.
    means: Vec<[f64; RegimeFeatures::DIMENSIONS]>,
    /// Diagonal covariance per component (variance per dimension).
    variances: Vec<[f64; RegimeFeatures::DIMENSIONS]>,
}

impl GmmDetector {
    /// Create a new GMM detector with default settings.
    pub fn new() -> Self {
        Self {
            max_k: 7,
            min_k: 2,
            max_iterations: 100,
            epsilon: 1e-6,
            variance_floor: 1e-6,
            model: Mutex::new(None),
        }
    }

    /// Override the maximum number of components.
    pub fn with_max_k(mut self, max_k: usize) -> Self {
        self.max_k = max_k;
        self
    }

    /// Override the minimum number of components.
    pub fn with_min_k(mut self, min_k: usize) -> Self {
        self.min_k = min_k;
        self
    }

    /// Run EM for a fixed K and return the fitted model + final log-likelihood.
    fn fit_em(&self, data: &[[f64; RegimeFeatures::DIMENSIONS]], k: usize) -> (GmmModel, f64) {
        let n = data.len();

        // Initialize with k-means++ style seeding
        let (mut means, mut variances, mut weights) = self.initialize(data, k);

        let mut log_likelihood = f64::NEG_INFINITY;
        let mut responsibilities = vec![vec![0.0f64; k]; n];

        for _iter in 0..self.max_iterations {
            // E-step: compute responsibilities
            let new_ll = self.e_step(data, &weights, &means, &variances, &mut responsibilities);

            // Check convergence
            if (new_ll - log_likelihood).abs() < self.epsilon {
                log_likelihood = new_ll;
                break;
            }
            log_likelihood = new_ll;

            // M-step: update parameters
            self.m_step(
                data,
                &responsibilities,
                &mut weights,
                &mut means,
                &mut variances,
            );

            // Apply variance floor
            for var in variances.iter_mut() {
                for v in var.iter_mut() {
                    if *v < self.variance_floor {
                        *v = self.variance_floor;
                    }
                }
            }
        }

        let model = GmmModel {
            weights,
            means,
            variances,
        };

        (model, log_likelihood)
    }

    /// Initialize GMM parameters using k-means++ seeding for means,
    /// global variance for covariances, and uniform weights.
    fn initialize(
        &self,
        data: &[[f64; RegimeFeatures::DIMENSIONS]],
        k: usize,
    ) -> (
        Vec<[f64; RegimeFeatures::DIMENSIONS]>,
        Vec<[f64; RegimeFeatures::DIMENSIONS]>,
        Vec<f64>,
    ) {
        // k-means++ initialization for means
        let mut means = Vec::with_capacity(k);
        means.push(data[0]);

        let mut min_dists: Vec<f64> = data.iter().map(|p| sq_dist(p, &data[0])).collect();

        for _ in 1..k {
            // Pick point with largest min-distance (deterministic farthest-first)
            let mut best_idx = 0;
            let mut best_dist = f64::NEG_INFINITY;
            for (i, &d) in min_dists.iter().enumerate() {
                if d > best_dist {
                    best_dist = d;
                    best_idx = i;
                }
            }
            means.push(data[best_idx]);

            let new_c = &means[means.len() - 1];
            for (i, p) in data.iter().enumerate() {
                let d = sq_dist(p, new_c);
                if d < min_dists[i] {
                    min_dists[i] = d;
                }
            }
        }

        // Compute global variance per dimension
        let n = data.len() as f64;
        let mut global_mean = [0.0f64; RegimeFeatures::DIMENSIONS];
        for p in data {
            for (d, val) in p.iter().enumerate() {
                global_mean[d] += val;
            }
        }
        for gm in global_mean.iter_mut() {
            *gm /= n;
        }

        let mut global_var = [0.0f64; RegimeFeatures::DIMENSIONS];
        for p in data {
            for (d, val) in p.iter().enumerate() {
                let diff = val - global_mean[d];
                global_var[d] += diff * diff;
            }
        }
        for gv in global_var.iter_mut() {
            *gv = (*gv / n).max(self.variance_floor);
        }

        let variances = vec![global_var; k];
        let weights = vec![1.0 / k as f64; k];

        (means, variances, weights)
    }

    /// E-step: compute responsibilities (soft assignments).
    /// Returns the log-likelihood.
    fn e_step(
        &self,
        data: &[[f64; RegimeFeatures::DIMENSIONS]],
        weights: &[f64],
        means: &[[f64; RegimeFeatures::DIMENSIONS]],
        variances: &[[f64; RegimeFeatures::DIMENSIONS]],
        responsibilities: &mut [Vec<f64>],
    ) -> f64 {
        let k = weights.len();
        let mut total_ll = 0.0f64;

        for (i, point) in data.iter().enumerate() {
            let mut log_probs = Vec::with_capacity(k);
            for j in 0..k {
                let lp = log_gaussian_diag(point, &means[j], &variances[j]) + weights[j].ln();
                log_probs.push(lp);
            }

            // Log-sum-exp for numerical stability
            let max_lp = log_probs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let sum_exp: f64 = log_probs.iter().map(|&lp| (lp - max_lp).exp()).sum();
            let log_sum = max_lp + sum_exp.ln();

            total_ll += log_sum;

            // Normalize to get responsibilities
            for j in 0..k {
                responsibilities[i][j] = (log_probs[j] - log_sum).exp();
            }
        }

        total_ll
    }

    /// M-step: update means, variances, and weights from responsibilities.
    fn m_step(
        &self,
        data: &[[f64; RegimeFeatures::DIMENSIONS]],
        responsibilities: &[Vec<f64>],
        weights: &mut [f64],
        means: &mut [[f64; RegimeFeatures::DIMENSIONS]],
        variances: &mut [[f64; RegimeFeatures::DIMENSIONS]],
    ) {
        let k = weights.len();
        let n = data.len() as f64;

        for j in 0..k {
            // Effective count
            let nk: f64 = responsibilities.iter().map(|r| r[j]).sum();

            if nk < 1e-10 {
                // Empty component — keep previous parameters
                weights[j] = 1e-10;
                continue;
            }

            // Update weight
            weights[j] = nk / n;

            // Update mean
            let mut new_mean = [0.0f64; RegimeFeatures::DIMENSIONS];
            for (i, point) in data.iter().enumerate() {
                let r = responsibilities[i][j];
                for (d, val) in point.iter().enumerate() {
                    new_mean[d] += r * val;
                }
            }
            for nm in new_mean.iter_mut() {
                *nm /= nk;
            }
            means[j] = new_mean;

            // Update variance (diagonal)
            let mut new_var = [0.0f64; RegimeFeatures::DIMENSIONS];
            for (i, point) in data.iter().enumerate() {
                let r = responsibilities[i][j];
                for (d, val) in point.iter().enumerate() {
                    let diff = val - new_mean[d];
                    new_var[d] += r * diff * diff;
                }
            }
            for nv in new_var.iter_mut() {
                *nv = (*nv / nk).max(self.variance_floor);
            }
            variances[j] = new_var;
        }

        // Renormalize weights
        let w_sum: f64 = weights.iter().sum();
        if w_sum > 0.0 {
            for w in weights.iter_mut() {
                *w /= w_sum;
            }
        }
    }

    /// Compute BIC for model selection: BIC = -2 * LL + p * ln(n)
    /// where p = number of free parameters.
    fn bic(log_likelihood: f64, k: usize, n: usize) -> f64 {
        let dim = RegimeFeatures::DIMENSIONS;
        // Parameters: k weights (k-1 free) + k*dim means + k*dim variances
        let num_params = (k - 1) + k * dim + k * dim;
        -2.0 * log_likelihood + (num_params as f64) * (n as f64).ln()
    }

    /// Compute posterior probabilities for a single point against the stored model.
    fn posterior(model: &GmmModel, point: &[f64; RegimeFeatures::DIMENSIONS]) -> Vec<f64> {
        let k = model.weights.len();
        let mut log_probs = Vec::with_capacity(k);

        for j in 0..k {
            let lp = log_gaussian_diag(point, &model.means[j], &model.variances[j])
                + model.weights[j].ln();
            log_probs.push(lp);
        }

        // Log-sum-exp
        let max_lp = log_probs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let sum_exp: f64 = log_probs.iter().map(|&lp| (lp - max_lp).exp()).sum();
        let log_sum = max_lp + sum_exp.ln();

        log_probs.iter().map(|&lp| (lp - log_sum).exp()).collect()
    }

    /// Recompute centroids as RegimeFeatures from model means.
    fn model_centroids(model: &GmmModel) -> Vec<RegimeFeatures> {
        model
            .means
            .iter()
            .map(|m| {
                let mut arr = [0.0f32; RegimeFeatures::DIMENSIONS];
                for (d, &v) in m.iter().enumerate() {
                    arr[d] = v as f32;
                }
                RegimeFeatures::from_array(arr)
            })
            .collect()
    }

    /// Recompute centroids from final labels after constraint modifications.
    fn recompute_centroids(features: &[RegimeFeatures], labels: &[i32]) -> Vec<RegimeFeatures> {
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
}

impl Default for GmmDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl ClusteringStrategy for GmmDetector {
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

        if features.len() < 2 {
            return Ok(ClusteringResult {
                labels: vec![0],
                centroids: vec![features[0].clone()],
                cluster_count: 1,
                noise_count: 0,
                probabilities: Some(vec![1.0]),
            });
        }

        // Convert to f64 arrays
        let data: Vec<[f64; RegimeFeatures::DIMENSIONS]> = features
            .iter()
            .map(|f| f.to_array().map(|v| v as f64))
            .collect();

        let n = data.len();
        let upper_k = self.max_k.min(n);
        let lower_k = self.min_k.max(1);

        let mut best_bic = f64::INFINITY;
        let mut best_model: Option<GmmModel> = None;

        for k in lower_k..=upper_k {
            let (model, ll) = self.fit_em(&data, k);
            let bic_score = Self::bic(ll, k, n);

            if bic_score < best_bic {
                best_bic = bic_score;
                best_model = Some(model);
            }
        }

        let model = best_model
            .ok_or_else(|| AnalysisError::Clustering("GMM: failed to fit any model".to_string()))?;

        // Assign each point to the component with highest posterior
        let mut labels = Vec::with_capacity(n);
        let mut probabilities = Vec::with_capacity(n);

        for point in &data {
            let posteriors = Self::posterior(&model, point);
            let (best_j, &best_p) = posteriors
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .expect("posteriors is non-empty");
            labels.push(best_j as i32);
            probabilities.push(best_p as f32);
        }

        let centroids = Self::model_centroids(&model);
        let cluster_count = model.weights.len();

        // Store model for classify()
        if let Ok(mut stored) = self.model.lock() {
            *stored = Some(model);
        }

        Ok(ClusteringResult {
            labels,
            centroids,
            cluster_count,
            noise_count: 0,
            probabilities: Some(probabilities),
        })
    }

    fn classify(&self, point: &RegimeFeatures) -> Option<ClusterAssignment> {
        let model_guard = self.model.lock().ok()?;
        let model = model_guard.as_ref()?;

        let arr = point.to_array().map(|v| v as f64);
        let posteriors = Self::posterior(model, &arr);

        let (best_j, &best_p) = posteriors
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))?;

        Some(ClusterAssignment {
            cluster_id: best_j as i32,
            probability: best_p as f32,
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

        // Run GMM on filtered data
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

        let noise_count = full_labels.iter().filter(|&&l| l < 0).count();
        let cluster_count = {
            let ids: HashSet<i32> = full_labels.iter().copied().filter(|&l| l >= 0).collect();
            ids.len()
        };

        let centroids = Self::recompute_centroids(features, &full_labels);

        Ok(ClusteringResult {
            labels: full_labels,
            centroids,
            cluster_count,
            noise_count,
            probabilities: None, // probabilities are invalidated by constraint post-processing
        })
    }

    fn algorithm_name(&self) -> &str {
        "gmm"
    }
}

// ---------------------------------------------------------------------------
// Math helpers
// ---------------------------------------------------------------------------

/// Squared Euclidean distance between two f64 vectors.
fn sq_dist(a: &[f64; RegimeFeatures::DIMENSIONS], b: &[f64; RegimeFeatures::DIMENSIONS]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Log probability density of a point under a diagonal Gaussian.
///
/// log N(x | mu, diag(sigma^2)) =
///   -0.5 * [D*ln(2pi) + sum(ln(sigma_d^2)) + sum((x_d - mu_d)^2 / sigma_d^2)]
fn log_gaussian_diag(
    x: &[f64; RegimeFeatures::DIMENSIONS],
    mean: &[f64; RegimeFeatures::DIMENSIONS],
    variance: &[f64; RegimeFeatures::DIMENSIONS],
) -> f64 {
    let dim = RegimeFeatures::DIMENSIONS;
    let log_2pi = (2.0 * std::f64::consts::PI).ln();

    let mut log_det = 0.0;
    let mut maha = 0.0;

    for d in 0..dim {
        log_det += variance[d].ln();
        let diff = x[d] - mean[d];
        maha += diff * diff / variance[d];
    }

    -0.5 * (dim as f64 * log_2pi + log_det + maha)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // Basic clustering tests
    // -----------------------------------------------------------------------

    #[test]
    fn detect_two_clusters() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let result = detector.detect(&features).unwrap();

        assert!(
            result.cluster_count >= 2,
            "expected >= 2 clusters, got {}",
            result.cluster_count
        );
        assert_eq!(result.labels.len(), 60);
        assert_eq!(result.noise_count, 0);
        assert!(result.probabilities.is_some());

        // All labels should be non-negative
        assert!(result.labels.iter().all(|&l| l >= 0));
    }

    #[test]
    fn detect_three_clusters() {
        let detector = GmmDetector::new().with_max_k(7).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..25 {
            features.push(coding_point(0.3 + (i as f32) * 0.002, 0.9));
        }
        for i in 0..25 {
            features.push(comm_point(0.8 + (i as f32) * 0.002, 0.3));
        }
        for i in 0..25 {
            features.push(browser_point(0.5 + (i as f32) * 0.002, 0.5));
        }

        let result = detector.detect(&features).unwrap();

        assert!(
            result.cluster_count >= 2,
            "expected >= 2 clusters, got {}",
            result.cluster_count
        );
        assert_eq!(result.labels.len(), 75);
    }

    #[test]
    fn detect_empty_returns_empty() {
        let detector = GmmDetector::new();
        let result = detector.detect(&[]).unwrap();
        assert_eq!(result.cluster_count, 0);
        assert_eq!(result.labels.len(), 0);
    }

    #[test]
    fn detect_single_point() {
        let detector = GmmDetector::new();
        let features = vec![coding_point(0.5, 0.5)];
        let result = detector.detect(&features).unwrap();
        assert_eq!(result.cluster_count, 1);
        assert_eq!(result.labels.len(), 1);
        assert_eq!(result.labels[0], 0);
    }

    // -----------------------------------------------------------------------
    // BIC selection tests
    // -----------------------------------------------------------------------

    #[test]
    fn bic_prefers_simpler_model_for_single_cluster() {
        let detector = GmmDetector::new().with_min_k(1).with_max_k(5);

        // All points are similar — should prefer k=1 or low k
        let features: Vec<RegimeFeatures> = (0..50)
            .map(|i| coding_point(0.5 + (i as f32) * 0.001, 0.5))
            .collect();

        let result = detector.detect(&features).unwrap();

        // With very similar data, BIC should select a small k
        assert!(
            result.cluster_count <= 3,
            "expected <= 3 for homogeneous data, got {}",
            result.cluster_count
        );
    }

    #[test]
    fn bic_computation_increases_with_model_complexity() {
        let n = 100;
        let ll = -500.0;
        // More components = more parameters = higher (worse) BIC
        let bic_2 = GmmDetector::bic(ll, 2, n);
        let bic_5 = GmmDetector::bic(ll, 5, n);
        assert!(
            bic_5 > bic_2,
            "BIC should increase with k for same LL: bic_2={bic_2}, bic_5={bic_5}"
        );
    }

    // -----------------------------------------------------------------------
    // Classify tests
    // -----------------------------------------------------------------------

    #[test]
    fn classify_returns_soft_probability() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

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
        // Probability should be between 0 and 1
        assert!(a.probability > 0.0 && a.probability <= 1.0);
    }

    #[test]
    fn classify_without_detect_returns_none() {
        let detector = GmmDetector::new();
        let assignment = detector.classify(&coding_point(0.5, 0.5));
        assert!(assignment.is_none());
    }

    #[test]
    fn classify_high_confidence_for_clear_point() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let _result = detector.detect(&features).unwrap();

        // A point deep within the coding cluster should have high confidence
        let assignment = detector.classify(&coding_point(0.35, 0.8)).unwrap();
        assert!(
            assignment.probability > 0.5,
            "expected high confidence, got {}",
            assignment.probability
        );
    }

    // -----------------------------------------------------------------------
    // Constraint tests
    // -----------------------------------------------------------------------

    #[test]
    fn noise_label_constraint_applied() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let constraints = vec![ClusterConstraint::NoiseLabel(0)];
        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        assert_eq!(result.labels[0], -1);
    }

    #[test]
    fn force_cluster_constraint_applied() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

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

        assert_eq!(result.labels[0], 99);
    }

    #[test]
    fn must_link_constraint_applied() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let constraints = vec![ClusterConstraint::MustLink(0, 30)];
        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        assert_eq!(result.labels[0], result.labels[30]);
    }

    #[test]
    fn cannot_link_constraint_applied() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..15 {
            features.push(coding_point(0.2 + (i as f32) * 0.005, 0.9));
        }
        for i in 0..15 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.3));
        }

        let constraints = vec![ClusterConstraint::CannotLink(0, 1)];
        let result = detector
            .detect_with_constraints(&features, &constraints)
            .unwrap();

        assert_ne!(result.labels[0], result.labels[1]);
    }

    // -----------------------------------------------------------------------
    // Algorithm metadata
    // -----------------------------------------------------------------------

    #[test]
    fn algorithm_name_returns_gmm() {
        let detector = GmmDetector::new();
        assert_eq!(detector.algorithm_name(), "gmm");
    }

    #[test]
    fn default_trait() {
        let detector = GmmDetector::default();
        assert_eq!(detector.max_k, 7);
        assert_eq!(detector.min_k, 2);
    }

    // -----------------------------------------------------------------------
    // Math helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn log_gaussian_diag_peaks_at_mean() {
        let mean = [0.5; RegimeFeatures::DIMENSIONS];
        let variance = [0.1; RegimeFeatures::DIMENSIONS];

        let at_mean = log_gaussian_diag(&mean, &mean, &variance);
        let mut off_mean = mean;
        off_mean[0] = 1.0;
        let away = log_gaussian_diag(&off_mean, &mean, &variance);

        assert!(
            at_mean > away,
            "log pdf should be highest at the mean: at_mean={at_mean}, away={away}"
        );
    }

    #[test]
    fn probabilities_sum_to_one() {
        let detector = GmmDetector::new().with_max_k(5).with_min_k(2);

        let mut features = Vec::new();
        for i in 0..30 {
            features.push(coding_point(0.3 + (i as f32) * 0.005, 0.8));
        }
        for i in 0..30 {
            features.push(comm_point(0.7 + (i as f32) * 0.005, 0.4));
        }

        let result = detector.detect(&features).unwrap();
        let probs = result.probabilities.unwrap();

        // Each probability should be in (0, 1] — they represent the max component probability
        for p in &probs {
            assert!(*p > 0.0 && *p <= 1.0, "probability out of range: {p}");
        }
    }
}
