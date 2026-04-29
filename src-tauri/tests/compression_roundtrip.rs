#![cfg(feature = "server")]

use oneshim_core::ports::compressor::{CompressionAlgorithm, Compressor};
use oneshim_network::compression::AdaptiveCompressor;

fn make_test_data(size: usize) -> Vec<u8> {
    let pattern = b"Maekon context data payload with repetitive content for testing. ";
    pattern.iter().cycle().take(size).cloned().collect()
}

#[test]
fn gzip_roundtrip_integration() {
    let compressor = AdaptiveCompressor::new();
    let data = make_test_data(10_000);

    let compressed = compressor
        .compress(&data, CompressionAlgorithm::Gzip)
        .unwrap();
    let decompressed = compressor
        .decompress(&compressed, CompressionAlgorithm::Gzip)
        .unwrap();

    assert_eq!(data, decompressed);
    assert!(
        compressed.len() < data.len(),
        "gzip should compress the data"
    );
}

#[test]
fn zstd_roundtrip_integration() {
    let compressor = AdaptiveCompressor::new();
    let data = make_test_data(50_000);

    let compressed = compressor
        .compress(&data, CompressionAlgorithm::Zstd)
        .unwrap();
    let decompressed = compressor
        .decompress(&compressed, CompressionAlgorithm::Zstd)
        .unwrap();

    assert_eq!(data, decompressed);
    assert!(
        compressed.len() < data.len(),
        "zstd should compress the data"
    );
}

#[test]
fn lz4_roundtrip_integration() {
    let compressor = AdaptiveCompressor::new();
    let data = make_test_data(500);

    let compressed = compressor
        .compress(&data, CompressionAlgorithm::Lz4)
        .unwrap();
    let decompressed = compressor
        .decompress(&compressed, CompressionAlgorithm::Lz4)
        .unwrap();

    assert_eq!(data, decompressed);
}

#[test]
fn auto_selection_by_size() {
    let compressor = AdaptiveCompressor::new();

    let small = make_test_data(100);
    let (compressed, algo) = compressor.compress_auto(&small).unwrap();
    assert_eq!(algo, CompressionAlgorithm::Lz4);
    let decompressed = compressor.decompress(&compressed, algo).unwrap();
    assert_eq!(small, decompressed);

    let medium = make_test_data(10_000);
    let (compressed, algo) = compressor.compress_auto(&medium).unwrap();
    assert_eq!(algo, CompressionAlgorithm::Zstd);
    let decompressed = compressor.decompress(&compressed, algo).unwrap();
    assert_eq!(medium, decompressed);

    let large = make_test_data(200_000);
    let (compressed, algo) = compressor.compress_auto(&large).unwrap();
    assert_eq!(algo, CompressionAlgorithm::Gzip);
    let decompressed = compressor.decompress(&compressed, algo).unwrap();
    assert_eq!(large, decompressed);
}

#[test]
fn all_algorithms_handle_empty_data() {
    let compressor = AdaptiveCompressor::new();

    for algo in [
        CompressionAlgorithm::Gzip,
        CompressionAlgorithm::Zstd,
        CompressionAlgorithm::Lz4,
    ] {
        let compressed = compressor.compress(&[], algo).unwrap();
        let decompressed = compressor.decompress(&compressed, algo).unwrap();
        assert!(
            decompressed.is_empty(),
            "{algo:?} empty-data roundtrip failed"
        );
    }
}

#[test]
fn compression_ratios_vary_by_algorithm() {
    let compressor = AdaptiveCompressor::new();
    let data = make_test_data(50_000);

    let gzip = compressor
        .compress(&data, CompressionAlgorithm::Gzip)
        .unwrap();
    let zstd = compressor
        .compress(&data, CompressionAlgorithm::Zstd)
        .unwrap();
    let lz4 = compressor
        .compress(&data, CompressionAlgorithm::Lz4)
        .unwrap();

    assert!(!gzip.is_empty());
    assert!(!zstd.is_empty());
    assert!(!lz4.is_empty());

    assert!(gzip.len() < data.len());
    assert!(zstd.len() < data.len());
    assert!(lz4.len() < data.len());
}
