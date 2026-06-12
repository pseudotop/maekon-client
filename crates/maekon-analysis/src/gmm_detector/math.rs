//! Math helpers: squared distance and diagonal Gaussian log-density.

use maekon_core::models::tiered_memory::RegimeFeatures;

/// Squared Euclidean distance between two f64 vectors.
pub(super) fn sq_dist(
    a: &[f64; RegimeFeatures::DIMENSIONS],
    b: &[f64; RegimeFeatures::DIMENSIONS],
) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Log probability density of a point under a diagonal Gaussian.
///
/// log N(x | mu, diag(sigma^2)) =
///   -0.5 * [D*ln(2pi) + sum(ln(sigma_d^2)) + sum((x_d - mu_d)^2 / sigma_d^2)]
pub(super) fn log_gaussian_diag(
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
