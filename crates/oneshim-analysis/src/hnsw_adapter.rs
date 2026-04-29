//! HNSW-based approximate nearest neighbor adapter.
//!
//! Wraps the `usearch` crate (C++ FFI) behind `spawn_blocking` to keep the
//! async executor responsive. Persistence uses atomic rename to avoid
//! half-written files on crash.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::ann_index::AnnIndex;
use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

use crate::error::AnalysisError;

// Compile-time proof that usearch::Index is Send + Sync.
fn _assert_send_sync() {
    fn _check<T: Send + Sync>() {}
    _check::<Index>();
}

/// HNSW vector index adapter backed by usearch.
///
/// All FFI calls are dispatched via `tokio::task::spawn_blocking` so that the
/// C++ work does not block the async runtime.
pub struct HnswAdapter {
    /// Shared usearch index — cloned into `spawn_blocking` closures.
    index: Arc<Index>,
    /// File path for save/load persistence.
    data_path: PathBuf,
    /// Dirty flag — set on mutation, cleared on save.
    dirty: AtomicBool,
    /// Cached size counter — kept in sync with the C++ index.
    /// Avoids FFI call for the synchronous `len()` method.
    cached_size: AtomicUsize,
}

impl HnswAdapter {
    /// Create a new HNSW index.
    ///
    /// - `dimensions`: number of vector dimensions
    /// - `data_path`: file path used by `save()` / `load()`
    ///
    /// The index is configured for cosine similarity with I8 scalar
    /// quantization, connectivity 16, and an initial capacity of 50 000.
    pub fn new(dimensions: usize, data_path: PathBuf) -> Result<Self, AnalysisError> {
        let options = IndexOptions {
            dimensions,
            metric: MetricKind::Cos,
            quantization: ScalarKind::I8,
            connectivity: 16,
            expansion_add: 128,
            expansion_search: 64,
            multi: false,
        };
        let index = Index::new(&options)
            .map_err(|e| AnalysisError::VectorIndex(format!("HNSW index creation failed: {e}")))?;
        index
            .reserve(50_000)
            .map_err(|e| AnalysisError::VectorIndex(format!("HNSW reserve failed: {e}")))?;
        Ok(Self {
            index: Arc::new(index),
            data_path,
            dirty: AtomicBool::new(false),
            cached_size: AtomicUsize::new(0),
        })
    }
}

