//! 2-bit binary quantization for coarse-grained Hamming distance filtering.
//!
//! Maps each dimension of an f32 vector to 2 bits based on per-dimension quantile
//! thresholds (q25, q50, q75). Produces a compact `BinaryCode` (96 bytes for 384 dims)
//! suitable for fast Hamming distance pre-filtering before INT8 re-ranking.
//!
//! See: docs/superpowers/specs/2026-03-19-p3-vector-phase-c-advanced-compression-design.md

use crate::error::CoreError;
use serde::{Deserialize, Serialize};

/// Per-dimension quantile thresholds computed across the entire collection.
/// Used by 2-bit binary quantization to map each f32 dimension to 2 bits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantileThresholds {
    /// 25th percentile per dimension.
    pub q25: Vec<f32>,
    /// 50th percentile (median) per dimension.
    pub q50: Vec<f32>,
    /// 75th percentile per dimension.
    pub q75: Vec<f32>,
    /// Number of dimensions.
    pub dimensions: usize,
}

/// 2-bit binary code packed into bytes. For 384 dims = 96 bytes.
/// Each dimension occupies 2 bits: 00, 01, 10, 11 mapped to 4 quantile levels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryCode {
    pub data: Vec<u8>,
}

/// Stateless 2-bit binary quantizer for coarse-grained Hamming distance filtering.
pub struct BinaryQuantizer;

