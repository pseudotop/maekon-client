//! Frame storage — screenshot/OCR frame persistence.
//!
//! Module layout (ADR-013 split from 1171-line monolith, OOS-TBD #3704):
//!
//! - `buffer`     — lock-free buffer pool (`BufferPool`)
//! - `disk`       — disk-space cache and platform-specific free-space query
//! - `fs`         — `FrameFileStorage` struct + constructors + accessors
//! - `io`         — read/write operations (save_frame, load_frame, batch variants)
//! - `retention`  — retention policy, storage-limit enforcement, GDPR delete-all
//! - `util`       — async filesystem helpers shared by io and retention
//! - `port_impl`  — `FrameStoragePort` impl (delegates to the concrete methods)
//! - `tests`      — integration tests

pub(super) mod buffer;
pub(super) mod disk;
mod fs;
mod io;
mod port_impl;
mod retention;
pub(super) mod util;

#[cfg(test)]
mod tests;

// Public re-exports — preserves `maekon_storage::frame_storage::{…}` compatibility.
pub use disk::DiskStatus;
pub use fs::FrameFileStorage;

/// Snapshot of buffer-pool sizing parameters.
#[derive(Debug, Clone)]
pub struct BufferPoolStats {
    pub pool_capacity: usize,
    pub buffer_size: usize,
}
