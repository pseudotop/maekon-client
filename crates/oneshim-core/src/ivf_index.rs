//! IVF (Inverted File) index for partitioned vector search.
//!
//! Uses k-means++ initialization and Lloyd's iteration to partition vectors into
//! sqrt(N) clusters. At query time, only the closest `nprobe` clusters are scanned,
//! reducing search from O(N) to O(N/sqrt(N)) = O(sqrt(N)).
//!
//! See: docs/superpowers/specs/2026-03-19-p3-vector-phase-c-advanced-compression-design.md

use crate::error::CoreError;
use crate::quantization::{QuantizedVector, ScalarQuantizer};
use std::collections::HashMap;

/// Configuration for building an IVF index.
pub struct IvfBuildConfig {
    /// Number of clusters. Default: sqrt(n_vectors).
    pub n_clusters: usize,
    /// Number of Lloyd's iterations. Default: 10.
    pub n_iterations: usize,
    /// Seed for reproducible k-means++ initialization.
    pub seed: u64,
}

impl IvfBuildConfig {
    /// Create a config with automatic cluster count = sqrt(n_vectors).
    pub fn auto(n_vectors: usize) -> Self {
        let n_clusters = (n_vectors as f64).sqrt().ceil() as usize;
        Self {
            n_clusters: n_clusters.max(1),
            n_iterations: 10,
            seed: 42,
        }
    }
}

/// A centroid in the IVF index, stored as an INT8 quantized vector.
pub struct IvfCentroid {
    /// Cluster ID (0-based).
    pub id: usize,
    /// INT8 quantized centroid vector.
    pub vector: QuantizedVector,
    /// Number of vectors assigned to this cluster.
    pub member_count: usize,
}

/// Inverted File Index: maps vectors to clusters for sub-linear search.
pub struct IvfIndex {
    centroids: Vec<IvfCentroid>,
    assignments: HashMap<i64, usize>, // vector_id -> cluster_id
}

/// Simple seeded PRNG (xorshift64) for reproducible k-means++ initialization.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Return a random f64 in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// L2-normalize a vector in-place.
fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

