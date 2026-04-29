use flate2::read::{GzDecoder, GzEncoder};
use flate2::Compression;
use oneshim_core::error::CoreError;
use oneshim_core::ports::compressor::{CompressionAlgorithm, Compressor};
use std::io::Read;

pub struct AdaptiveCompressor;

impl AdaptiveCompressor {
    pub fn new() -> Self {
        Self
    }

    pub fn select_algorithm(data_size: usize) -> CompressionAlgorithm {
        if data_size < 1024 {
            CompressionAlgorithm::Lz4
        } else if data_size < 100 * 1024 {
            CompressionAlgorithm::Zstd
        } else {
            CompressionAlgorithm::Gzip
        }
    }

    pub fn compress_auto(&self, data: &[u8]) -> Result<(Vec<u8>, CompressionAlgorithm), CoreError> {
        let algo = Self::select_algorithm(data.len());
        let compressed = self.compress(data, algo)?;
        Ok((compressed, algo))
    }
}

impl Default for AdaptiveCompressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor for AdaptiveCompressor {
    fn compress(&self, data: &[u8], algorithm: CompressionAlgorithm) -> Result<Vec<u8>, CoreError> {
        match algorithm {
            CompressionAlgorithm::Gzip => {
                let mut encoder = GzEncoder::new(data, Compression::default());
                let mut compressed = Vec::new();
                encoder
                    .read_to_end(&mut compressed)
                    .map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("Gzip compression failed: {e}"),
                    })?;
                Ok(compressed)
            }
            CompressionAlgorithm::Zstd => {
                zstd::encode_all(data, 3).map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("Zstd compression failed: {e}"),
                })
            }
            CompressionAlgorithm::Lz4 => Ok(lz4_flex::compress_prepend_size(data)),
        }
    }

    fn decompress(
        &self,
        data: &[u8],
        algorithm: CompressionAlgorithm,
    ) -> Result<Vec<u8>, CoreError> {
        match algorithm {
            CompressionAlgorithm::Gzip => {
                let mut decoder = GzDecoder::new(data);
                let mut decompressed = Vec::new();
                decoder
                    .read_to_end(&mut decompressed)
                    .map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("Gzip decompression failed: {e}"),
                    })?;
                Ok(decompressed)
            }
            CompressionAlgorithm::Zstd => zstd::decode_all(data).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Zstd decompression failed: {e}"),
            }),
            CompressionAlgorithm::Lz4 => {
                lz4_flex::decompress_size_prepended(data).map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("Lz4 decompression failed: {e}"),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_roundtrip() {
        let compressor = AdaptiveCompressor::new();
        let data = b"Hello ONESHIM! This is test data for compression.";
        let compressed = compressor
            .compress(data, CompressionAlgorithm::Gzip)
            .unwrap();
        let decompressed = compressor
            .decompress(&compressed, CompressionAlgorithm::Gzip)
            .unwrap();
        assert_eq!(data.to_vec(), decompressed);
    }

    #[test]
    fn zstd_roundtrip() {
        let compressor = AdaptiveCompressor::new();
        let data = b"Zstandard compression test data for ONESHIM client.";
        let compressed = compressor
            .compress(data, CompressionAlgorithm::Zstd)
            .unwrap();
        let decompressed = compressor
            .decompress(&compressed, CompressionAlgorithm::Zstd)
            .unwrap();
        assert_eq!(data.to_vec(), decompressed);
    }

    #[test]
    fn lz4_roundtrip() {
        let compressor = AdaptiveCompressor::new();
        let data = b"LZ4 fast compression test.";
        let compressed = compressor
            .compress(data, CompressionAlgorithm::Lz4)
            .unwrap();
        let decompressed = compressor
            .decompress(&compressed, CompressionAlgorithm::Lz4)
            .unwrap();
        assert_eq!(data.to_vec(), decompressed);
    }

    #[test]
    fn algorithm_auto_selection() {
        assert_eq!(
            AdaptiveCompressor::select_algorithm(500),
            CompressionAlgorithm::Lz4
        );
        assert_eq!(
            AdaptiveCompressor::select_algorithm(50_000),
            CompressionAlgorithm::Zstd
        );
        assert_eq!(
            AdaptiveCompressor::select_algorithm(200_000),
            CompressionAlgorithm::Gzip
        );
    }

    #[test]
    fn compress_auto() {
        let compressor = AdaptiveCompressor::new();
        let data = vec![0u8; 50_000]; // medium size -> Zstd
        let (compressed, algo) = compressor.compress_auto(&data).unwrap();
        assert_eq!(algo, CompressionAlgorithm::Zstd);
        let decompressed = compressor.decompress(&compressed, algo).unwrap();
        assert_eq!(data, decompressed);
    }

    #[test]
    fn empty_data() {
        let compressor = AdaptiveCompressor::new();
        let data = b"";
        for algo in [
            CompressionAlgorithm::Gzip,
            CompressionAlgorithm::Zstd,
            CompressionAlgorithm::Lz4,
        ] {
            let compressed = compressor.compress(data, algo).unwrap();
            let decompressed = compressor.decompress(&compressed, algo).unwrap();
            assert_eq!(data.to_vec(), decompressed);
        }
    }

    #[test]
    fn wrong_algorithm_gzip_as_zstd() {
        let compressor = AdaptiveCompressor::new();
        let data = b"test data for cross-algorithm check";
        let compressed = compressor
            .compress(data, CompressionAlgorithm::Gzip)
            .unwrap();
        let result = compressor.decompress(&compressed, CompressionAlgorithm::Zstd);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_algorithm_lz4_as_gzip() {
        let compressor = AdaptiveCompressor::new();
        let data = b"cross algorithm test data lz4";
        let compressed = compressor
            .compress(data, CompressionAlgorithm::Lz4)
            .unwrap();
        let result = compressor.decompress(&compressed, CompressionAlgorithm::Gzip);
        assert!(result.is_err());
    }

    #[test]
    fn corrupted_data_gzip() {
        let compressor = AdaptiveCompressor::new();
        let corrupted = vec![0xFF, 0xFE, 0x00, 0x01, 0x02, 0x03];
        let result = compressor.decompress(&corrupted, CompressionAlgorithm::Gzip);
        assert!(result.is_err());
    }

    #[test]
    fn corrupted_data_zstd() {
        let compressor = AdaptiveCompressor::new();
        let corrupted = vec![0xAB, 0xCD, 0xEF, 0x00, 0x11, 0x22];
        let result = compressor.decompress(&corrupted, CompressionAlgorithm::Zstd);
        assert!(result.is_err());
    }

    #[test]
    fn corrupted_data_lz4() {
        let compressor = AdaptiveCompressor::new();
        let data = b"valid test data for lz4 corruption test";
        let mut compressed = compressor
            .compress(data, CompressionAlgorithm::Lz4)
            .unwrap();
        for byte in compressed.iter_mut().skip(4) {
            *byte ^= 0xFF;
        }
        let result = compressor.decompress(&compressed, CompressionAlgorithm::Lz4);
        assert!(result.is_err());
    }
}
