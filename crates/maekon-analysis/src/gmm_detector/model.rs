//! GMM model types: `GmmModel` storing learned parameters.

use maekon_core::models::tiered_memory::RegimeFeatures;

/// A fitted GMM model storing the learned parameters.
#[derive(Debug, Clone)]
pub(super) struct GmmModel {
    /// Mixing weights (sum to 1.0).
    pub(super) weights: Vec<f64>,
    /// Mean vectors per component.
    pub(super) means: Vec<[f64; RegimeFeatures::DIMENSIONS]>,
    /// Diagonal covariance per component (variance per dimension).
    pub(super) variances: Vec<[f64; RegimeFeatures::DIMENSIONS]>,
}
