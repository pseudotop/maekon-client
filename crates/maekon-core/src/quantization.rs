//! Scalar INT8 quantization for embedding vectors.
//!
//! Converts f32 embedding vectors to i8 using per-vector min-max scaling.
//! ~4x storage reduction with ~99% recall preservation for 384-dim embeddings.
//!
//! Public architecture context: docs/architecture/ADR-013-llm-summary-vector-rag.md.

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
    /// - Finite-but-huge values whose `max - min` overflows f32 to Inf:
    ///   rejected after the range is computed, returns
    ///   `CoreError::InvalidArguments` (prevents a corrupt `scale = Inf`)
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
        // Reject a non-finite range before deriving `scale`. Each element is
        // finite (checked above), but for finite-but-huge magnitudes
        // `max - min` can still overflow f32 to +Inf (e.g. `f32::MAX -
        // f32::MIN`). Persisting `scale = Inf / 255.0 = Inf` would corrupt
        // every quantized code and silently poison downstream similarity.
        if !range.is_finite() {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "vector value range overflows f32 (values too large in magnitude)"
                    .to_string(),
            });
        }
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

    /// Compute approximate cosine similarity between two quantized vectors.
    ///
    /// Each component is dequantized to f32 (`(code + 128) * scale + offset`)
    /// before the dot product and norms are accumulated, so the result equals
    /// the true f32 cosine up to quantization rounding error.
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

    /// Compute cosine similarity without dimension validation.
    ///
    /// Intended for hot-path loops where dimensions have been pre-validated.
    /// Caller is responsible for ensuring `a.data.len() == b.data.len()` and
    /// both are non-empty. Behavior on mismatched lengths is unspecified
    /// (may silently truncate).
    ///
    /// Each int8 code is dequantized to its f32 value
    /// (`(code + 128) * scale + offset`) before accumulating the dot product
    /// and norms. Computing cosine directly over the raw int8 codes would be
    /// wrong for asymmetric (non-zero-`offset`) quantization: the `+ 128`
    /// recentering and per-vector `scale`/`offset` terms do not cancel, so the
    /// code-domain ranking diverges from the true f32 ranking for non-zero-mean
    /// embeddings.
    pub fn cosine_similarity_int8_unchecked(a: &QuantizedVector, b: &QuantizedVector) -> f32 {
        // f64 accumulators: dequantized values are real-valued, and f64
        // keeps the dot/norm sums accurate across high-dimensional vectors.
        let mut dot: f64 = 0.0;
        let mut norm_a: f64 = 0.0;
        let mut norm_b: f64 = 0.0;

        let (sa, oa) = (a.scale as f64, a.offset as f64);
        let (sb, ob) = (b.scale as f64, b.offset as f64);

        for (va, vb) in a.data.iter().zip(b.data.iter()) {
            // Dequantize: real = (code + 128) * scale + offset.
            let a_val = (*va as f64 + 128.0) * sa + oa;
            let b_val = (*vb as f64 + 128.0) * sb + ob;
            dot += a_val * b_val;
            norm_a += a_val * a_val;
            norm_b += b_val * b_val;
        }

        let denom = (norm_a.sqrt() * norm_b.sqrt()) as f32;
        if denom < f32::EPSILON {
            0.0
        } else {
            (dot / (norm_a.sqrt() * norm_b.sqrt())) as f32
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
        let err = ScalarQuantizer::quantize(&[]).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "empty vector must produce InvalidArguments, got: {err:?}"
        );
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
        let err = ScalarQuantizer::quantize(&v).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "NaN vector must produce InvalidArguments, got: {err:?}"
        );
    }

    #[test]
    fn quantize_inf_rejected() {
        let v = vec![1.0, f32::INFINITY, 0.5];
        let err = ScalarQuantizer::quantize(&v).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "Inf vector must produce InvalidArguments, got: {err:?}"
        );
    }

    /// #6170 regression guard: finite-but-huge magnitudes whose `max - min`
    /// overflows f32 to Inf must be rejected. Each element passes the
    /// NaN/Inf pre-scan, but `f32::MAX - f32::MIN == 2 * f32::MAX` overflows
    /// to +Inf, which would otherwise persist as `scale = Inf` and corrupt
    /// every quantized code.
    #[test]
    fn quantize_overflowing_range_rejected() {
        let v = vec![f32::MAX, f32::MIN];
        // Sanity: both inputs are finite, so the NaN/Inf pre-scan passes and
        // the overflow guard is the one doing the rejecting.
        assert!(v.iter().all(|x| x.is_finite()));
        let err = ScalarQuantizer::quantize(&v).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "overflowing range must produce InvalidArguments, got: {err:?}"
        );
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    /// A vector with large-but-non-overflowing magnitudes must still quantize
    /// successfully and produce a finite `scale` — the #6170 guard must not be
    /// over-eager and reject ordinary large vectors.
    #[test]
    fn quantize_large_finite_range_still_ok() {
        let v = vec![-1.0e30_f32, 0.0, 1.0e30_f32];
        let qv = ScalarQuantizer::quantize(&v).expect("large finite range must quantize");
        assert!(
            qv.scale.is_finite() && qv.scale > 0.0,
            "scale must be finite and positive, got: {}",
            qv.scale
        );
        assert_eq!(qv.data.len(), 3);
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

    /// Cosine of a quantized vector against itself must be ~1.0 even when the
    /// embedding is strongly non-zero-mean (large `offset`). Computing cosine
    /// over the raw int8 codes would not yield 1.0 here, since recentering the
    /// dequantized vector around its true mean changes the angle.
    #[test]
    fn cosine_similarity_identical_nonzero_mean() {
        let v = vec![10.0, 10.5, 9.8, 10.2, 11.0, 9.5];
        let qv = ScalarQuantizer::quantize(&v).unwrap();
        let sim = ScalarQuantizer::cosine_similarity_int8(&qv, &qv).unwrap();
        assert!(
            (sim - 1.0).abs() < 0.01,
            "self-similarity of a non-zero-mean vector must be ~1.0, got {sim}"
        );
    }

    /// #6097 regression guard: for non-zero-mean embeddings the int8 cosine
    /// must rank candidates the same way the true f32 cosine does.
    ///
    /// These exact vectors produce a ranking *inversion* under the old
    /// code-domain cosine (which accumulated dot/norm directly over the raw
    /// int8 codes): true f32 cosine ranks `cand1` above `cand2`, but the
    /// code-domain cosine ranked `cand2` above `cand1`. Dequantizing each code
    /// to `(code + 128) * scale + offset` before accumulating restores the
    /// correct order.
    #[test]
    fn cosine_similarity_ranking_matches_f32_for_nonzero_mean() {
        let query = vec![0.7, 0.6, 3.0, 2.1];
        let cand1 = vec![2.4, 2.6, 2.3, 2.3];
        let cand2 = vec![3.0, 0.7, 0.6, 0.9];

        // True f32 cosine — the ground truth ranking.
        let f32_cosine = |a: &[f32], b: &[f32]| -> f32 {
            let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
            let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
            let nb: f32 = b.iter().map(|y| y * y).sum::<f32>().sqrt();
            dot / (na * nb)
        };
        let true1 = f32_cosine(&query, &cand1);
        let true2 = f32_cosine(&query, &cand2);
        assert!(
            true1 > true2,
            "fixture invariant: f32 cosine must prefer cand1 ({true1}) over cand2 ({true2})"
        );

        let q = ScalarQuantizer::quantize(&query).unwrap();
        let c1 = ScalarQuantizer::quantize(&cand1).unwrap();
        let c2 = ScalarQuantizer::quantize(&cand2).unwrap();

        let sim1 = ScalarQuantizer::cosine_similarity_int8(&q, &c1).unwrap();
        let sim2 = ScalarQuantizer::cosine_similarity_int8(&q, &c2).unwrap();

        // The int8 cosine must agree with the f32 ranking (cand1 preferred).
        assert!(
            sim1 > sim2,
            "int8 cosine ranking must match f32: cand1 ({sim1}) must beat cand2 ({sim2})"
        );

        // And it must be numerically close to the true cosine, not merely
        // ordered correctly.
        assert!(
            (sim1 - true1).abs() < 0.02,
            "int8 cosine ({sim1}) must approximate true f32 cosine ({true1})"
        );
        assert!(
            (sim2 - true2).abs() < 0.02,
            "int8 cosine ({sim2}) must approximate true f32 cosine ({true2})"
        );
    }

    #[test]
    fn cosine_similarity_dimension_mismatch_returns_error() {
        let a = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0]).unwrap();
        let b = ScalarQuantizer::quantize(&[1.0, 0.0, 0.0, 0.5, 0.5]).unwrap();
        let err = ScalarQuantizer::cosine_similarity_int8(&a, &b).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "dimension mismatch must produce InvalidArguments, got: {err:?}"
        );
        let err_msg = format!("{err}");
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
        let err = ScalarQuantizer::cosine_similarity_int8(&a, &b).unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "empty vector cosine similarity must produce InvalidArguments, got: {err:?}"
        );
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
        let qv = ScalarQuantizer::quantize_with_expected_dim(&v, 384).expect(
            "quantize_with_expected_dim must accept a vector whose length matches expected_dim",
        );
        // Contract: the quantized data must have the same dimension as the input.
        assert_eq!(
            qv.data.len(),
            384,
            "quantized data must preserve the 384-dim length"
        );
    }

    #[test]
    fn quantize_with_expected_dim_rejects_wrong_size() {
        let v = vec![0.1; 100];
        let result = ScalarQuantizer::quantize_with_expected_dim(&v, 384);
        let err = result.unwrap_err();
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "dimension mismatch must produce InvalidArguments, got: {err:?}"
        );
        let err_msg = format!("{err}");
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
