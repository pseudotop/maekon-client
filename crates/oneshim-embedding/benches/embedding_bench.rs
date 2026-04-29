//! Criterion benchmarks for oneshim-embedding.
//!
//! The `LocalEmbeddingProvider` requires downloading an ONNX model from the
//! network, so we only benchmark the utility functions re-exported from
//! oneshim-core (scalar quantization + cosine similarity) that are used
//! internally by the embedding pipeline.

use criterion::{criterion_group, criterion_main, Criterion};
use oneshim_core::quantization::ScalarQuantizer;
use std::hint::black_box;

/// Generate a deterministic pseudo-random f32 vector of `len` dimensions.
fn pseudo_random_vec(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            (state >> 33) as f32 / (u32::MAX as f32 / 2.0) - 1.0
        })
        .collect()
}

fn bench_quantize_roundtrip(c: &mut Criterion) {
    let vec_384 = pseudo_random_vec(384, 42);

    c.bench_function(
        "embedding: quantize → dequantize roundtrip (384-dim)",
        |b| {
            b.iter(|| {
                let qv = ScalarQuantizer::quantize(black_box(&vec_384)).unwrap();
                let _reconstructed = ScalarQuantizer::dequantize(black_box(&qv));
            })
        },
    );
}

fn bench_batch_quantize(c: &mut Criterion) {
    let vectors: Vec<Vec<f32>> = (0..100).map(|i| pseudo_random_vec(384, i)).collect();

    c.bench_function("embedding: batch quantize 100 vectors (384-dim)", |b| {
        b.iter(|| {
            for v in black_box(&vectors) {
                let _ = ScalarQuantizer::quantize(v);
            }
        })
    });
}

fn bench_batch_cosine_similarity(c: &mut Criterion) {
    let vectors: Vec<_> = (0..100)
        .map(|i| ScalarQuantizer::quantize(&pseudo_random_vec(384, i)).unwrap())
        .collect();
    let query = ScalarQuantizer::quantize(&pseudo_random_vec(384, 999)).unwrap();

    c.bench_function("embedding: cosine_similarity_int8 x 100 (384-dim)", |b| {
        b.iter(|| {
            for v in black_box(&vectors) {
                let _ = ScalarQuantizer::cosine_similarity_int8_unchecked(
                    black_box(&query),
                    black_box(v),
                );
            }
        })
    });
}

criterion_group!(
    benches,
    bench_quantize_roundtrip,
    bench_batch_quantize,
    bench_batch_cosine_similarity,
);
criterion_main!(benches);