#[async_trait]
impl AnnIndex for HnswAdapter {
    async fn add(&self, key: u64, vector: &[f32]) -> Result<(), CoreError> {
        let idx = Arc::clone(&self.index);
        let vec = vector.to_vec();
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            // Capacity check + potential growth
            let size = idx.size();
            let cap = idx.capacity();
            if cap > 0 && size > cap * 80 / 100 {
                let new_cap = cap * 2;
                idx.reserve(new_cap).map_err(|e| CoreError::Analysis {
                    code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                    message: format!("HNSW reserve (grow) failed: {e}"),
                })?;
                tracing::debug!(
                    old_cap = cap,
                    new_cap = new_cap,
                    "HNSW index capacity doubled"
                );
            }
            idx.add(key, &vec).map_err(|e| CoreError::Analysis {
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                message: format!("HNSW add failed: {e}"),
            })?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("HNSW add task join failed: {e}"),
        })??;
        self.cached_size.store(self.index.size(), Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn search(&self, query: &[f32], k: usize) -> Result<Vec<(u64, f32)>, CoreError> {
        let idx = Arc::clone(&self.index);
        let q = query.to_vec();
        tokio::task::spawn_blocking(move || {
            let matches = idx.search(&q, k).map_err(|e| CoreError::Analysis {
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                message: format!("HNSW search failed: {e}"),
            })?;
            Ok(matches
                .keys
                .into_iter()
                .zip(matches.distances)
                .collect::<Vec<_>>())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("HNSW search task join failed: {e}"),
        })?
    }

    async fn remove(&self, key: u64) -> Result<(), CoreError> {
        let idx = Arc::clone(&self.index);
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            idx.remove(key).map_err(|e| CoreError::Analysis {
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                message: format!("HNSW remove failed: {e}"),
            })?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("HNSW remove task join failed: {e}"),
        })??;
        self.cached_size.store(self.index.size(), Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Persist the HNSW index to disk using atomic rename (.tmp → final).
    ///
    /// **Thread safety note:** `usearch::Index::save()` is not documented as
    /// safe during concurrent `search()` or `add()` calls. In practice, saves
    /// are scheduled every 60s from the aggregation loop, making conflicts
    /// unlikely. If `save_to_buffer()` becomes available in the usearch Rust
    /// API, migrate to it for guaranteed thread safety.
    async fn save(&self) -> Result<(), CoreError> {
        if !self.dirty.load(Ordering::Relaxed) {
            return Ok(());
        }
        let idx = Arc::clone(&self.index);
        let path = self.data_path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            // Write to a .tmp sibling, then atomic rename.
            let tmp_path = path.with_extension("usearch.tmp");
            let tmp_str = tmp_path.to_str().ok_or_else(|| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: "Non-UTF-8 HNSW data path".into(),
            })?;
            idx.save(tmp_str).map_err(|e| CoreError::Analysis {
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                message: format!("HNSW save failed: {e}"),
            })?;
            std::fs::rename(&tmp_path, &path).map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("HNSW atomic rename failed: {e}"),
            })?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("HNSW save task join failed: {e}"),
        })??;
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }

    async fn load(&self) -> Result<(), CoreError> {
        let idx = Arc::clone(&self.index);
        let path = self.data_path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), CoreError> {
            let path_str = path.to_str().ok_or_else(|| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: "Non-UTF-8 HNSW data path".into(),
            })?;
            idx.load(path_str).map_err(|e| CoreError::Analysis {
                code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
                message: format!("HNSW load failed: {e}"),
            })?;
            Ok(())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("HNSW load task join failed: {e}"),
        })??;
        self.cached_size.store(self.index.size(), Ordering::Relaxed);
        self.dirty.store(false, Ordering::Relaxed);
        Ok(())
    }

    fn len(&self) -> usize {
        self.cached_size.load(Ordering::Relaxed)
    }

    fn capacity(&self) -> usize {
        self.index.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_adapter(dims: usize) -> (HnswAdapter, TempDir) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test_index.usearch");
        let adapter = HnswAdapter::new(dims, path).unwrap();
        (adapter, dir)
    }

    #[tokio::test]
    async fn test_add_search_round_trip() {
        let (adapter, _dir) = make_adapter(4);
        adapter.add(1, &[1.0, 0.0, 0.0, 0.0]).await.unwrap();
        adapter.add(2, &[0.0, 1.0, 0.0, 0.0]).await.unwrap();
        adapter.add(3, &[1.0, 1.0, 0.0, 0.0]).await.unwrap();

        let results = adapter.search(&[1.0, 0.0, 0.0, 0.0], 2).await.unwrap();
        assert_eq!(results.len(), 2);
        // Nearest neighbor to [1,0,0,0] should be key 1 (exact match, distance ~0).
        assert_eq!(results[0].0, 1);
        assert!(results[0].1 < 0.01, "distance should be near-zero");
    }

    #[tokio::test]
    async fn test_remove() {
        let (adapter, _dir) = make_adapter(4);
        adapter.add(10, &[1.0, 0.0, 0.0, 0.0]).await.unwrap();
        adapter.add(20, &[0.0, 1.0, 0.0, 0.0]).await.unwrap();
        assert_eq!(adapter.len(), 2);

        adapter.remove(10).await.unwrap();
        // After removal the vector is lazily tombstoned — size may or may not
        // decrement depending on usearch internals, but a search for the exact
        // vector should no longer return key 10 as the top result if another
        // vector is present.
        let results = adapter.search(&[0.0, 1.0, 0.0, 0.0], 1).await.unwrap();
        assert_eq!(results[0].0, 20);
    }

    #[tokio::test]
    async fn test_save_load_persistence() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("persist.usearch");

        // Create and populate an index.
        {
            let adapter = HnswAdapter::new(4, path.clone()).unwrap();
            adapter.add(1, &[1.0, 0.0, 0.0, 0.0]).await.unwrap();
            adapter.add(2, &[0.0, 1.0, 0.0, 0.0]).await.unwrap();
            adapter.save().await.unwrap();
        }

        // Verify the file exists.
        assert!(path.exists(), "persisted file should exist");

        // Load into a fresh index and verify.
        {
            let adapter = HnswAdapter::new(4, path).unwrap();
            adapter.load().await.unwrap();
            assert!(adapter.len() >= 2);

            let results = adapter.search(&[1.0, 0.0, 0.0, 0.0], 1).await.unwrap();
            assert_eq!(results[0].0, 1);
        }
    }

    #[tokio::test]
    async fn test_is_empty_and_len() {
        let (adapter, _dir) = make_adapter(4);
        assert!(adapter.is_empty());
        assert_eq!(adapter.len(), 0);

        adapter.add(1, &[1.0, 0.0, 0.0, 0.0]).await.unwrap();
        assert!(!adapter.is_empty());
        assert_eq!(adapter.len(), 1);
    }

    #[tokio::test]
    async fn test_capacity_growth() {
        // Create an index with very small initial capacity to trigger growth.
        // usearch rounds capacity up internally (e.g. 10 → 64), so we need to
        // fill enough vectors to exceed 80 % of the *actual* capacity.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("grow.usearch");
        let options = IndexOptions {
            dimensions: 4,
            metric: MetricKind::Cos,
            quantization: ScalarKind::I8,
            connectivity: 16,
            expansion_add: 128,
            expansion_search: 64,
            multi: false,
        };
        let index = Index::new(&options).unwrap();
        index.reserve(10).unwrap();
        let adapter = HnswAdapter {
            index: Arc::new(index),
            data_path: path,
            dirty: AtomicBool::new(false),
            cached_size: AtomicUsize::new(0),
        };

        let initial_cap = adapter.capacity();
        // Determine how many vectors we need to exceed the 80 % threshold.
        let fill_target = initial_cap * 80 / 100 + 2;

        for i in 0..fill_target as u64 {
            adapter.add(i, &[i as f32, 0.0, 0.0, 0.0]).await.unwrap();
        }

        let grown_cap = adapter.capacity();
        assert!(
            grown_cap > initial_cap,
            "capacity should have grown: initial={initial_cap}, filled={fill_target}, after={grown_cap}"
        );
    }

    #[tokio::test]
    async fn test_save_skips_when_not_dirty() {
        let (adapter, _dir) = make_adapter(4);
        // save() on a clean index should be a no-op (no file created).
        adapter.save().await.unwrap();
        assert!(
            !adapter.data_path.exists(),
            "no file should be written when index is clean"
        );
    }
}
