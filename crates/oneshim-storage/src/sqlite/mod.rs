mod annotation_storage_impl;
mod calibration_store_impl;
mod coaching_storage;
mod coaching_storage_port_impl;
mod dashboard_streaming;
mod device_identity;
pub(crate) mod edge_intelligence;
mod events;
mod few_shot_storage_impl;
mod focus_storage_impl;
mod frames;
mod fts_search_impl;
mod habit_storage;
mod integration_query_impl;
mod lan_pin_store;
mod maintenance;
mod metrics;
mod override_store_impl;
mod preset_storage_impl;
mod session_context_store_impl;
mod session_storage_impl;
mod tags;
pub mod vector_index_impl;
pub mod vector_store_impl;
mod web_storage_impl;

#[cfg(test)]
mod port_contract_tests;
#[cfg(test)]
pub(crate) mod test_utils;
#[cfg(test)]
mod tests;

use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

use crate::migration;

/// Process-global flag indicating whether the `search_fts` FTS5 table exists.
///
/// Set once after migrations complete in `open()` / `open_in_memory()`.
/// This avoids per-operation `sqlite_master` queries in the FTS hot path.
///
/// # Thread-safety in tests
///
/// Parallel test instances each run migrations, so FTS is always available
/// and this global flag being `true` is correct for all concurrent tests.
pub(super) static FTS_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Process-global flag indicating whether the `gui_interactions` table exists (V13 migration).
///
/// Same rationale and thread-safety guarantees as [`FTS_AVAILABLE`].
pub(super) static GUI_INTERACTIONS_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Local SQLite storage with a single-connection, Mutex-guarded design.
///
/// # Connection design
///
/// This store uses a single `Connection` behind a `Mutex` rather than a
/// connection pool. The rationale:
///
/// 1. **WAL mode** (`PRAGMA journal_mode=WAL`) allows concurrent readers
///    from the OS level, but rusqlite's `Connection` is not `Sync`, so we
///    still need a Mutex for Rust's thread-safety requirements.
/// 2. All blocking SQLite operations are offloaded to `spawn_blocking`,
///    which prevents the Mutex from starving the async runtime.
/// 3. A full read/write pool (e.g. r2d2 + separate read-only connections)
///    adds complexity without measurable benefit for our workload profile:
///    the scheduler ticks at 1-10 Hz and queries complete in <1ms.
///
/// If profiling reveals lock contention, the next step would be opening a
/// second read-only connection (`SQLITE_OPEN_READ_ONLY`) and routing
/// SELECT-only queries through it. The [`read_only_query`](Self::read_only_query)
/// helper already enforces the "acquire lock, clone data out, release lock"
/// pattern to minimise the critical section.
pub struct SqliteStorage {
    pub(super) conn: Arc<Mutex<Connection>>,
    pub(super) retention_days: u32,
}

impl SqliteStorage {
    /// Open a disk-backed SQLite database.
    ///
    /// When `encryption_key` is `Some`, SQLCipher `PRAGMA key` is applied after
    /// opening. If the database was previously unencrypted, the key verification
    /// will fail and the database is reopened **without** encryption so that
    /// existing data is not lost. A warning is logged in this case.
    pub fn open(
        path: &Path,
        retention_days: u32,
        encryption_key: Option<&EncryptionKey>,
    ) -> Result<Self, StorageError> {
        let conn = Connection::open(path)
            .map_err(|e| StorageError::Internal(format!("Failed to open SQLite database: {e}")))?;

        let conn = apply_sqlcipher_key(conn, path, encryption_key)?;

        configure_connection(&conn, true)?;

        migration::run_migrations(&conn)
            .map_err(|e| StorageError::Internal(format!("migration failure: {e}")))?;

        post_migration_setup(&conn)?;

        info!("SQLite save initialize: {}", path.display());

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            retention_days,
        })
    }

    pub fn open_in_memory(retention_days: u32) -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory().map_err(|e| {
            StorageError::Internal(format!("Failed to create in-memory SQLite database: {e}"))
        })?;

        configure_connection(&conn, false)?;

        migration::run_migrations(&conn)
            .map_err(|e| StorageError::Internal(format!("migration failure: {e}")))?;

        post_migration_setup(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            retention_days,
        })
    }

    /// Expose the underlying connection Arc for shared-connection adapters
    /// (e.g., `SqliteVectorStore`).
    pub fn connection_arc(&self) -> Arc<Mutex<Connection>> {
        self.conn.clone()
    }

    /// 동기 SQLite 읽기/단순 쓰기 연산을 spawn_blocking으로 격리한다.
    /// 클로저는 커넥션의 공유 참조를 받는다.
    pub(super) async fn with_conn<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
            f(&guard)
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    /// 동기 SQLite 트랜잭션 연산을 spawn_blocking으로 격리한다.
    /// 클로저는 커넥션의 배타적(가변) 참조를 받는다.
    #[allow(dead_code)]
    pub(super) async fn with_conn_mut<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
            f(&mut guard)
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    /// Execute a read-only query with a short-lived lock scope.
    ///
    /// The closure `f` receives a `&Connection` and must clone/copy the
    /// data it needs into a fully-owned `T`. The Mutex is released as soon
    /// as `f` returns, before the `spawn_blocking` future completes, so
    /// writers are not blocked while the caller processes the result.
    ///
    /// This is the recommended pattern for pure SELECT queries that return
    /// small to medium result sets (e.g., config lookups, aggregate stats).
    /// For large result sets, consider streaming via `with_conn` with
    /// incremental fetching.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let count: i64 = storage.read_only_query(|conn| {
    ///     conn.query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
    ///         .map_err(|e| StorageError::Internal(e.to_string()))
    /// }).await?;
    /// ```
    pub async fn read_only_query<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            // Acquire lock, execute query, release lock -- all within the
            // blocking thread. The result `T` is owned so the lock is not
            // held while the async runtime schedules the continuation.
            let guard = conn
                .lock()
                .map_err(|e| StorageError::Internal(format!("SQLite lock poisoned: {e}")))?;
            f(&guard)
            // guard drops here, releasing the Mutex
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    // ── app_meta key-value helpers (V19) ────────────────────────────

    /// Retrieve a value from the `app_meta` table, or `None` if the key does not exist.
    pub fn get_meta(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().ok()?;
        conn.query_row("SELECT value FROM app_meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .ok()
    }

    /// Insert or replace a value in the `app_meta` table.
    pub fn set_meta(&self, key: &str, value: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            );
        }
    }

    /// Delete a key from the `app_meta` table.
    pub fn delete_meta(&self, key: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute("DELETE FROM app_meta WHERE key = ?1", [key]);
        }
    }
}