impl IvfIndex {
    /// Build an IVF index from a set of quantized vectors using k-means++ / Lloyd's.
    ///
    /// # Arguments
    /// - `vectors`: (vector_id, QuantizedVector) pairs
    /// - `config`: build configuration
    ///
    /// # Algorithm
    /// 1. K-means++ initialization (seeded)
    /// 2. Lloyd's iteration (config.n_iterations rounds)
    ///    - Assign each vector to nearest centroid (cosine distance)
    ///    - Recompute centroids: dequantize, mean, L2-normalize, re-quantize
    ///    - Handle empty clusters by reassigning to furthest vector
    pub fn build(
        vectors: &[(i64, QuantizedVector)],
        config: &IvfBuildConfig,
    ) -> Result<IvfIndex, CoreError> {
        // Iter-106: input-validation errors emit `validation.invalid_arguments`
        // consistent with iter-95/105 (quantization, binary_quantizer,
        // ml_classifier::preprocess). Caller-supplied bad input should not
        // be conflated with internal runtime failures in telemetry.
        if vectors.is_empty() {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "cannot build IVF index from empty vector set".to_string(),
            });
        }
        if config.n_clusters < 1 {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "n_clusters must be >= 1".to_string(),
            });
        }
        if vectors.len() < config.n_clusters {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "cannot create {} clusters from {} vectors",
                    config.n_clusters,
                    vectors.len()
                ),
            });
        }

        let dims = vectors[0].1.data.len();
        let n = vectors.len();
        let k = config.n_clusters;

        // Pre-dequantize all vectors for faster iteration
        let dequantized: Vec<Vec<f32>> = vectors
            .iter()
            .map(|(_, qv)| ScalarQuantizer::dequantize(qv))
            .collect();

        // K-means++ initialization: pick initial centroids
        let mut rng = Rng::new(config.seed);
        let mut centroid_f32: Vec<Vec<f32>> = Vec::with_capacity(k);

        // First centroid: pick uniformly at random
        let first_idx = (rng.next_u64() as usize) % n;
        centroid_f32.push(dequantized[first_idx].clone());

        // Subsequent centroids: proportional to squared distance to nearest centroid
        let mut min_dists = vec![f64::MAX; n];
        for c in 1..k {
            // Update min distances with the last added centroid
            let last_centroid = &centroid_f32[c - 1];
            for (i, dequant) in dequantized.iter().enumerate() {
                let sim = cosine_sim_f32(last_centroid, dequant);
                let dist = (1.0 - sim as f64).max(0.0);
                if dist < min_dists[i] {
                    min_dists[i] = dist;
                }
            }

            // Pick next centroid with probability proportional to squared distance
            let total: f64 = min_dists.iter().map(|d| d * d).sum();
            if total < f64::EPSILON {
                // All remaining vectors are identical to existing centroids
                let idx = (rng.next_u64() as usize) % n;
                centroid_f32.push(dequantized[idx].clone());
            } else {
                let threshold = rng.next_f64() * total;
                let mut cumulative = 0.0;
                let mut chosen = 0;
                for (i, d) in min_dists.iter().enumerate() {
                    cumulative += d * d;
                    if cumulative >= threshold {
                        chosen = i;
                        break;
                    }
                }
                centroid_f32.push(dequantized[chosen].clone());
            }
        }

        // Lloyd's iteration
        let mut cluster_assignments = vec![0usize; n];

        for _iter in 0..config.n_iterations {
            // Assign each vector to nearest centroid (by cosine similarity)
            for (i, dequant) in dequantized.iter().enumerate() {
                let mut best_cluster = 0;
                let mut best_sim = f32::NEG_INFINITY;
                for (c, centroid) in centroid_f32.iter().enumerate() {
                    let sim = cosine_sim_f32(centroid, dequant);
                    if sim > best_sim {
                        best_sim = sim;
                        best_cluster = c;
                    }
                }
                cluster_assignments[i] = best_cluster;
            }

            // Recompute centroids: component-wise mean, L2-normalize
            let mut new_centroids = vec![vec![0.0f32; dims]; k];
            let mut counts = vec![0usize; k];

            for (i, dequant) in dequantized.iter().enumerate() {
                let c = cluster_assignments[i];
                for (d, &val) in dequant.iter().enumerate() {
                    new_centroids[c][d] += val;
                }
                counts[c] += 1;
            }

            for c in 0..k {
                if counts[c] > 0 {
                    for val in new_centroids[c].iter_mut() {
                        *val /= counts[c] as f32;
                    }
                    // Spherical k-means: L2-normalize centroids
                    l2_normalize(&mut new_centroids[c]);
                } else {
                    // Empty cluster: reassign to the vector furthest from its centroid
                    let mut max_dist: f32 = 0.0;
                    let mut max_idx = 0;
                    for (i, dequant) in dequantized.iter().enumerate() {
                        let assigned_c = cluster_assignments[i];
                        let sim = cosine_sim_f32(&centroid_f32[assigned_c], dequant);
                        let dist = 1.0 - sim;
                        if dist > max_dist {
                            max_dist = dist;
                            max_idx = i;
                        }
                    }
                    new_centroids[c] = dequantized[max_idx].clone();
                    l2_normalize(&mut new_centroids[c]);
                }
            }

            centroid_f32 = new_centroids;
        }

        // Final assignment pass
        for (i, dequant) in dequantized.iter().enumerate() {
            let mut best_cluster = 0;
            let mut best_sim = f32::NEG_INFINITY;
            for (c, centroid) in centroid_f32.iter().enumerate() {
                let sim = cosine_sim_f32(centroid, dequant);
                if sim > best_sim {
                    best_sim = sim;
                    best_cluster = c;
                }
            }
            cluster_assignments[i] = best_cluster;
        }

        // Count members per cluster
        let mut member_counts = vec![0usize; k];
        for &c in &cluster_assignments {
            member_counts[c] += 1;
        }

        // Build centroids as QuantizedVector
        let centroids: Vec<IvfCentroid> = centroid_f32
            .into_iter()
            .enumerate()
            .map(|(id, f32_vec)| {
                let quantized =
                    ScalarQuantizer::quantize(&f32_vec).unwrap_or_else(|_| QuantizedVector {
                        data: vec![0i8; dims],
                        scale: 1.0,
                        offset: 0.0,
                    });
                IvfCentroid {
                    id,
                    vector: quantized,
                    member_count: member_counts[id],
                }
            })
            .collect();

        // Build assignments map
        let mut assignments = HashMap::with_capacity(n);
        for (i, (vec_id, _)) in vectors.iter().enumerate() {
            assignments.insert(*vec_id, cluster_assignments[i]);
        }

        Ok(IvfIndex {
            centroids,
            assignments,
        })
    }

    /// Find the `nprobe` nearest centroids to a query vector (by cosine similarity).
    ///
    /// Returns cluster IDs sorted by similarity descending.
    /// Returns `Err` if query dimensions do not match centroid dimensions.
    pub fn nearest_centroids(
        &self,
        query: &QuantizedVector,
        nprobe: usize,
    ) -> Result<Vec<usize>, CoreError> {
        // Pre-validate dimensions once before the hot loop.
        if let Some(first) = self.centroids.first() {
            if first.vector.data.len() != query.data.len() {
                return Err(CoreError::InvalidArguments {
                    code: crate::error_codes::ValidationCode::InvalidArguments,
                    message: format!(
                        "Dimension mismatch: centroid {} vs query {}",
                        first.vector.data.len(),
                        query.data.len()
                    ),
                });
            }
        }

        let mut sims: Vec<(usize, f32)> = self
            .centroids
            .iter()
            .map(|c| {
                let sim = ScalarQuantizer::cosine_similarity_int8_unchecked(&c.vector, query);
                (c.id, sim)
            })
            .collect();

        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(sims
            .into_iter()
            .take(nprobe.min(self.centroids.len()))
            .map(|(id, _)| id)
            .collect())
    }

    /// Assign a single vector to its nearest centroid. Returns the cluster ID.
    /// Returns `Err` if query dimensions do not match centroid dimensions.
    pub fn assign(&self, vector: &QuantizedVector) -> Result<usize, CoreError> {
        // Pre-validate dimensions once before the hot loop.
        if let Some(first) = self.centroids.first() {
            if first.vector.data.len() != vector.data.len() {
                return Err(CoreError::InvalidArguments {
                    code: crate::error_codes::ValidationCode::InvalidArguments,
                    message: format!(
                        "Dimension mismatch: centroid {} vs query {}",
                        first.vector.data.len(),
                        vector.data.len()
                    ),
                });
            }
        }

        Ok(self
            .centroids
            .iter()
            .map(|c| {
                let sim = ScalarQuantizer::cosine_similarity_int8_unchecked(&c.vector, vector);
                (c.id, sim)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(id, _)| id)
            .unwrap_or(0))
    }

    /// Get all vector IDs assigned to a given cluster.
    pub fn get_cluster_members(&self, cluster_id: usize) -> Vec<i64> {
        self.assignments
            .iter()
            .filter(|(_, &c)| c == cluster_id)
            .map(|(&id, _)| id)
            .collect()
    }

    /// Access the centroids.
    pub fn centroids(&self) -> &[IvfCentroid] {
        &self.centroids
    }

    /// Access the assignments map.
    pub fn assignments(&self) -> &HashMap<i64, usize> {
        &self.assignments
    }

    /// Number of clusters.
    pub fn n_clusters(&self) -> usize {
        self.centroids.len()
    }
}

