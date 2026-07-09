//! #7730: shared `in_memory_storage()` helper — builds an in-memory
//! `SqliteStorage` wrapped as `Arc<dyn WebStorage>`.
//!
//! De-duplicated from verbatim copies that used to live in both
//! `tests/external_grpc_integration.rs` and `tests/external_grpc_stress.rs`.
//!
//! This file is included via `#[path]` from each integration-test crate root
//! independently (mirrors `tests/support/failing_storage.rs`). It lives
//! under `tests/support/` — NOT in the `maekon-web` library's
//! `test_support.rs` — because it needs `maekon-storage`, an adapter crate:
//! `scripts/check-crate-boundaries.sh` forbids `maekon-web` from taking
//! `maekon-storage` as a normal (even optional/feature-gated) `[dependencies]`
//! entry; only `[dev-dependencies]` are exempt, and code under `tests/` gets
//! `[dev-dependencies]` for free without tripping that gate. No inner
//! `#![cfg]` attribute is needed — the caller's crate-root gate is
//! sufficient.

use std::sync::Arc;

use maekon_storage::sqlite::SqliteStorage;
use maekon_web::storage_port::WebStorage;

/// Build an in-memory `SqliteStorage` wrapped as `Arc<dyn WebStorage>`.
pub(crate) fn in_memory_storage() -> Arc<dyn WebStorage> {
    Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite")) as Arc<dyn WebStorage>
}