// ── Audit log persistence (V25) ────────────────────────────

impl SqliteStorage {
    /// Persist a single audit entry to the `audit_log` table (V25).
    ///
    /// Designed to be called from a persistence callback wired by `src-tauri`.
    /// Failures are logged and swallowed to avoid disrupting the audit buffer.
    pub fn save_audit_entry(&self, entry: &oneshim_core::models::audit::AuditEntry) {
        let Ok(conn) = self.conn.lock() else {
            warn!("audit persistence: failed to acquire SQLite lock");
            return;
        };

        let status_str = format!("{:?}", entry.status);
        let timestamp_str = entry.timestamp.to_rfc3339();
        let exec_time = entry.execution_time_ms.map(|v| v as i64);

        if let Err(e) = conn.execute(
            "INSERT OR IGNORE INTO audit_log \
             (entry_id, timestamp, session_id, command_id, action_type, status, details, execution_time_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                entry.entry_id,
                timestamp_str,
                entry.session_id,
                entry.command_id,
                entry.action_type,
                status_str,
                entry.details,
                exec_time,
            ],
        ) {
            warn!("audit persistence: INSERT failed: {e}");
        }
    }
}

impl SqliteStorage {
    /// Return audit entries whose `command_id` equals the given value, ordered
    /// newest-first, up to `limit` rows.
    ///
    /// Synchronous, matching the existing [`Self::save_audit_entry`] pattern.
    /// Async callers wrap at the Adapter layer. Infallible — logs `warn!` on
    /// SQLite error and returns an empty `Vec`.
    ///
    /// Not an `impl AuditLogPort` — `SqliteStorage` does not implement the
    /// port trait directly. The `AuditLogAdapter` (in `oneshim-automation`)
    /// holds `Arc<RwLock<AuditLogger>>` and may delegate here as a fall-through
    /// in a future task.
    pub fn entries_by_command_id(
        &self,
        command_id: &str,
        limit: usize,
    ) -> Vec<oneshim_core::models::audit::AuditEntry> {
        use oneshim_core::models::audit::{AuditEntry, AuditStatus};

        let Ok(conn) = self.conn.lock() else {
            warn!("audit: entries_by_command_id failed to acquire SQLite lock");
            return Vec::new();
        };

        let mut stmt = match conn.prepare(
            "SELECT entry_id, timestamp, session_id, command_id, action_type,
                    status, details, execution_time_ms
             FROM audit_log
             WHERE command_id = ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!(err = %e, "audit: entries_by_command_id prepare failed");
                return Vec::new();
            }
        };

