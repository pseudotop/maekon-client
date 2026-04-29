//! `AuditQuery` adapter — bridges `oneshim-automation::audit::AuditQuery`
//! (library trait) to `oneshim_storage::SqliteStorage` (infrastructure).
//!
//! Per ADR-001 §1, `oneshim-automation` MUST NOT depend on `oneshim-storage`
//! directly. The binary crate (`src-tauri`) wires the bridge so
//! `AuditLogger::entries_by_command_id` can fall through from the in-memory
//! buffer (~1000-row cap) to historical persistence on the V32 partial index.
//!
//! Mirrors the existing write-path bridge — see `app_runtime_launch.rs` and
//! `web_server_runtime.rs` for the `AuditPersistence` callback pattern.

use std::sync::Arc;

use oneshim_automation::audit::AuditQuery;
use oneshim_core::models::audit::AuditEntry;
use oneshim_storage::sqlite::SqliteStorage;

/// SQLite-backed implementation of [`AuditQuery`].
///
/// Holds an `Arc<SqliteStorage>` and forwards `entries_by_command_id` to the
/// V32-indexed query method on storage. Read-only — never mutates.
pub(crate) struct SqliteAuditQuery {
    storage: Arc<SqliteStorage>,
}

impl SqliteAuditQuery {
    pub(crate) fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

impl AuditQuery for SqliteAuditQuery {
    fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
        self.storage.entries_by_command_id(command_id, limit)
    }
}