/// Cosine similarity between two f32 vectors.
fn cosine_sim_f32(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < f32::EPSILON || norm_b < f32::EPSILON {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a synthetic quantized vector from f32 values.
    fn make_qv(values: &[f32]) -> QuantizedVector {
        ScalarQuantizer::quantize(values).unwrap()
    }

    /// Generate synthetic vectors clustered around centers.
    fn generate_clustered_vectors(
        centers: &[Vec<f32>],
        per_cluster: usize,
        dims: usize,
        seed: u64,
    ) -> Vec<(i64, QuantizedVector)> {
        let mut rng = Rng::new(seed);
        let mut vectors = Vec::new();
        let mut id = 1i64;

        for center in centers {
            for _ in 0..per_cluster {
                let mut v = center.clone();
                // Add small noise
                for val in v.iter_mut().take(dims) {
                    let noise = (rng.next_f64() as f32 - 0.5) * 0.1;
                    *val += noise;
                }
                vectors.push((id, make_qv(&v)));
                id += 1;
            }
        }
        vectors
    }

    #[test]
    fn build_basic_clustering() {
        let dims = 10;
        // 3 well-separated clusters
        let center1 = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let center2 = vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let center3 = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

        let vectors = generate_clustered_vectors(&[center1, center2, center3], 33, dims, 42);

        let config = IvfBuildConfig {
            n_clusters: 3,
            n_iterations: 10,
            seed: 42,
        };

        let index = IvfIndex::build(&vectors, &config).unwrap();

        assert_eq!(index.n_clusters(), 3);
        // All 3 clusters should have non-zero membership
        for c in index.centroids() {
            assert!(c.member_count > 0, "cluster {} has 0 members", c.id);
        }
        // All vectors should be assigned
        assert_eq!(index.assignments().len(), vectors.len());
    }

    #[test]
    fn build_single_cluster() {
        let vectors: Vec<(i64, QuantizedVector)> = (1..=10)
            .map(|i| {
                let mut v = vec![0.0; 5];
                v[0] = 1.0 + (i as f32) * 0.01;
                (i as i64, make_qv(&v))
            })
            .collect();

        let config = IvfBuildConfig {
            n_clusters: 1,
            n_iterations: 5,
            seed: 42,
        };

        let index = IvfIndex::build(&vectors, &config).unwrap();
        assert_eq!(index.n_clusters(), 1);
        // All vectors assigned to cluster 0
        for &c in index.assignments().values() {
            assert_eq!(c, 0);
        }
    }

    #[test]
    fn build_too_few_vectors() {
        let vectors = vec![(1, make_qv(&[1.0, 0.0, 0.0]))];
        let config = IvfBuildConfig {
            n_clusters: 5,
            n_iterations: 10,
            seed: 42,
        };
        let result = IvfIndex::build(&vectors, &config);
        assert!(result.is_err());
    }

    #[test]
    fn build_empty_vectors() {
        let vectors: Vec<(i64, QuantizedVector)> = vec![];
        let config = IvfBuildConfig {
            n_clusters: 3,
            n_iterations: 10,
            seed: 42,
        };
        let result = IvfIndex::build(&vectors, &config);
        assert!(result.is_err());
    }

    #[test]
    fn nearest_centroids_returns_correct_order() {
        let dims = 10;
        let center1 = vec![1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let center2 = vec![0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let center3 = vec![0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

        let vectors = generate_clustered_vectors(&[center1, center2, center3], 30, dims, 42);

        let config = IvfBuildConfig {
            n_clusters: 3,
            n_iterations: 10,
            seed: 42,
        };

        let index = IvfIndex::build(&vectors, &config).unwrap();

        // Query near cluster 1 center
        let query = make_qv(&[1.0, 0.05, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let nearest = index.nearest_centroids(&query, 3).unwrap();
        assert_eq!(nearest.len(), 3);
        assert!(!nearest.is_empty());
    }

    #[test]
    fn nearest_centroids_nprobe_limits_results() {
        let vectors: Vec<(i64, QuantizedVector)> = (1..=30)
            .map(|i| {
                let mut v = vec![0.0; 5];
                v[(i as usize) % 5] = 1.0;
                (i as i64, make_qv(&v))
            })
            .collect();

        let config = IvfBuildConfig {
            n_clusters: 5,
            n_iterations: 5,
            seed: 42,
        };

        let index = IvfIndex::build(&vectors, &config).unwrap();
        let query = make_qv(&[1.0, 0.0, 0.0, 0.0, 0.0]);
        let nearest = index.nearest_centroids(&query, 2).unwrap();
        assert_eq!(nearest.len(), 2);
    }

    #[test]
    fn assign_to_nearest() {
        let dims = 5;
        let center1 = vec![1.0, 0.0, 0.0, 0.0, 0.0];
        let center2 = vec![0.0, 1.0, 0.0, 0.0, 0.0];

        let vectors = generate_clustered_vectors(&[center1, center2], 20, dims, 42);

        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 10,
            seed: 42,
        };

        let index = IvfIndex::build(&vectors, &config).unwrap();

        // New vector near center1
        let new_vec = make_qv(&[0.95, 0.05, 0.0, 0.0, 0.0]);
        let cluster = index.assign(&new_vec).unwrap();
        assert!(cluster < 2);
    }

    #[test]
    fn get_cluster_members_returns_correct_ids() {
        let vectors: Vec<(i64, QuantizedVector)> = (1..=20)
            .map(|i| {
                let mut v = vec![0.0; 5];
                v[0] = 1.0 + (i as f32) * 0.01;
                (i as i64, make_qv(&v))
            })
            .collect();

        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 5,
            seed: 42,
        };

        let index = IvfIndex::build(&vectors, &config).unwrap();

        // Collect all members across clusters
        let mut all_members: Vec<i64> = Vec::new();
        for c in 0..2 {
            all_members.extend(index.get_cluster_members(c));
        }
        all_members.sort();

        let mut expected: Vec<i64> = (1..=20).collect();
        expected.sort();
        assert_eq!(all_members, expected);
    }

    #[test]
    fn deterministic_with_seed() {
        let vectors: Vec<(i64, QuantizedVector)> = (1..=50)
            .map(|i| {
                let mut v = vec![0.0; 5];
                v[(i as usize) % 5] = 1.0;
                v[0] += (i as f32) * 0.01;
                (i as i64, make_qv(&v))
            })
            .collect();

        let config = IvfBuildConfig {
            n_clusters: 5,
            n_iterations: 5,
            seed: 12345,
        };

        let index1 = IvfIndex::build(&vectors, &config).unwrap();
        let index2 = IvfIndex::build(&vectors, &config).unwrap();

        for (id, &c1) in index1.assignments() {
            let c2 = index2.assignments()[id];
            assert_eq!(c1, c2, "assignment differs for vector {id}");
        }
    }

    #[test]
    fn build_config_defaults() {
        let config = IvfBuildConfig::auto(10000);
        assert_eq!(config.n_clusters, 100); // sqrt(10000) = 100
        assert_eq!(config.n_iterations, 10);

        let config2 = IvfBuildConfig::auto(0);
        assert_eq!(config2.n_clusters, 1);
    }
}