        let mapped = stmt.query_map(rusqlite::params![command_id, limit as i64], |row| {
            let ts_str: String = row.get("timestamp")?;
            let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?;

            let status_str: String = row.get("status")?;
            let status = match status_str.as_str() {
                "Completed" => AuditStatus::Completed,
                "Failed" => AuditStatus::Failed,
                "Denied" => AuditStatus::Denied,
                "Timeout" => AuditStatus::Timeout,
                "Started" => AuditStatus::Started,
                _ => AuditStatus::Completed, // forward-compat default
            };

            let etime: Option<i64> = row.get("execution_time_ms").ok();
            Ok(AuditEntry {
                entry_id: row.get("entry_id")?,
                timestamp,
                session_id: row.get("session_id")?,
                command_id: row.get("command_id")?,
                action_type: row.get("action_type")?,
                status,
                details: row.get("details").ok(),
                execution_time_ms: etime.map(|v| v as u64),
            })
        });

        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                warn!(err = %e, "audit: entries_by_command_id query_map failed");
                Vec::new()
            }
        }
    }
}

/// Apply SQLCipher `PRAGMA key` and verify the key works.
///
/// If the key is rejected (e.g. database was previously unencrypted), falls back
/// to a fresh connection without encryption so existing data is preserved.
fn apply_sqlcipher_key(
    conn: Connection,
    path: &Path,
    encryption_key: Option<&EncryptionKey>,
) -> Result<Connection, StorageError> {
    let Some(key) = encryption_key else {
        return Ok(conn);
    };

    // PRAGMA key must be the first statement after opening.
    let pragma = format!("PRAGMA key = \"x'{}'\";", key.as_hex());
    if let Err(e) = conn.execute_batch(&pragma) {
        warn!("SQLCipher PRAGMA key execution failed: {e} — opening without encryption");
        drop(conn);
        let fallback = Connection::open(path).map_err(|e| {
            StorageError::Internal(format!("Failed to reopen SQLite database: {e}"))
        })?;
        return Ok(fallback);
    }

    // Verify the key actually works by reading sqlite_master.
    match conn.execute_batch("SELECT count(*) FROM sqlite_master;") {
        Ok(()) => Ok(conn),
        Err(_) => {
            warn!(
                "SQLCipher key verification failed — database may be unencrypted, reopening without encryption"
            );
            drop(conn);
            let fallback = Connection::open(path).map_err(|e| {
                StorageError::Internal(format!("Failed to reopen SQLite database: {e}"))
            })?;
            Ok(fallback)
        }
    }
}

/// Apply PRAGMA settings to a freshly opened connection.
///
/// * `is_disk=true` — all PRAGMAs (WAL, synchronous, cache_size, temp_store,
///   mmap_size, page_size, journal_size_limit).
/// * `is_disk=false` — only PRAGMAs that are meaningful for in-memory databases
///   (cache_size, temp_store). WAL, mmap_size, journal_size_limit, and page_size
///   are skipped because they have no effect on `:memory:` connections.
fn configure_connection(conn: &Connection, is_disk: bool) -> Result<(), StorageError> {
    if is_disk {
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA busy_timeout=5000;
            PRAGMA synchronous=NORMAL;
            PRAGMA cache_size=8000;
            PRAGMA temp_store=MEMORY;
            PRAGMA mmap_size=268435456;
            PRAGMA page_size=4096;
            PRAGMA journal_size_limit=67108864;
            ",
        )
        .map_err(|e| StorageError::Internal(format!("Failed to apply PRAGMA settings: {e}")))?;
    } else {
        conn.execute_batch(
            "
            PRAGMA cache_size=8000;
            PRAGMA temp_store=MEMORY;
            ",
        )
        .map_err(|e| StorageError::Internal(format!("Failed to apply PRAGMA settings: {e}")))?;
    }
    Ok(())
}

/// Post-migration one-time setup: PRAGMA optimize + table-existence caching.
///
/// Called after `run_migrations()` completes in both `open()` and `open_in_memory()`.
fn post_migration_setup(conn: &Connection) -> Result<(), StorageError> {
    // PRAGMA optimize with analysis_limit=1000 + optimize mask 0x10002:
    // - 0x2: run ANALYZE on tables that would benefit
    // - 0x10000: set an internal analysis_limit of 1000 rows
    conn.execute_batch("PRAGMA optimize=0x10002;")
        .map_err(|e| StorageError::Internal(format!("PRAGMA optimize failed: {e}")))?;

    // Cache table existence flags so hot-path code avoids sqlite_master queries.
    let fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='search_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    FTS_AVAILABLE.store(fts_exists, Ordering::Release);

    let gui_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='gui_interactions'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    GUI_INTERACTIONS_AVAILABLE.store(gui_exists, Ordering::Release);

    Ok(())
}

// Record types are canonical in oneshim-core; re-exported here for backward compatibility.
pub use oneshim_core::models::storage_records::{
    DeletedRangeCounts, EventExportRecord, FocusInterruptionRecord, FocusWorkSessionRecord,
    FrameExportRecord, FrameRecord, FrameTagLinkRecord, HourlyMetricsRecord, LocalSuggestionRecord,
    MetricExportRecord, SearchEventRow, SearchFrameRow, StorageStatsSummaryRecord, TagRecord,
};
