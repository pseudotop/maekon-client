//! Detection and inference: posterior probabilities, centroid helpers,
//! and the `ClusteringStrategy` impl for `GmmDetector`.

use std::collections::{HashMap, HashSet};

use maekon_core::models::recalibration::ClusterConstraint;
use maekon_core::models::tiered_memory::RegimeFeatures;

use crate::clustering_strategy::{
    apply_link_constraints, filter_features, parse_constraints, reconstruct_labels,
    ClusterAssignment, ClusteringResult, ClusteringStrategy,
};
use crate::error::AnalysisError;

use super::math::log_gaussian_diag;
use super::model::GmmModel;
use super::GmmDetector;

impl Default for GmmDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl GmmDetector {
    /// Compute posterior probabilities for a single point against the stored model.
    pub(super) fn posterior(
        model: &GmmModel,
        point: &[f64; RegimeFeatures::DIMENSIONS],
    ) -> Vec<f64> {
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

    /// Recompute centroids as `RegimeFeatures` from model means.
    pub(super) fn model_centroids(model: &GmmModel) -> Vec<RegimeFeatures> {
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
    pub(super) fn recompute_centroids(
        features: &[RegimeFeatures],
        labels: &[i32],
    ) -> Vec<RegimeFeatures> {
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
            let (model, ll) = self.trainer.fit_em(&data, k);
            let bic_score = super::training::EmTrainer::bic(ll, k, n);

            if bic_score < best_bic {
                best_bic = bic_score;
                best_model = Some(model);
            }
        }

        let model = best_model
            .ok_or_else(|| AnalysisError::Clustering("GMM: failed to fit any model".to_string()))?;

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
            probabilities: None,
        })
    }

    fn algorithm_name(&self) -> &str {
        "gmm"
    }
}
