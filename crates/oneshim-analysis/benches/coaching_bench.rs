//! Criterion benchmarks for coaching adaptive scorer and feedback operations.
//!
//! Run with: `cargo bench -p oneshim-analysis -- coaching`

use criterion::{criterion_group, criterion_main, Criterion};
use oneshim_analysis::coaching_engine::adaptive_scorer::{AdaptiveScorer, CoachingFeatures};
use oneshim_analysis::coaching_engine::tunable_params::TunableParams;
use std::hint::black_box;

fn make_features() -> CoachingFeatures {
    CoachingFeatures::extract(14, 3600, 5, 0.6, 0.5, false, 2, 1800)
}

fn bench_adaptive_scorer(c: &mut Criterion) {
    let mut group = c.benchmark_group("adaptive_scorer");

    // Predict (untrained — baseline)
    group.bench_function("predict_untrained", |b| {
        let scorer = AdaptiveScorer::default();
        let features = make_features();
        b.iter(|| black_box(scorer.predict(&features)));
    });

    // Predict (trained — 100 samples)
    group.bench_function("predict_trained_100", |b| {
        let mut scorer = AdaptiveScorer::default();
        let features = make_features();
        for _ in 0..100 {
            scorer.update(&features, 1.0);
        }
        b.iter(|| black_box(scorer.predict(&features)));
    });

    // Single SGD update
    group.bench_function("update_single", |b| {
        let mut scorer = AdaptiveScorer::default();
        let features = make_features();
        b.iter(|| {
            scorer.update(&features, black_box(1.0));
        });
    });

    // Feature extraction
    group.bench_function("feature_extract", |b| {
        b.iter(|| {
            black_box(CoachingFeatures::extract(
                black_box(14),
                black_box(3600),
                black_box(5),
                black_box(0.6),
                black_box(0.5),
                black_box(false),
                black_box(2),
                black_box(1800),
            ))
        });
    });

    group.finish();
}

fn bench_tunable_params(c: &mut Criterion) {
    let mut group = c.benchmark_group("tunable_params");

    group.bench_function("adjust_negative", |b| {
        let mut params = TunableParams::default();
        b.iter(|| {
            params.adjust_on_feedback(black_box(false));
        });
    });

    group.bench_function("adjust_positive", |b| {
        let mut params = TunableParams::default();
        b.iter(|| {
            params.adjust_on_feedback(black_box(true));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_adaptive_scorer, bench_tunable_params);
criterion_main!(benches);
