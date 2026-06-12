//! EM training: initialization, E-step, M-step, BIC, and `fit_em` orchestration.

use maekon_core::models::tiered_memory::RegimeFeatures;

use super::math::{log_gaussian_diag, sq_dist};
use super::model::GmmModel;

/// Core EM training methods, implemented as free functions that take
/// hyperparameters explicitly so they stay pure and testable.
pub(super) struct EmTrainer {
    pub(super) max_iterations: usize,
    pub(super) epsilon: f64,
    pub(super) variance_floor: f64,
}

impl EmTrainer {
    /// Run EM for a fixed K and return the fitted model + final log-likelihood.
    pub(super) fn fit_em(
        &self,
        data: &[[f64; RegimeFeatures::DIMENSIONS]],
        k: usize,
    ) -> (GmmModel, f64) {
        let n = data.len();

        let (mut means, mut variances, mut weights) = self.initialize(data, k);

        let mut log_likelihood = f64::NEG_INFINITY;
        let mut responsibilities = vec![vec![0.0f64; k]; n];

        for _iter in 0..self.max_iterations {
            let new_ll = self.e_step(data, &weights, &means, &variances, &mut responsibilities);

            if (new_ll - log_likelihood).abs() < self.epsilon {
                log_likelihood = new_ll;
                break;
            }
            log_likelihood = new_ll;

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
            let nk: f64 = responsibilities.iter().map(|r| r[j]).sum();

            if nk < 1e-10 {
                // Empty component — keep previous parameters
                weights[j] = 1e-10;
                continue;
            }

            weights[j] = nk / n;

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
    pub(super) fn bic(log_likelihood: f64, k: usize, n: usize) -> f64 {
        let dim = RegimeFeatures::DIMENSIONS;
        // Parameters: k weights (k-1 free) + k*dim means + k*dim variances
        let num_params = (k - 1) + k * dim + k * dim;
        -2.0 * log_likelihood + (num_params as f64) * (n as f64).ln()
    }
}
