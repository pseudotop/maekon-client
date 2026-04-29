use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use oneshim_core::ports::compressor::{CompressionAlgorithm, Compressor};
use oneshim_network::compression::AdaptiveCompressor;

fn create_compressible_data(size: usize) -> Vec<u8> {
    let pattern =
        b"ONESHIM event payload with repeated structure and timestamps 2026-01-31T12:00:00Z ";
    pattern.iter().cycle().take(size).copied().collect()
}

fn create_random_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| ((i * 17 + 31) % 256) as u8).collect()
}

fn bench_compress_by_algorithm(c: &mut Criterion) {
    let mut group = c.benchmark_group("compress_algorithm");
    let compressor = AdaptiveCompressor::new();

    let sizes = [1024, 10_240, 102_400]; // 1KB, 10KB, 100KB
    let algorithms = [
        ("gzip", CompressionAlgorithm::Gzip),
        ("zstd", CompressionAlgorithm::Zstd),
        ("lz4", CompressionAlgorithm::Lz4),
    ];

    for size in sizes {
        let data = create_compressible_data(size);
        group.throughput(Throughput::Bytes(size as u64));

        for (name, algo) in &algorithms {
            group.bench_with_input(
                BenchmarkId::new(*name, format!("{}KB", size / 1024)),
                &(&data, *algo),
                |b, (data, algo)| {
                    b.iter(|| black_box(compressor.compress(data, *algo).unwrap()));
                },
            );
        }
    }

    group.finish();
}

fn bench_compress_auto(c: &mut Criterion) {
    let mut group = c.benchmark_group("compress_auto");
    let compressor = AdaptiveCompressor::new();

    let sizes = [512, 2048, 16_384, 65_536]; // bytes
    for size in sizes {
        let data = create_compressible_data(size);
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(
            BenchmarkId::new("compressible", format!("{}B", size)),
            &data,
            |b, data| {
                b.iter(|| black_box(compressor.compress_auto(data).unwrap()));
            },
        );
    }

    let random_data = create_random_data(65_536);
    group.throughput(Throughput::Bytes(65_536));
    group.bench_function("random_64KB", |b| {
        b.iter(|| black_box(compressor.compress_auto(&random_data).unwrap()));
    });

    group.finish();
}

fn bench_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("compress_roundtrip");
    let compressor = AdaptiveCompressor::new();

    let data = create_compressible_data(65_536); // 64KB
    group.throughput(Throughput::Bytes(65_536));

    let algorithms = [
        ("gzip", CompressionAlgorithm::Gzip),
        ("zstd", CompressionAlgorithm::Zstd),
        ("lz4", CompressionAlgorithm::Lz4),
    ];

    for (name, algo) in &algorithms {
        let compressed = compressor.compress(&data, *algo).unwrap();

        group.bench_with_input(
            BenchmarkId::new(*name, "64KB"),
            &(&compressed, *algo),
            |b, (compressed, algo)| {
                b.iter(|| black_box(compressor.decompress(compressed, *algo).unwrap()));
            },
        );
    }

    group.finish();
}

fn bench_algorithm_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("algorithm_selection");

    let sizes = [100, 1_000, 10_000, 100_000, 1_000_000];

    for size in sizes {
        group.bench_with_input(
            BenchmarkId::new("select", format!("{}B", size)),
            &size,
            |b, &size| {
                b.iter(|| black_box(AdaptiveCompressor::select_algorithm(size)));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_compress_by_algorithm,
    bench_compress_auto,
    bench_roundtrip,
    bench_algorithm_selection
);
criterion_main!(benches);
