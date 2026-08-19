#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-A nursery-hardening. (Enforced workspace-wide via
// `[workspace.lints.clippy]`, #7719.)
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! Local embedding provider — fastembed-rs (ONNX Runtime) wrapper.
//!
//! Wraps the synchronous fastembed `TextEmbedding` API behind the async
//! `EmbeddingProvider` port defined in `maekon-core`.  All blocking calls
//! are dispatched via `tokio::task::spawn_blocking` to avoid starving the
//! async runtime.
//!
//! When the `fastembed-local` feature is disabled (or if the ONNX runtime
//! cannot be loaded on the host platform) a compile-time stub is provided
//! that returns `CoreError::Internal` for every operation so that dependent
//! crates can still compile.

pub mod embedder;
pub mod error;
pub mod fallback;
pub mod model_integrity;
pub mod stub;

pub use error::EmbeddingError;
pub use fallback::FallbackEmbeddingProvider;

#[cfg(feature = "fastembed-local")]
pub use embedder::fastembed_impl::LocalEmbeddingProvider;

#[cfg(not(feature = "fastembed-local"))]
pub use stub::stub_impl::LocalEmbeddingProvider;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod lib_tests;
