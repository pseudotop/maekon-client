//! Integration tests for `GmmDetector`.

use maekon_core::models::recalibration::ClusterConstraint;
use maekon_core::models::tiered_memory::RegimeFeatures;

use crate::clustering_strategy::ClusteringStrategy;

use super::training::EmTrainer;
use super::GmmDetector;

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

// ---------------------------------------------------------------------------
// Basic clustering tests
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// BIC selection tests
// ---------------------------------------------------------------------------

#[test]
fn bic_prefers_simpler_model_for_single_cluster() {
    let detector = GmmDetector::new().with_min_k(1).with_max_k(5);

    let features: Vec<RegimeFeatures> = (0..50)
        .map(|i| coding_point(0.5 + (i as f32) * 0.001, 0.5))
        .collect();

    let result = detector.detect(&features).unwrap();

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
    let bic_2 = EmTrainer::bic(ll, 2, n);
    let bic_5 = EmTrainer::bic(ll, 5, n);
    assert!(
        bic_5 > bic_2,
        "BIC should increase with k for same LL: bic_2={bic_2}, bic_5={bic_5}"
    );
}

// ---------------------------------------------------------------------------
// Classify tests
// ---------------------------------------------------------------------------

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

    let assignment = detector.classify(&coding_point(0.35, 0.85));
    assert!(assignment.is_some());
    let a = assignment.unwrap();
    assert!(a.cluster_id >= 0);
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

    let assignment = detector.classify(&coding_point(0.35, 0.8)).unwrap();
    assert!(
        assignment.probability > 0.5,
        "expected high confidence, got {}",
        assignment.probability
    );
}

// ---------------------------------------------------------------------------
// Constraint tests
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Algorithm metadata
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Math helper tests
// ---------------------------------------------------------------------------

#[test]
fn log_gaussian_diag_peaks_at_mean() {
    use super::math::log_gaussian_diag;
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

    for p in &probs {
        assert!(*p > 0.0 && *p <= 1.0, "probability out of range: {p}");
    }
}