impl BinaryQuantizer {
    /// Compute per-dimension quantile thresholds from a collection of f32 vectors.
    ///
    /// For each dimension, collects all values, sorts them, and picks indices at
    /// the 25th, 50th, and 75th percentiles. Processes one dimension at a time to
    /// keep memory usage proportional to the number of vectors (not vectors x dims).
    ///
    /// # Errors
    /// - Returns error if `vectors` is empty or has fewer than 2 vectors
    /// - Returns error if any vector length != `dimensions`
    pub fn compute_thresholds(
        vectors: &[Vec<f32>],
        dimensions: usize,
    ) -> Result<QuantileThresholds, CoreError> {
        // Iter-95: align with quantization.rs::cosine_similarity_int8 — input
        // validation emits CoreError::InvalidArguments with wire code
        // `validation.invalid_arguments`, not Internal.Generic. Previously
        // all four guards below emitted `internal.generic`, conflating caller
        // bad-input with runtime failures.
        if vectors.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "cannot compute thresholds on empty vector set".to_string(),
            });
        }
        if vectors.len() < 2 {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "cannot compute quantile thresholds with fewer than 2 vectors".to_string(),
            });
        }
        if dimensions == 0 {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "dimensions must be > 0".to_string(),
            });
        }
        for (i, v) in vectors.iter().enumerate() {
            if v.len() != dimensions {
                return Err(CoreError::InvalidArguments {
                    code: crate::error_codes::ValidationCode::InvalidArguments,
                    message: format!(
                        "vector {i} has {} dimensions, expected {dimensions}",
                        v.len()
                    ),
                });
            }
        }

        let n = vectors.len();
        let mut q25 = Vec::with_capacity(dimensions);
        let mut q50 = Vec::with_capacity(dimensions);
        let mut q75 = Vec::with_capacity(dimensions);

        // Process one dimension at a time to limit memory usage
        let mut dim_values = Vec::with_capacity(n);

        for d in 0..dimensions {
            dim_values.clear();
            for v in vectors {
                dim_values.push(v[d]);
            }
            dim_values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

            // For constant dimensions (all same value), set all thresholds equal
            let first = dim_values[0];
            let last = dim_values[n - 1];
            if (last - first).abs() < f32::EPSILON {
                q25.push(first);
                q50.push(first);
                q75.push(first);
            } else {
                // Pick percentile indices: floor-based for simplicity
                let idx_25 = n / 4;
                let idx_50 = n / 2;
                let idx_75 = 3 * n / 4;
                q25.push(dim_values[idx_25]);
                q50.push(dim_values[idx_50]);
                q75.push(dim_values[idx_75]);
            }
        }

        Ok(QuantileThresholds {
            q25,
            q50,
            q75,
            dimensions,
        })
    }

    /// Encode a single f32 vector to a 2-bit binary code using pre-computed thresholds.
    ///
    /// For each dimension, maps the value to 2 bits:
    /// - `00` if < q25
    /// - `01` if < q50
    /// - `10` if < q75
    /// - `11` if >= q75
    ///
    /// Packs 4 two-bit codes per byte (MSB first): dims 0-3 in byte 0, dims 4-7 in byte 1, etc.
    /// Output length: `ceil(dimensions * 2 / 8)` bytes = 96 for 384 dims.
    pub fn encode(
        vector: &[f32],
        thresholds: &QuantileThresholds,
    ) -> Result<BinaryCode, CoreError> {
        if vector.len() != thresholds.dimensions {
            // Iter-95: dimension mismatch is caller-side input validation.
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "vector length {} does not match threshold dimensions {}",
                    vector.len(),
                    thresholds.dimensions
                ),
            });
        }

        let num_bytes = (thresholds.dimensions * 2).div_ceil(8);
        let mut data = vec![0u8; num_bytes];

        for (d, &val) in vector.iter().enumerate() {
            let code: u8 = if val < thresholds.q25[d] {
                0b00
            } else if val < thresholds.q50[d] {
                0b01
            } else if val < thresholds.q75[d] {
                0b10
            } else {
                0b11
            };

            // Each dimension takes 2 bits. 4 dimensions per byte.
            // Dimension d maps to byte (d / 4), shift (6 - 2 * (d % 4)).
            let byte_idx = d / 4;
            let bit_shift = 6 - 2 * (d % 4);
            data[byte_idx] |= code << bit_shift;
        }

        Ok(BinaryCode { data })
    }

    /// Compute the Hamming distance between two binary codes.
    ///
    /// Counts the number of differing bits across all bytes via XOR + popcount.
    pub fn hamming_distance(a: &BinaryCode, b: &BinaryCode) -> u32 {
        a.data
            .iter()
            .zip(b.data.iter())
            .map(|(&x, &y)| (x ^ y).count_ones())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_computation_basic() {
        // 4 vectors, 3 dims — each dim has values [1, 2, 3, 4]
        let vectors = vec![
            vec![1.0, 4.0, 1.0],
            vec![2.0, 3.0, 2.0],
            vec![3.0, 2.0, 3.0],
            vec![4.0, 1.0, 4.0],
        ];

        let thresholds = BinaryQuantizer::compute_thresholds(&vectors, 3).unwrap();
        assert_eq!(thresholds.dimensions, 3);
        assert_eq!(thresholds.q25.len(), 3);
        assert_eq!(thresholds.q50.len(), 3);
        assert_eq!(thresholds.q75.len(), 3);

        // For dim 0: sorted = [1,2,3,4], n=4: idx_25=1, idx_50=2, idx_75=3
        assert!((thresholds.q25[0] - 2.0).abs() < f32::EPSILON);
        assert!((thresholds.q50[0] - 3.0).abs() < f32::EPSILON);
        assert!((thresholds.q75[0] - 4.0).abs() < f32::EPSILON);
    }

    #[test]
    fn threshold_computation_single_vector() {
        let vectors = vec![vec![1.0, 2.0, 3.0]];
        let result = BinaryQuantizer::compute_thresholds(&vectors, 3);
        assert!(result.is_err());
    }

    #[test]
    fn threshold_computation_empty() {
        let vectors: Vec<Vec<f32>> = vec![];
        let result = BinaryQuantizer::compute_thresholds(&vectors, 3);
        assert!(result.is_err());
    }

    #[test]
    fn threshold_computation_constant_dimension() {
        // All vectors have the same value in dim 0
        let vectors = vec![
            vec![5.0, 1.0],
            vec![5.0, 2.0],
            vec![5.0, 3.0],
            vec![5.0, 4.0],
        ];
        let thresholds = BinaryQuantizer::compute_thresholds(&vectors, 2).unwrap();
        // Constant dim: all thresholds equal to that value
        assert!((thresholds.q25[0] - 5.0).abs() < f32::EPSILON);
        assert!((thresholds.q50[0] - 5.0).abs() < f32::EPSILON);
        assert!((thresholds.q75[0] - 5.0).abs() < f32::EPSILON);
    }

    #[test]
    fn encode_basic() {
        let thresholds = QuantileThresholds {
            q25: vec![1.0, 1.0, 1.0, 1.0],
            q50: vec![2.0, 2.0, 2.0, 2.0],
            q75: vec![3.0, 3.0, 3.0, 3.0],
            dimensions: 4,
        };

        // Vector [0.5, 1.5, 2.5, 3.5] => codes [00, 01, 10, 11]
        let code = BinaryQuantizer::encode(&[0.5, 1.5, 2.5, 3.5], &thresholds).unwrap();

        // Packed into 1 byte (4 dims * 2 bits = 8 bits):
        // 00_01_10_11 = 0b00011011 = 0x1B = 27
        assert_eq!(code.data.len(), 1);
        assert_eq!(code.data[0], 0b00_01_10_11);
    }

    #[test]
    fn encode_dimension_mismatch() {
        let thresholds = QuantileThresholds {
            q25: vec![1.0],
            q50: vec![2.0],
            q75: vec![3.0],
            dimensions: 1,
        };
        let result = BinaryQuantizer::encode(&[1.0, 2.0], &thresholds);
        assert!(result.is_err());
    }

    #[test]
    fn encode_all_below_q25() {
        let thresholds = QuantileThresholds {
            q25: vec![10.0; 8],
            q50: vec![20.0; 8],
            q75: vec![30.0; 8],
            dimensions: 8,
        };
        let vector = vec![0.0; 8]; // All below q25
        let code = BinaryQuantizer::encode(&vector, &thresholds).unwrap();
        // All bits 00 => all bytes = 0
        assert_eq!(code.data.len(), 2); // 8 dims * 2 bits / 8 = 2 bytes
        assert_eq!(code.data[0], 0x00);
        assert_eq!(code.data[1], 0x00);
    }

    #[test]
    fn encode_all_above_q75() {
        let thresholds = QuantileThresholds {
            q25: vec![1.0; 8],
            q50: vec![2.0; 8],
            q75: vec![3.0; 8],
            dimensions: 8,
        };
        let vector = vec![100.0; 8]; // All above q75
        let code = BinaryQuantizer::encode(&vector, &thresholds).unwrap();
        // All bits 11 => all bytes = 0xFF
        assert_eq!(code.data.len(), 2);
        assert_eq!(code.data[0], 0xFF);
        assert_eq!(code.data[1], 0xFF);
    }

    #[test]
    fn hamming_distance_identical() {
        let code = BinaryCode {
            data: vec![0xAB, 0xCD, 0xEF],
        };
        assert_eq!(BinaryQuantizer::hamming_distance(&code, &code), 0);
    }

    #[test]
    fn hamming_distance_opposite() {
        let a = BinaryCode {
            data: vec![0x00, 0x00],
        };
        let b = BinaryCode {
            data: vec![0xFF, 0xFF],
        };
        // 0xFF has 8 ones, two bytes = 16
        assert_eq!(BinaryQuantizer::hamming_distance(&a, &b), 16);
    }

    #[test]
    fn hamming_distance_single_bit_diff() {
        let a = BinaryCode {
            data: vec![0b00000000],
        };
        let b = BinaryCode {
            data: vec![0b00000001],
        };
        assert_eq!(BinaryQuantizer::hamming_distance(&a, &b), 1);
    }

    #[test]
    fn hamming_distance_384_dims() {
        // 384 dims => 96 bytes
        let a = BinaryCode {
            data: vec![0x00; 96],
        };
        let b = BinaryCode {
            data: vec![0xFF; 96],
        };
        // Each byte differs in all 8 bits
        assert_eq!(BinaryQuantizer::hamming_distance(&a, &b), 96 * 8);
    }

    #[test]
    fn encode_decode_roundtrip() {
        // Use a larger set of vectors with clear separation so thresholds are meaningful.
        // Group A: values concentrated near 0.1, Group B: values concentrated near 0.9
        let mut vectors = Vec::new();
        for i in 0..10 {
            let offset = i as f32 * 0.01;
            vectors.push(vec![0.1 + offset, 0.1 + offset, 0.1 + offset, 0.1 + offset]);
        }
        for i in 0..10 {
            let offset = i as f32 * 0.01;
            vectors.push(vec![0.9 + offset, 0.9 + offset, 0.9 + offset, 0.9 + offset]);
        }

        let thresholds = BinaryQuantizer::compute_thresholds(&vectors, 4).unwrap();

        // Two vectors from Group A (close)
        let code_a = BinaryQuantizer::encode(&vectors[0], &thresholds).unwrap();
        let code_b = BinaryQuantizer::encode(&vectors[1], &thresholds).unwrap();
        // One vector from Group B (far from A)
        let code_c = BinaryQuantizer::encode(&vectors[15], &thresholds).unwrap();

        let dist_ab = BinaryQuantizer::hamming_distance(&code_a, &code_b);
        let dist_ac = BinaryQuantizer::hamming_distance(&code_a, &code_c);

        // Vectors within the same group should have smaller Hamming distance
        assert!(
            dist_ab <= dist_ac,
            "close vectors should have smaller Hamming distance: ab={dist_ab}, ac={dist_ac}"
        );
    }

    /// Iter-95 regression guards: input-validation errors across both
    /// compute_thresholds and encode must emit `validation.invalid_arguments`.
    /// Pre-iter-95 all five guards emitted `internal.generic`, making caller
    /// bad-input and runtime failures indistinguishable in telemetry.
    #[test]
    fn compute_thresholds_empty_vectors_emits_invalid_arguments_code() {
        let err = BinaryQuantizer::compute_thresholds(&[], 4).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    #[test]
    fn compute_thresholds_single_vector_emits_invalid_arguments_code() {
        let err = BinaryQuantizer::compute_thresholds(&[vec![0.1, 0.2, 0.3]], 3).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    #[test]
    fn compute_thresholds_zero_dimensions_emits_invalid_arguments_code() {
        let err = BinaryQuantizer::compute_thresholds(&[vec![], vec![]], 0).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    #[test]
    fn compute_thresholds_dimension_mismatch_emits_invalid_arguments_code() {
        let err = BinaryQuantizer::compute_thresholds(
            &[vec![0.1, 0.2, 0.3], vec![0.4, 0.5]], // 2nd has wrong len
            3,
        )
        .unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }

    #[test]
    fn encode_dimension_mismatch_emits_invalid_arguments_code() {
        let thresholds = QuantileThresholds {
            dimensions: 3,
            q25: vec![0.25, 0.25, 0.25],
            q50: vec![0.50, 0.50, 0.50],
            q75: vec![0.75, 0.75, 0.75],
        };
        let err = BinaryQuantizer::encode(&[0.1, 0.2], &thresholds).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }
}
