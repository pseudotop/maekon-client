//! Criterion benchmarks for oneshim-core quantization primitives.

use criterion::{criterion_group, criterion_main, Criterion};
use oneshim_core::binary_quantizer::{BinaryQuantizer, QuantileThresholds};
use oneshim_core::quantization::ScalarQuantizer;
use std::hint::black_box;

/// Generate a deterministic pseudo-random f32 vector of `len` dimensions.
/// Uses a simple LCG to avoid pulling in `rand` as a bench dependency.
fn pseudo_random_vec(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            // LCG: x_{n+1} = (a * x_n + c) mod m
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            // Map to [-1.0, 1.0]
            (state >> 33) as f32 / (u32::MAX as f32 / 2.0) - 1.0
        })
        .collect()
}

fn build_thresholds_384() -> QuantileThresholds {
    let vectors: Vec<Vec<f32>> = (0..100).map(|i| pseudo_random_vec(384, i)).collect();
    BinaryQuantizer::compute_thresholds(&vectors, 384).unwrap()
}

// ── Scalar quantization benchmarks ───────────────────────────────────────────

fn bench_scalar_quantize(c: &mut Criterion) {
    let vec_384 = pseudo_random_vec(384, 42);

    c.bench_function("ScalarQuantizer::quantize (384-dim)", |b| {
        b.iter(|| ScalarQuantizer::quantize(black_box(&vec_384)))
    });
}

fn bench_scalar_dequantize(c: &mut Criterion) {
    let vec_384 = pseudo_random_vec(384, 42);
    let qv = ScalarQuantizer::quantize(&vec_384).unwrap();

    c.bench_function("ScalarQuantizer::dequantize (384-dim)", |b| {
        b.iter(|| ScalarQuantizer::dequantize(black_box(&qv)))
    });
}

fn bench_cosine_similarity_int8(c: &mut Criterion) {
    let a = ScalarQuantizer::quantize(&pseudo_random_vec(384, 1)).unwrap();
    let b = ScalarQuantizer::quantize(&pseudo_random_vec(384, 2)).unwrap();

    c.bench_function(
        "ScalarQuantizer::cosine_similarity_int8_unchecked (384-dim)",
        |bench| {
            bench.iter(|| {
                ScalarQuantizer::cosine_similarity_int8_unchecked(black_box(&a), black_box(&b))
            })
        },
    );
}

// ── Binary quantization benchmarks ───────────────────────────────────────────

fn bench_binary_encode(c: &mut Criterion) {
    let thresholds = build_thresholds_384();
    let vec_384 = pseudo_random_vec(384, 99);

    c.bench_function("BinaryQuantizer::encode (384-dim)", |b| {
        b.iter(|| BinaryQuantizer::encode(black_box(&vec_384), black_box(&thresholds)))
    });
}

fn bench_hamming_distance(c: &mut Criterion) {
    let thresholds = build_thresholds_384();
    let code_a = BinaryQuantizer::encode(&pseudo_random_vec(384, 10), &thresholds).unwrap();
    let code_b = BinaryQuantizer::encode(&pseudo_random_vec(384, 20), &thresholds).unwrap();

    c.bench_function(
        "BinaryQuantizer::hamming_distance (384-dim / 96 bytes)",
        |b| b.iter(|| BinaryQuantizer::hamming_distance(black_box(&code_a), black_box(&code_b))),
    );
}

criterion_group!(
    benches,
    bench_scalar_quantize,
    bench_scalar_dequantize,
    bench_cosine_similarity_int8,
    bench_binary_encode,
    bench_hamming_distance,
);
criterion_main!(benches);
