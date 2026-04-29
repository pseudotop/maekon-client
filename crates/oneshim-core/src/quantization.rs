//! Scalar INT8 quantization for embedding vectors.
//!
//! Converts f32 embedding vectors to i8 using per-vector min-max scaling.
//! ~4x storage reduction with ~99% recall preservation for 384-dim embeddings.
//!
//! See: docs/superpowers/specs/2026-03-19-p3-vector-compression-embedding-optimization-design.md

use crate::error::CoreError;
use serde::{Deserialize, Serialize};

/// A quantized embedding vector stored as INT8 with scale/offset for dequantization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizedVector {
    /// INT8-quantized embedding data (384 bytes for MiniLM-L6-v2).
    pub data: Vec<i8>,
    /// Scale factor: (max - min) / 255.0
    pub scale: f32,
    /// Offset: min value of original vector.
    pub offset: f32,
}

/// Stateless scalar quantizer for f32 → INT8 conversion.
pub struct ScalarQuantizer;

impl ScalarQuantizer {
    /// Quantize a f32 embedding vector to INT8.
    ///
    /// Edge cases:
    /// - Zero-length vector: returns `CoreError::InvalidArguments`
    ///   (wire: `validation.invalid_arguments`). Iter-95 corrected this
    ///   from `Internal` — input-validation failures aren't internal errors.
    /// - Constant vector (all same value): scale=1.0, offset=min, all INT8=0
    /// - NaN/Inf: rejected via `f32::is_finite()` pre-scan, returns
    ///   `CoreError::InvalidArguments`
    pub fn quantize(vector: &[f32]) -> Result<QuantizedVector, CoreError> {
        // Iter-95: input-validation errors use InvalidArguments consistent with
        // cosine_similarity_int8 below (wire code `validation.invalid_arguments`).
        // Pre-iter-95 these were `internal.generic`, conflating caller-side
        // bad input with true internal runtime failures in telemetry.
        if vector.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "cannot quantize zero-length vector".to_string(),
            });
        }

        // Reject NaN/Inf
        if !vector.iter().all(|v| v.is_finite()) {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "vector contains NaN or Inf values".to_string(),
            });
        }

        let min = vector.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = vector.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

        let range = max - min;
        let scale = if range < f32::EPSILON {
            1.0
        } else {
            range / 255.0
        };

        let data: Vec<i8> = vector
            .iter()
            .map(|&v| {
                let normalized = if range < f32::EPSILON {
                    0.0
                } else {
                    (v - min) / range * 255.0
                };
                (normalized.round() as i16 - 128).clamp(-128, 127) as i8
            })
            .collect();

        Ok(QuantizedVector {
            data,
            scale,
            offset: min,
        })
    }

    /// Quantize with dimension validation.
    ///
    /// Like [`Self::quantize`] but additionally verifies that the input
    /// vector length matches `expected_dim`. Intended for entry-point
    /// validation where the embedding model dimension is known.
    pub fn quantize_with_expected_dim(
        vector: &[f32],
        expected_dim: usize,
    ) -> Result<QuantizedVector, CoreError> {
        if vector.len() != expected_dim {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "Vector dimension mismatch: expected {expected_dim}, got {}",
                    vector.len()
                ),
            });
        }
        Self::quantize(vector)
    }

    /// Dequantize an INT8 vector back to f32 (approximate reconstruction).
    pub fn dequantize(qv: &QuantizedVector) -> Vec<f32> {
        qv.data
            .iter()
            .map(|&v| (v as f32 + 128.0) * qv.scale + qv.offset)
            .collect()
    }

    /// Compute approximate cosine similarity between two quantized vectors
    /// using INT8 dot product (avoids full dequantization).
    ///
    /// Returns `Err(CoreError::InvalidArguments)` on dimension mismatch or
    /// empty vectors.
    pub fn cosine_similarity_int8(
        a: &QuantizedVector,
        b: &QuantizedVector,
    ) -> Result<f32, CoreError> {
        if a.data.is_empty() || b.data.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "cannot compute cosine similarity on empty vectors".to_string(),
            });
        }
        if a.data.len() != b.data.len() {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: format!("Dimension mismatch: {} vs {}", a.data.len(), b.data.len()),
            });
        }

        Ok(Self::cosine_similarity_int8_unchecked(a, b))
    }

    /// Compute approximate cosine similarity without dimension validation.
    ///
    /// Intended for hot-path loops where dimensions have been pre-validated.
    /// Caller is responsible for ensuring `a.data.len() == b.data.len()` and
    /// both are non-empty. Behavior on mismatched lengths is unspecified
    /// (may silently truncate).
    pub fn cosine_similarity_int8_unchecked(a: &QuantizedVector, b: &QuantizedVector) -> f32 {
        // i32 accumulator: max possible value for 384 dims of i8 is
        // 384 * 127 * 127 = 6,193,152, well within i32 max (2,147,483,647).
        // Using i32 enables LLVM auto-vectorization with SIMD (SDOT on ARM, SSSE3 on x86).
        let mut dot: i32 = 0;
        let mut norm_a: i32 = 0;
        let mut norm_b: i32 = 0;

        for (va, vb) in a.data.iter().zip(b.data.iter()) {
            let a_val = *va as i32;
            let b_val = *vb as i32;
            dot += a_val * b_val;
            norm_a += a_val * a_val;
            norm_b += b_val * b_val;
        }

        let denom = ((norm_a as f64).sqrt() * (norm_b as f64).sqrt()) as f32;
        if denom < f32::EPSILON {
            0.0
        } else {
            dot as f32 / denom
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_basic() {
        let v = vec![0.0, 0.5, 1.0, -0.5, -1.0];
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        assert_eq!(qv.data.len(), 5);

        // Dequantize should be approximately equal
        let reconstructed = ScalarQuantizer::dequantize(&qv);
        for (orig, recon) in v.iter().zip(reconstructed.iter()) {
            assert!((orig - recon).abs() < 0.02, "{orig} vs {recon}");
        }
    }

    #[test]
    fn quantize_empty_vector() {
        let result = ScalarQuantizer::quantize(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn quantize_constant_vector() {
        let v = vec![0.5; 384];
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        // All values should be the same after dequant
        let reconstructed = ScalarQuantizer::dequantize(&qv);
        for val in &reconstructed {
            assert!((val - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn quantize_nan_rejected() {
        let v = vec![1.0, f32::NAN, 0.5];
        let result = ScalarQuantizer::quantize(&v);
        assert!(result.is_err());
    }

    #[test]
    fn quantize_inf_rejected() {
        let v = vec![1.0, f32::INFINITY, 0.5];
        let result = ScalarQuantizer::quantize(&v);
        assert!(result.is_err());
    }

    #[test]
    fn cosine_similarity_identical() {
        let v = vec![0.1, 0.5, 0.9, -0.3, 0.7];
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        let sim = ScalarQuantizer::cosine_similarity_int8(&qv, &qv).unwrap();
        assert!((sim - 1.0).abs() < 0.01);
    }

    #[test]
    fn cosine_similarity_different() {
        // Use higher-dimensional vectors to reduce quantization noise
        let mut a = vec![0.0; 64];
        let mut b = vec![0.0; 64];
        // Make them point in different directions
        for val in a.iter_mut().take(32) {
            *val = 1.0;
        }
        for val in b.iter_mut().skip(32).take(32) {
            *val = 1.0;
        }
        let qa = ScalarQuantizer::quantize(&a).unwrap();
        let qb = ScalarQuantizer::quantize(&b).unwrap();
        let sim = ScalarQuantizer::cosine_similarity_int8(&qa, &qb).unwrap();
        // Quantized orthogonal vectors — similarity should be low
        assert!(sim < 0.3, "expected low similarity, got {sim}");
    }

    #[test]
    fn cosine_similarity_dimension_mismatch_returns_error() {
        let a = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
        let b = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0, 0.5, 0.5]).unwrap();
        let result = ScalarQuantizer::cosine_similarity_int8(&a, &b);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("Dimension mismatch"),
            "error should mention dimension mismatch, got: {err_msg}"
        );
        assert!(
            err_msg.contains("3") && err_msg.contains("5"),
            "error should contain both dimensions, got: {err_msg}"
        );
    }

    #[test]
    fn cosine_similarity_empty_vectors_returns_error() {
        let a = QuantizedVector {
            data: vec![],
            scale: 1.0,
            offset: 0.0,
        };
        let b = QuantizedVector {
            data: vec![],
            scale: 1.0,
            offset: 0.0,
        };
        let result = ScalarQuantizer::cosine_similarity_int8(&a, &b);
        assert!(result.is_err());
    }

    #[test]
    fn cosine_similarity_unchecked_same_as_checked() {
        let v = vec![0.1, 0.5, 0.9, -0.3, 0.7];
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        let checked = ScalarQuantizer::cosine_similarity_int8(&qv, &qv).unwrap();
        let unchecked = ScalarQuantizer::cosine_similarity_int8_unchecked(&qv, &qv);
        assert!(
            (checked - unchecked).abs() < f32::EPSILON,
            "checked ({checked}) and unchecked ({unchecked}) should produce identical results"
        );
    }

    #[test]
    fn quantize_with_expected_dim_accepts_correct() {
        let v = vec![0.1; 384];
        let qv = ScalarQuantizer::quantize_with_expected_dim(&v, 384);
        assert!(qv.is_ok());
        assert_eq!(qv.unwrap().data.len(), 384);
    }

    #[test]
    fn quantize_with_expected_dim_rejects_wrong_size() {
        let v = vec![0.1; 100];
        let result = ScalarQuantizer::quantize_with_expected_dim(&v, 384);
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            err_msg.contains("384") && err_msg.contains("100"),
            "error should mention expected and actual dims, got: {err_msg}"
        );
    }

    /// Iter-95 regression guard: input-validation errors in quantize() must
    /// emit wire code `validation.invalid_arguments`, not `internal.generic`.
    /// Pre-iter-95 the zero-length and NaN guards used InternalCode::Generic,
    /// drifting from the sibling cosine_similarity_int8 which correctly uses
    /// InvalidArguments.
    #[test]
    fn quantize_empty_vector_emits_invalid_arguments_code() {
        let err = ScalarQuantizer::quantize(&[]).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    #[test]
    fn quantize_nan_vector_emits_invalid_arguments_code() {
        let err = ScalarQuantizer::quantize(&[0.5, f32::NAN, 0.3]).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    #[test]
    fn quantize_inf_vector_emits_invalid_arguments_code() {
        let err = ScalarQuantizer::quantize(&[0.5, f32::INFINITY, 0.3]).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }
}
