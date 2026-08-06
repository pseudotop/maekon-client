//! IVF (Inverted File) index for partitioned vector search.
//!
//! Uses k-means++ initialization and Lloyd's iteration to partition vectors into
//! sqrt(N) clusters. At query time, only the closest `nprobe` clusters are scanned,
//! reducing search from O(N) to O(N/sqrt(N)) = O(sqrt(N)).
//!
//! Public architecture context: docs/architecture/ADR-013-llm-summary-vector-rag.md.

use crate::error::CoreError;
use crate::quantization::{QuantizedVector, ScalarQuantizer};
use std::collections::{HashMap, HashSet};

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

        // Dimension homogeneity: every vector must share the same non-zero
        // dimension. Mixed dimensions later cause an out-of-bounds panic in the
        // centroid accumulation loop (`new_centroids[c][d]` is sized to `dims`
        // but indexed by the per-vector length) or, for shorter vectors, silent
        // centroid corruption. Reject up front as caller-supplied bad input.
        let dims = vectors[0].1.data.len();
        if dims == 0 || vectors.iter().any(|(_, qv)| qv.data.len() != dims) {
            return Err(CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                message: "all vectors must share the same non-zero dimension".to_string(),
            });
        }
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

            // Track vectors already chosen to reseed an empty cluster in THIS
            // iteration. Without this, multiple empty clusters in one Lloyd
            // round all pick the same farthest vector, producing duplicate
            // centroids that then collapse together. Skipping taken indices
            // keeps reseeded centroids distinct.
            let mut taken: HashSet<usize> = HashSet::new();
            for c in 0..k {
                if counts[c] > 0 {
                    for val in new_centroids[c].iter_mut() {
                        *val /= counts[c] as f32;
                    }
                    // Spherical k-means: L2-normalize centroids
                    l2_normalize(&mut new_centroids[c]);
                } else {
                    // Empty cluster: reassign to the vector furthest from its
                    // centroid, excluding vectors already used to reseed another
                    // empty cluster this iteration. `n >= k` (validated above)
                    // guarantees at least one untaken vector remains for every
                    // empty cluster, so a target is always found.
                    if let Some(idx) = farthest_untaken_vector(
                        &dequantized,
                        &cluster_assignments,
                        &centroid_f32,
                        &taken,
                    ) {
                        taken.insert(idx);
                        new_centroids[c] = dequantized[idx].clone();
                        l2_normalize(&mut new_centroids[c]);
                    }
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

/// Pick the index of the vector farthest (by cosine distance) from its
/// currently-assigned centroid, skipping any index already in `taken`.
///
/// Used to reseed empty clusters during Lloyd's iteration. Skipping `taken`
/// indices is what keeps multiple empty clusters in the same iteration from all
/// grabbing the same farthest vector (#6172). Returns `None` only if every index
/// is already taken (cannot happen while `vectors.len() >= n_clusters`).
///
/// `max_dist` starts below the cosine-distance floor (`1 - sim ∈ [0, 2]`) so the
/// first untaken candidate is always selected even when all distances are ~0;
/// the old `max_dist = 0.0` / `max_idx = 0` defaults could otherwise return a
/// taken index 0 and reintroduce a duplicate.
fn farthest_untaken_vector(
    dequantized: &[Vec<f32>],
    cluster_assignments: &[usize],
    centroid_f32: &[Vec<f32>],
    taken: &HashSet<usize>,
) -> Option<usize> {
    let mut max_dist: f32 = -1.0;
    let mut max_idx: Option<usize> = None;
    for (i, dequant) in dequantized.iter().enumerate() {
        if taken.contains(&i) {
            continue;
        }
        let assigned_c = cluster_assignments[i];
        let sim = cosine_sim_f32(&centroid_f32[assigned_c], dequant);
        let dist = 1.0 - sim;
        if dist > max_dist {
            max_dist = dist;
            max_idx = Some(i);
        }
    }
    max_idx
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
        // IvfIndex does not implement Debug, so use the .err().unwrap() pattern
        let err = IvfIndex::build(&vectors, &config)
            .err()
            .expect("fewer-vectors-than-clusters must return Err");
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "fewer vectors than clusters must produce InvalidArguments, got: {err:?}"
        );
    }

    #[test]
    fn build_empty_vectors() {
        let vectors: Vec<(i64, QuantizedVector)> = vec![];
        let config = IvfBuildConfig {
            n_clusters: 3,
            n_iterations: 10,
            seed: 42,
        };
        // IvfIndex does not implement Debug, so use the .err().unwrap() pattern
        let err = IvfIndex::build(&vectors, &config)
            .err()
            .expect("empty vector set must return Err");
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "empty vector set must produce InvalidArguments, got: {err:?}"
        );
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
    fn build_mixed_dimensions_returns_err() {
        // #6171: mixed-dimension input must be rejected up front rather than
        // panicking (OOB) or silently corrupting centroids during accumulation.
        let vectors: Vec<(i64, QuantizedVector)> = vec![
            (1, make_qv(&[1.0, 0.0, 0.0, 0.0, 0.0])), // 5 dims
            (2, make_qv(&[0.0, 1.0, 0.0])),           // 3 dims
            (3, make_qv(&[0.0, 0.0, 1.0, 0.0, 0.0])), // 5 dims
        ];
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 5,
            seed: 42,
        };
        // IvfIndex does not implement Debug, so use the .err().unwrap() pattern
        let err = IvfIndex::build(&vectors, &config)
            .err()
            .expect("mixed-dimension vectors must return Err");
        assert!(
            matches!(err, CoreError::InvalidArguments { .. }),
            "mixed dimensions must produce InvalidArguments, got: {err:?}"
        );
    }

    #[test]
    fn empty_cluster_reseed_picks_distinct_indices() {
        // #6172 (core fix, deterministic): when several clusters are empty in a
        // single Lloyd iteration, each reseed must pick a DISTINCT source vector.
        // The buggy version scanned for the farthest point without excluding
        // already-chosen indices, so every empty cluster in the iteration grabbed
        // the SAME farthest vector -> duplicate centroids.
        //
        // We exercise the extracted selection helper directly so the assertion is
        // independent of k-means++ RNG. Five vectors, all assigned to centroid 0,
        // with strictly decreasing distance from that centroid (idx 0 farthest).
        let centroid = vec![1.0f32, 0.0, 0.0];
        let dequantized = vec![
            vec![0.0f32, 1.0, 0.0],   // idx 0: orthogonal -> dist ~1.0 (farthest)
            vec![0.30f32, 0.95, 0.0], // idx 1
            vec![0.60f32, 0.80, 0.0], // idx 2
            vec![0.85f32, 0.52, 0.0], // idx 3
            vec![0.97f32, 0.24, 0.0], // idx 4: closest to centroid
        ];
        let cluster_assignments = vec![0usize; dequantized.len()];
        let centroid_f32 = vec![centroid];

        // Simulate three empty clusters reseeding in one iteration.
        let mut taken: HashSet<usize> = HashSet::new();
        let mut picks = Vec::new();
        for _ in 0..3 {
            let idx =
                farthest_untaken_vector(&dequantized, &cluster_assignments, &centroid_f32, &taken)
                    .expect("an untaken vector must remain");
            assert!(
                taken.insert(idx),
                "reseed picked an already-taken index {idx} (duplicate centroid)"
            );
            picks.push(idx);
        }

        // Farthest-first ordering: idx 0, then 1, then 2 — and all distinct.
        assert_eq!(
            picks,
            vec![0, 1, 2],
            "reseed must pick farthest untaken first"
        );
        let unique: HashSet<usize> = picks.iter().copied().collect();
        assert_eq!(unique.len(), picks.len(), "reseed indices must be distinct");
    }

    #[test]
    fn empty_cluster_reseed_all_zero_distance_avoids_taken_index_zero() {
        // #6172 regression on the default-value collision: if every candidate has
        // distance ~0 (e.g. each vector sits on its own centroid), the old
        // `max_dist = 0.0` / `max_idx = 0` defaults returned index 0 even after it
        // was taken, recreating a duplicate. With `max_dist = -1.0` and an
        // `Option` result, a *different* untaken index is returned instead.
        let centroid = vec![1.0f32, 0.0];
        // All three vectors are identical to the centroid -> dist == 0 for each.
        let dequantized = vec![vec![1.0f32, 0.0], vec![1.0f32, 0.0], vec![1.0f32, 0.0]];
        let cluster_assignments = vec![0usize; 3];
        let centroid_f32 = vec![centroid];

        let mut taken: HashSet<usize> = HashSet::new();
        let first =
            farthest_untaken_vector(&dequantized, &cluster_assignments, &centroid_f32, &taken)
                .expect("first reseed must find a vector");
        taken.insert(first);
        let second =
            farthest_untaken_vector(&dequantized, &cluster_assignments, &centroid_f32, &taken)
                .expect("second reseed must find a different vector");
        assert_ne!(
            first, second,
            "zero-distance reseed must not reselect the taken index"
        );

        // When every index is taken, the helper reports exhaustion via None.
        taken.insert(second);
        let third =
            farthest_untaken_vector(&dequantized, &cluster_assignments, &centroid_f32, &taken);
        taken.insert(third.expect("third (last) reseed must find the final vector"));
        assert!(
            farthest_untaken_vector(&dequantized, &cluster_assignments, &centroid_f32, &taken)
                .is_none(),
            "all indices taken must return None"
        );
    }

    #[test]
    fn build_with_empty_clusters_succeeds() {
        // End-to-end sanity: requesting more clusters than distinct directions
        // forces empty clusters every Lloyd iteration (the reseed path). Build
        // must still succeed and assign every vector. (Centroid *values* cannot
        // all be distinct here — there are fewer directions than clusters — so
        // the distinct-reseed invariant is asserted at the helper level above.)
        let dims = 5;
        let mut vectors: Vec<(i64, QuantizedVector)> = Vec::new();
        let mut id = 1i64;
        for axis in 0..3 {
            for _ in 0..10 {
                let mut v = vec![0.0f32; dims];
                v[axis] = 1.0;
                vectors.push((id, make_qv(&v)));
                id += 1;
            }
        }
        let config = IvfBuildConfig {
            n_clusters: 5, // 5 clusters, only 3 distinct directions -> 2 empty/iter
            n_iterations: 5,
            seed: 42,
        };
        let index = IvfIndex::build(&vectors, &config).expect("build must succeed");
        assert_eq!(index.n_clusters(), 5);
        assert_eq!(index.assignments().len(), vectors.len());
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

/// #10197 Wave 1: mutation guards for the IVF index internals.
///
/// The full-crate measurement (run 31027028682) left 66 surviving mutants
/// here. The existing tests assert clustering OUTCOMES loosely (members end up
/// together), which k-means reaches even with a perturbed interior — so the
/// arithmetic inside the seeded pipeline could flip without a failure. These
/// guards pin the pure helpers with known-answer cases and the seeded builder
/// with an exact-determinism snapshot: the algorithm is documented as
/// "reproducible k-means++", so exact same-seed output is contract, not
/// implementation accident.
#[cfg(test)]
mod mutation_guard_tests {
    use super::*;
    use std::collections::HashSet;

    // ---- Rng: xorshift64 is a known-answer function ----------------------

    #[test]
    fn rng_zero_seed_is_coerced_to_one() {
        // state 0 is a xorshift fixed point (would emit zeros forever).
        let mut zero_seeded = Rng::new(0);
        let mut one_seeded = Rng::new(1);
        assert_eq!(zero_seeded.next_u64(), one_seeded.next_u64());
    }

    #[test]
    fn rng_next_u64_matches_the_xorshift64_reference() {
        // Reference values computed from the xorshift64 definition
        // (x ^= x<<13; x ^= x>>7; x ^= x<<17) starting at state 1. Any flip of
        // a shift direction, constant, or xor collapses this.
        let mut rng = Rng::new(1);
        assert_eq!(rng.next_u64(), 1_082_269_761);
        assert_eq!(rng.next_u64(), 1_152_992_998_833_853_505);
    }

    #[test]
    fn rng_next_f64_is_the_53_bit_unit_interval_projection() {
        // next_f64 = (next_u64 >> 11) / 2^53 — pinned exactly for seed 1, and
        // range-checked over a run so constant-replacement mutants die.
        let mut rng = Rng::new(1);
        let expected = (1_082_269_761u64 >> 11) as f64 / (1u64 << 53) as f64;
        let got = rng.next_f64();
        assert!(
            (got - expected).abs() < f64::EPSILON,
            "got {got}, want {expected}"
        );

        let mut rng = Rng::new(42);
        for _ in 0..64 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "next_f64 out of [0,1): {v}");
        }
    }

    // ---- l2_normalize ----------------------------------------------------

    #[test]
    fn l2_normalize_produces_exact_unit_components() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6, "3/5 component, got {}", v[0]);
        assert!((v[1] - 0.8).abs() < 1e-6, "4/5 component, got {}", v[1]);
    }

    #[test]
    fn l2_normalize_leaves_a_zero_vector_untouched() {
        // The norm guard must be a strict `>`-style comparison against EPSILON:
        // dividing a zero vector by its zero norm would produce NaNs that then
        // poison every cosine similarity downstream.
        let mut v = vec![0.0f32, 0.0, 0.0];
        l2_normalize(&mut v);
        assert_eq!(v, vec![0.0, 0.0, 0.0]);
    }

    // ---- cosine_sim_f32 --------------------------------------------------

    #[test]
    fn cosine_sim_hits_the_three_reference_angles() {
        assert!((cosine_sim_f32(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine_sim_f32(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine_sim_f32(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_sim_pins_a_non_trivial_quotient() {
        // ([1,2]·[3,4]) / (|[1,2]|·|[3,4]|) = 11 / (√5·5) ≈ 0.98386991.
        // A dot-product `*`->`+`, a norm `x*x` flip, or the final `/` mutation
        // all move this value; the three reference angles alone would not
        // catch every one of them.
        let sim = cosine_sim_f32(&[1.0, 2.0], &[3.0, 4.0]);
        assert!((sim - 0.983_87).abs() < 1e-5, "got {sim}");
    }

    #[test]
    fn cosine_sim_zero_norm_guards_are_each_sufficient() {
        // Each `||` arm alone must force 0.0 — a zero on either side.
        assert_eq!(cosine_sim_f32(&[0.0, 0.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine_sim_f32(&[1.0, 2.0], &[0.0, 0.0]), 0.0);
    }

    // ---- farthest_untaken_vector ----------------------------------------

    #[test]
    fn farthest_untaken_prefers_the_largest_distance_and_respects_taken() {
        // One centroid at [1,0]. Vector 0 sits on it (dist 0), vector 1 is
        // orthogonal (dist 1), vector 2 is opposite (dist 2). Farthest is 2;
        // with 2 taken, the next farthest is 1; with all taken, None.
        let dequantized = vec![vec![1.0f32, 0.0], vec![0.0, 1.0], vec![-1.0, 0.0]];
        let assignments = vec![0usize, 0, 0];
        let centroids = vec![vec![1.0f32, 0.0]];

        let mut taken = HashSet::new();
        assert_eq!(
            farthest_untaken_vector(&dequantized, &assignments, &centroids, &taken),
            Some(2)
        );
        taken.insert(2);
        assert_eq!(
            farthest_untaken_vector(&dequantized, &assignments, &centroids, &taken),
            Some(1)
        );
        taken.insert(1);
        taken.insert(0);
        assert_eq!(
            farthest_untaken_vector(&dequantized, &assignments, &centroids, &taken),
            None
        );
    }

    // ---- build: seeded determinism is the contract -----------------------

    fn quantize(values: &[f32]) -> QuantizedVector {
        ScalarQuantizer::quantize(values).unwrap()
    }

    /// Two tight, well-separated 2-D clusters. Small enough that the exact
    /// assignment is forced, not statistical.
    fn two_cluster_fixture() -> Vec<(i64, QuantizedVector)> {
        vec![
            (10, quantize(&[1.0, 0.02])),
            (11, quantize(&[0.98, 0.05])),
            (12, quantize(&[0.99, -0.03])),
            (20, quantize(&[0.02, 1.0])),
            (21, quantize(&[-0.03, 0.97])),
            (22, quantize(&[0.05, 0.99])),
        ]
    }

    #[test]
    fn build_is_deterministic_for_a_fixed_seed() {
        // "Reproducible k-means++" is the documented contract. Any interior
        // arithmetic flip (the weighted-sampling `-`/`*`, the `+=` accumulators,
        // the comparison directions) changes the seeded trajectory, and this
        // equality is the net that catches whichever branch it lands in.
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 10,
            seed: 7,
        };
        let a = IvfIndex::build(&two_cluster_fixture(), &config).unwrap();
        let b = IvfIndex::build(&two_cluster_fixture(), &config).unwrap();
        for id in [10, 11, 12, 20, 21, 22] {
            assert_eq!(
                a.assignments().get(&id),
                b.assignments().get(&id),
                "same seed must yield the same assignment for {id}"
            );
        }
    }

    #[test]
    fn build_separates_the_two_obvious_clusters_completely() {
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 10,
            seed: 7,
        };
        let index = IvfIndex::build(&two_cluster_fixture(), &config).unwrap();

        let cluster_of = |id: i64| *index.assignments().get(&id).expect("assigned");
        // The x-axis trio must share a cluster, the y-axis trio the other.
        assert_eq!(cluster_of(10), cluster_of(11));
        assert_eq!(cluster_of(10), cluster_of(12));
        assert_eq!(cluster_of(20), cluster_of(21));
        assert_eq!(cluster_of(20), cluster_of(22));
        assert_ne!(cluster_of(10), cluster_of(20), "clusters must be distinct");

        // get_cluster_members inverts the same map: each cluster reports
        // exactly its trio.
        let mut members = index.get_cluster_members(cluster_of(10));
        members.sort_unstable();
        assert_eq!(members, vec![10, 11, 12]);
        let mut members = index.get_cluster_members(cluster_of(20));
        members.sort_unstable();
        assert_eq!(members, vec![20, 21, 22]);
    }

    // ---- Round 2 (#10197): the first-round nets had two real holes ------
    //
    // The re-measurement (34 survivors) exposed both:
    // 1. `build_is_deterministic_for_a_fixed_seed` compared two runs of the
    //    SAME binary — a deterministic mutant is wrong identically in both, so
    //    a == b held. Determinism cannot kill deterministic mutations; only
    //    values pinned against the ORIGINAL trajectory can.
    // 2. `build_separates_...` asserted grouping (same/different cluster),
    //    which is invariant under label swap — `>` -> `<` in the assignment
    //    loop sends each trio to the OPPOSITE centroid and the grouping still
    //    passes. The centroid CONTENT has to be asserted too.

    /// Line 121 boundary: exactly n == k vectors is legal (one vector per
    /// cluster); `<` -> `<=` would reject it.
    #[test]
    fn build_accepts_exactly_as_many_vectors_as_clusters() {
        let vectors = vec![(1, quantize(&[1.0, 0.0])), (2, quantize(&[0.0, 1.0]))];
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 1,
            seed: 3,
        };
        let index = IvfIndex::build(&vectors, &config).expect("n == k must be buildable");
        assert_eq!(index.n_clusters(), 2);
        assert!(
            !index.centroids().is_empty(),
            "centroids accessor must expose the built set"
        );
    }

    /// With ZERO Lloyd iterations the produced centroids ARE the k-means++
    /// seed picks, so the seeding arithmetic (min-dist update, d*d weights,
    /// cumulative walk, threshold comparison) is pinned directly: for this
    /// fixture every pick lands in a distinct group, whichever seed is used,
    /// because the weighted walk makes a same-group second pick (weight ~0)
    /// unreachable. A flipped `-`, `*`, `+=` or `>=` in that walk collapses
    /// the picks into one group.
    #[test]
    fn kmeanspp_seeding_picks_centroids_from_distinct_groups() {
        // Three tight groups far apart on the sphere.
        let vectors = vec![
            (1, quantize(&[1.0, 0.0, 0.0])),
            (2, quantize(&[0.99, 0.01, 0.0])),
            (3, quantize(&[0.0, 1.0, 0.0])),
            (4, quantize(&[0.01, 0.99, 0.0])),
            (5, quantize(&[0.0, 0.0, 1.0])),
            (6, quantize(&[0.0, 0.01, 0.99])),
        ];
        for seed in [1, 7, 42, 1234] {
            let config = IvfBuildConfig {
                n_clusters: 3,
                n_iterations: 0,
                seed,
            };
            let index = IvfIndex::build(&vectors, &config).expect("build");
            // Identify each seeded centroid's dominant axis; all three axes
            // must be represented — that is only true when the distance-
            // weighted walk actually walks.
            let mut axes: Vec<usize> = index
                .centroids()
                .iter()
                .map(|c| {
                    let v = ScalarQuantizer::dequantize(&c.vector);
                    let mut best = 0;
                    for (d, x) in v.iter().enumerate() {
                        if x.abs() > v[best].abs() {
                            best = d;
                        }
                    }
                    best
                })
                .collect();
            axes.sort_unstable();
            assert_eq!(
                axes,
                vec![0, 1, 2],
                "seed {seed}: seeding must cover all three groups"
            );
        }
    }

    /// The `total < EPSILON` degenerate branch: all vectors identical means
    /// zero total weight, and the fallback picks SOME vector — which is the
    /// same vector. The centroids must all equal it.
    #[test]
    fn kmeanspp_seeding_survives_an_all_identical_input() {
        let vectors = vec![
            (1, quantize(&[0.6, 0.8])),
            (2, quantize(&[0.6, 0.8])),
            (3, quantize(&[0.6, 0.8])),
        ];
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 0,
            seed: 9,
        };
        let index = IvfIndex::build(&vectors, &config).expect("degenerate build");
        for c in index.centroids() {
            let v = ScalarQuantizer::dequantize(&c.vector);
            assert!((v[0] - 0.6).abs() < 0.02 && (v[1] - 0.8).abs() < 0.02);
        }
    }

    /// One cluster, one Lloyd round: the final centroid is exactly
    /// normalize(mean(inputs)) — [1,0] and [0,1] average to [0.5,0.5] and
    /// normalize to [0.7071,0.7071]. This pins the accumulation `+=` (a `-=`
    /// negates a component, a `*=` zeroes the sum) and the count divide.
    #[test]
    fn lloyd_round_computes_the_normalized_mean_centroid() {
        let vectors = vec![(1, quantize(&[1.0, 0.0])), (2, quantize(&[0.0, 1.0]))];
        let config = IvfBuildConfig {
            n_clusters: 1,
            n_iterations: 1,
            seed: 5,
        };
        let index = IvfIndex::build(&vectors, &config).expect("build");
        let centroid = ScalarQuantizer::dequantize(&index.centroids()[0].vector);
        let expected = std::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (centroid[0] - expected).abs() < 0.02 && (centroid[1] - expected).abs() < 0.02,
            "centroid must be the normalized mean [{expected}, {expected}], got {centroid:?}"
        );
    }

    /// Label-swap-proof assignment check: the cluster containing the x-axis
    /// trio must have an x-dominant CENTROID. `>` -> `<` in the best-sim
    /// scan assigns every vector to its FARTHEST centroid; grouping-only
    /// assertions survive that (labels swap), content assertions do not.
    #[test]
    fn assignment_sends_vectors_to_the_centroid_that_matches_them() {
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 10,
            seed: 7,
        };
        let index = IvfIndex::build(&two_cluster_fixture(), &config).expect("build");
        let cluster_of = |id: i64| *index.assignments().get(&id).expect("assigned");

        let x_cluster = cluster_of(10);
        let x_centroid = ScalarQuantizer::dequantize(&index.centroids()[x_cluster].vector);
        assert!(
            x_centroid[0] > 0.9,
            "the x-trio's centroid must be x-dominant, got {x_centroid:?}"
        );

        let y_cluster = cluster_of(20);
        let y_centroid = ScalarQuantizer::dequantize(&index.centroids()[y_cluster].vector);
        assert!(
            y_centroid[1] > 0.9,
            "the y-trio's centroid must be y-dominant, got {y_centroid:?}"
        );
    }

    /// Norm-guard boundaries sit exactly AT f32::EPSILON:
    /// - `l2_normalize` must leave an epsilon-norm vector untouched (`>` not `>=`),
    /// - `cosine_sim_f32` must still COMPUTE at epsilon norms (`<` not `<=`) —
    ///   an epsilon-length vector along +x has similarity 1.0 with +x, and the
    ///   mutant's early 0.0 is unmistakable.
    #[test]
    fn epsilon_norm_boundaries_are_exclusive() {
        let mut v = vec![f32::EPSILON, 0.0];
        l2_normalize(&mut v);
        assert_eq!(
            v,
            vec![f32::EPSILON, 0.0],
            "epsilon norm must not be scaled"
        );

        let sim = cosine_sim_f32(&[f32::EPSILON, 0.0], &[1.0, 0.0]);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "epsilon-norm lhs must still compute, got {sim}"
        );
        let sim = cosine_sim_f32(&[1.0, 0.0], &[f32::EPSILON, 0.0]);
        assert!(
            (sim - 1.0).abs() < 1e-6,
            "epsilon-norm rhs must still compute, got {sim}"
        );
    }

    /// A distance TIE keeps the FIRST candidate (`>` is strict); `>=` would
    /// return the last.
    #[test]
    fn farthest_untaken_keeps_the_first_of_tied_candidates() {
        let dequantized = vec![vec![0.0f32, 1.0], vec![0.0, -1.0]];
        let assignments = vec![0usize, 0];
        let centroids = vec![vec![1.0f32, 0.0]];
        let taken = HashSet::new();
        assert_eq!(
            farthest_untaken_vector(&dequantized, &assignments, &centroids, &taken),
            Some(0),
            "equal distances must keep the first index"
        );
    }

    // ---- Round 3 (#10197): three killable clusters remained --------------

    /// Kills the k-means++ weighting mutants (168 `-`->`/`, 181 `*`->`/`,
    /// 185 `*`->`+`) with one crafted draw. Seed 1's xorshift trajectory is
    /// pinned above: first pick = 1_082_269_761 % 3 = index 0, second draw
    /// next_f64 ≈ 0.0625. With A=[1,0] (dist 0), B=[0.8,0.6] (dist 0.2) and
    /// C=[-1,0] (dist 2), the weighted walk has total = 0.04 + 4 = 4.04 and
    /// threshold ≈ 0.2525, which the walk first crosses AT C — so the second
    /// centroid must be −x-dominant. Under each mutant the walk crosses at A
    /// or B instead (`/`: C's weight collapses to max(0, 1/−1) = 0; `d/d`:
    /// NaN total freezes the walk at index 0; `+`: the threshold lands inside
    /// B's doubled band), so the −x centroid disappears.
    #[test]
    fn kmeanspp_weighted_walk_prefers_the_far_opposite_vector() {
        let vectors = vec![
            (1, quantize(&[1.0, 0.0])),
            (2, quantize(&[0.8, 0.6])),
            (3, quantize(&[-1.0, 0.0])),
        ];
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 0,
            seed: 1,
        };
        let index = IvfIndex::build(&vectors, &config).expect("build");
        let dequantized: Vec<Vec<f32>> = index
            .centroids()
            .iter()
            .map(|c| ScalarQuantizer::dequantize(&c.vector))
            .collect();
        assert!(
            dequantized.iter().any(|v| v[0] > 0.9),
            "seed 1's first pick is A=[1,0]: {dequantized:?}"
        );
        assert!(
            dequantized.iter().any(|v| v[0] < -0.9),
            "the weighted walk must select the far opposite vector C=[-1,0]: {dequantized:?}"
        );
    }

    /// Kills 232 `>`->`>=` (the empty-cluster guard). All-identical input
    /// with TWO clusters and ONE Lloyd round: both seeds are the same vector,
    /// every vector ties to cluster 0, cluster 1 ends EMPTY and must take the
    /// reseed branch. The mutant instead divides the zero accumulator by a
    /// zero count, and the NaN centroid survives normalization (NaN
    /// comparisons are false), so finiteness is the tell.
    #[test]
    fn empty_cluster_is_reseeded_not_divided_by_zero() {
        let vectors = vec![
            (1, quantize(&[0.6, 0.8])),
            (2, quantize(&[0.6, 0.8])),
            (3, quantize(&[0.6, 0.8])),
        ];
        let config = IvfBuildConfig {
            n_clusters: 2,
            n_iterations: 1,
            seed: 9,
        };
        let index = IvfIndex::build(&vectors, &config).expect("build");
        for c in index.centroids() {
            let v = ScalarQuantizer::dequantize(&c.vector);
            assert!(
                v.iter().all(|x| x.is_finite()),
                "an empty cluster must be reseeded, never divided by zero: {v:?}"
            );
            assert!(
                (v[0] - 0.6).abs() < 0.02 && (v[1] - 0.8).abs() < 0.02,
                "every centroid of an identical set is that vector: {v:?}"
            );
        }
    }
}
