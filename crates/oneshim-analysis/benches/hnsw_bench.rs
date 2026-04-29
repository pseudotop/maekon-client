//! Criterion benchmarks for HNSW approximate nearest neighbor operations.
//!
//! Run with: `cargo bench -p oneshim-analysis --features hnsw`

use criterion::{criterion_group, criterion_main, Criterion};
use oneshim_analysis::HnswAdapter;
use oneshim_core::ports::ann_index::AnnIndex;
use rand::{Rng, RngExt};

const DIMS: usize = 384;

/// Generate a random f32 vector of the given dimensionality.
fn random_vector(rng: &mut impl Rng, dims: usize) -> Vec<f32> {
    (0..dims)
        .map(|_| rng.random_range(0.0f32..1.0f32))
        .collect()
}

/// Build an HNSW index with `n` random vectors and return the adapter.
fn build_index(n: usize) -> HnswAdapter {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("bench_index.usearch");
    let adapter = HnswAdapter::new(DIMS, path).expect("HnswAdapter::new");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut rng = rand::rng();

    rt.block_on(async {
        for key in 0..n as u64 {
            let vec = random_vector(&mut rng, DIMS);
            adapter.add(key, &vec).await.expect("add");
        }
    });

    // Leak the TempDir so the index files remain valid during the benchmark.
    std::mem::forget(dir);
    adapter
}

fn hnsw_add_single(c: &mut Criterion) {
    let adapter = build_index(10_000);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut rng = rand::rng();
    let mut next_key = 10_000u64;

    c.bench_function("hnsw_add_single_10k", |b| {
        b.iter(|| {
            let vec = random_vector(&mut rng, DIMS);
            let key = next_key;
            next_key += 1;
            rt.block_on(async {
                adapter.add(key, &vec).await.expect("add");
            });
        });
    });
}

fn hnsw_search_top10_10k(c: &mut Criterion) {
    let adapter = build_index(10_000);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut rng = rand::rng();

    c.bench_function("hnsw_search_top10_10k", |b| {
        b.iter(|| {
            let query = random_vector(&mut rng, DIMS);
            rt.block_on(async {
                let _results = adapter.search(&query, 10).await.expect("search");
            });
        });
    });
}

fn hnsw_search_top10_50k(c: &mut Criterion) {
    let adapter = build_index(50_000);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut rng = rand::rng();

    c.bench_function("hnsw_search_top10_50k", |b| {
        b.iter(|| {
            let query = random_vector(&mut rng, DIMS);
            rt.block_on(async {
                let _results = adapter.search(&query, 10).await.expect("search");
            });
        });
    });
}

criterion_group!(
    benches,
    hnsw_add_single,
    hnsw_search_top10_10k,
    hnsw_search_top10_50k
);
criterion_main!(benches);
