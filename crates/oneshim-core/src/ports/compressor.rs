//! Data compression port — defines the contract for compressing and
//! decompressing payloads using gzip, zstd, or lz4 algorithms.
//! Implemented by `AdaptiveCompressor` in `oneshim-network`.

use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    /// gzip (flate2)
    Gzip,
    /// Zstandard
    Zstd,
    Lz4,
}

/// Data compressor — gzip/zstd/lz4 payload transform.
///
/// # Errors
/// `CoreError::Internal` (wire: `internal.generic`) for codec-layer
/// failures: invalid input magic bytes, truncated buffers, library
/// panics from flate2/zstd/lz4. Invalid algorithm choice or zero-length
/// input is a caller-side contract violation; adapters may
/// debug_assert. No separate "bad-input" wire code exists — callers
/// that need to distinguish user-supplied corruption from internal
/// library failure should validate before invocation.
pub trait Compressor: Send + Sync {
    fn compress(&self, data: &[u8], algorithm: CompressionAlgorithm) -> Result<Vec<u8>, CoreError>;

    fn decompress(
        &self,
        data: &[u8],
        algorithm: CompressionAlgorithm,
    ) -> Result<Vec<u8>, CoreError>;
}
