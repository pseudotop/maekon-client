//! Bayesian Gaussian Mixture Model (GMM) detector implementing `ClusteringStrategy`.
//!
//! Uses Expectation-Maximization with diagonal covariance for efficiency.
//! K selection via BIC (Bayesian Information Criterion). Pure Rust
//! implementation — no external crate needed.
//!
//! # Module layout
//! - `model`     — `GmmModel` parameter struct
//! - `math`      — `sq_dist`, `log_gaussian_diag`
//! - `training`  — `EmTrainer`: `fit_em`, E/M steps, BIC
//! - `detection` — `ClusteringStrategy` impl, posterior, centroid helpers
//! - `tests`     — integration tests

mod detection;
mod math;
mod model;
mod training;

#[cfg(test)]
mod tests;

use std::sync::Mutex;

use model::GmmModel;
use training::EmTrainer;

/// Gaussian Mixture Model detector for regime detection.
///
/// Fits a GMM using EM with diagonal covariance and selects K via BIC.
/// Provides soft (probabilistic) cluster assignments through `classify()`.
pub struct GmmDetector {
    /// Maximum number of components to evaluate.
    pub(in crate::gmm_detector) max_k: usize,
    /// Minimum number of components to evaluate.
    pub(in crate::gmm_detector) min_k: usize,
    /// EM algorithm trainer (holds iteration/convergence hyperparameters).
    pub(in crate::gmm_detector) trainer: EmTrainer,
    /// Stored model from the last `detect()` call, for `classify()`.
    pub(in crate::gmm_detector) model: Mutex<Option<GmmModel>>,
}

impl GmmDetector {
    /// Create a new GMM detector with default settings.
    pub fn new() -> Self {
        Self {
            max_k: 7,
            min_k: 2,
            trainer: EmTrainer {
                max_iterations: 100,
                epsilon: 1e-6,
                variance_floor: 1e-6,
            },
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
}
