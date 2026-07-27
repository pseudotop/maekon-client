mod adaptive_scorer_store_impl;
mod annotation_storage_impl;
mod calibration_store_impl;
pub(crate) mod cjk_shadow;
mod coaching_effectiveness_store_impl;
mod coaching_storage;
mod coaching_storage_port_impl;
mod dashboard_streaming;
mod device_identity;
pub(crate) mod edge_intelligence;
mod events;
pub use events::storage_event_id;
mod extension_registry_impl;
mod feedback_scorer_store_impl;
mod few_shot_storage_impl;
mod focus_storage_impl;
mod frames;
mod fts_search_impl;
pub mod guarded_connection;
mod habit_storage;
pub(crate) mod hlc_clock;
mod integration_query_impl;
mod lan_pin_store;
mod maintenance;
mod memory_graph_impl;
mod metrics;
mod override_store_impl;
mod pomodoro_store_impl;
mod preset_storage_impl;
mod regime_reaction_store_impl;
mod scheduler_storage_impl;
mod session_context_store_impl;
mod session_storage_impl;
mod skill_pack_registry_impl;
mod tags;
mod task_store_impl;
mod transcript_storage_impl;
pub mod vector_index_impl;
pub mod vector_store_impl;
mod web_storage_impl;
mod work_context_store_impl;
mod work_context_writers;

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
use std::sync::Arc;
use tracing::{info, warn};

pub use guarded_connection::{GuardedConnection, ReadGuard, RetainedGuard, WriteGuard};

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

/// Format `instant` as the canonical `system_metrics_hourly.hour` bucket key.
///
/// Single source of truth for the hour-bucket string used at write time
/// (`aggregate_hourly_metrics`), at range-delete time (`delete_data_in_range`),
/// and at scheduled-retention time (`cleanup_old_hourly_metrics`). The bucket
/// key is the instant truncated to the start of its hour, formatted with the
/// `Z` suffix (`%Y-%m-%dT%H:00:00Z`). Keeping a single helper guarantees the
/// stored key and any comparison bound share an identical, lexically-ordered
/// representation — a `to_rfc3339()` bound would carry a `+00:00` offset and
/// sub-hour precision, which sorts differently from the stored `Z` key and
/// orphaned the boundary bucket.
pub(super) fn hour_bucket_key(instant: chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Timelike;
    instant
        .with_minute(0)
        .and_then(|d| d.with_second(0))
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(instant)
        .format("%Y-%m-%dT%H:00:00Z")
        .to_string()
}

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
    /// consent-erasure barrier chokepoint (#4928). The raw `Mutex<Connection>`
    /// accessor has been removed; every SQLite access goes through the
    /// [`GuardedConnection`] funnel.
    pub(super) conn: Arc<GuardedConnection>,
    pub(super) retention_days: u32,
    /// Persistent monotonic HLC clock for stamping local synced-table writes so they
    /// propagate via cross-device sync (F0/#5186). Its own instance is fine — the durable
    /// floor lives in the `hlc_clock` table reached through the same `GuardedConnection`
    /// mutex (shared with `SqliteVectorStore`'s clock), so RMW stays serialized + monotonic.
    pub(super) clock: Arc<hlc_clock::HlcClock>,
    /// #8589 (ADR-030 §7 + Amendment I1): master key for deriving the
    /// work-context raw-plane per-account AEAD subkey. `Some` whenever the
    /// database itself was opened encrypted (production); `None` for the
    /// unencrypted in-memory path, where consented raw writes fail closed rather
    /// than persist plaintext. Wired independently of SQLCipher — the raw plane
    /// is an app-level AEAD on the blob columns, not the SQLCipher page cipher.
    pub(super) work_context_raw_key: Option<EncryptionKey>,
}

impl SqliteStorage {
    /// Open a disk-backed SQLite database.
    ///
    /// When `encryption_key` is `Some`, SQLCipher `PRAGMA key` is applied after
    /// opening. If the database is plaintext or the key cannot unlock it, opening
    /// fails closed instead of silently reopening without encryption.
    pub fn open(
        path: &Path,
        retention_days: u32,
        encryption_key: Option<&EncryptionKey>,
    ) -> Result<Self, StorageError> {
        Self::open_with_max_schema_version(path, retention_days, encryption_key, None)
    }

    /// Open a disk-backed database migrating only up to `max_schema_version`
    /// (`None` = [`migration::CURRENT_VERSION`], i.e. the production path).
    ///
    /// The bounded form exists solely for the QC legacy-migration fixture
    /// (#9083): it produces a REAL older-schema database, replacing the old
    /// approach of migrating fully and then deleting newer-version artifacts —
    /// a forgery that silently broke whenever `CURRENT_VERSION` advanced.
    pub fn open_with_max_schema_version(
        path: &Path,
        retention_days: u32,
        encryption_key: Option<&EncryptionKey>,
        max_schema_version: Option<u32>,
    ) -> Result<Self, StorageError> {
        let conn = Connection::open(path)
            .map_err(|e| StorageError::Internal(format!("Failed to open SQLite database: {e}")))?;

        let conn = apply_sqlcipher_key(conn, path, encryption_key)?;

        configure_connection(&conn, true)?;

        let target = max_schema_version.unwrap_or(migration::CURRENT_VERSION);
        migration::run_migrations_to(&conn, target)
            .map_err(|e| StorageError::Internal(format!("migration failure: {e}")))?;

        post_migration_setup(&conn)?;

        info!("SQLite storage initialized: {}", path.display());

        Ok(Self {
            conn: Arc::new(GuardedConnection::new_unflagged(conn)),
            retention_days,
            clock: Arc::new(hlc_clock::HlcClock::new()),
            // Encrypted-at-rest DB → the same master key seeds the raw-plane
            // subkey derivation (Amendment I1). Cloned, not borrowed, because the
            // raw writer runs inside the spawn_blocking connection closure.
            work_context_raw_key: encryption_key.cloned(),
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
            conn: Arc::new(GuardedConnection::new_unflagged(conn)),
            retention_days,
            clock: Arc::new(hlc_clock::HlcClock::new()),
            work_context_raw_key: None,
        })
    }

    /// #8589: inject the work-context raw-plane master key on a storage handle
    /// opened without SQLCipher (the in-memory / plaintext-file paths).
    ///
    /// Production reaches the raw key automatically through `open(..,
    /// Some(&key))`; this consuming builder lets the composition root and tests
    /// enable consented raw-plane writes on an otherwise-unencrypted handle
    /// without dragging in the SQLCipher page cipher. Call before wrapping in
    /// `Arc`.
    pub fn with_work_context_raw_key(mut self, key: EncryptionKey) -> Self {
        self.work_context_raw_key = Some(key);
        self
    }

    /// Expose the shared [`GuardedConnection`] for shared-connection adapters
    /// (`SqliteVectorStore`/`SqliteVectorIndex`/`SqliteSyncMerger`/
    /// `SqliteSyncExtractor`/`SqliteRegimeManagerStateStore`/memory_graph).
    ///
    /// The raw `Arc<Mutex<Connection>>` accessor was removed in #4928 — no
    /// adapter can obtain a handle that bypasses the deletion barrier (by
    /// construction).
    pub fn connection_arc(&self) -> Arc<GuardedConnection> {
        self.conn.clone()
    }

    /// Expose the shared `deletion_flag` (for PHASE 2 composition-root wiring /
    /// ptr-eq verification).
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.conn.deletion_flag()
    }

    /// Install the shared `deletion_flag` (seam for PHASE 2 composition-root wiring).
    ///
    /// This swaps the `ArcSwap` cell inside `Arc<GuardedConnection>` via `&self`,
    /// so it also applies to instances already shared as `Arc<SqliteStorage>`.
    /// After install, the `write_lock` re-check of every adapter (sharing
    /// `connection_arc()`) sees the same flag — ptr-eq-linked with
    /// `ConsentManager::deletion_flag()`.
    pub fn set_deletion_flag(&self, flag: Arc<AtomicBool>) {
        self.conn.set_deletion_flag(flag);
    }

    /// #4928 round-3 (FIX B): expose the shared `erasing` signal (for wiring /
    /// ptr-eq verification).
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.conn.erasing()
    }

    /// #4928 round-3 (FIX B): install the shared `erasing` signal (seam for
    /// composition-root wiring).
    ///
    /// Works via `&self` like `set_deletion_flag`; while `erase_all_local_data`
    /// sets/clears it via RAII, every adapter's `write_lock` sees the same
    /// signal. `grant_consent` cannot touch this signal, so it blocks any
    /// re-consent race within the erase window.
    pub fn set_erasing(&self, erasing: Arc<AtomicBool>) {
        self.conn.set_erasing(erasing);
    }

    /// Funnel that isolates a synchronous SQLite **write** operation onto
    /// spawn_blocking.
    ///
    /// Because it goes through [`GuardedConnection::write_lock`], after consent
    /// revoke (deletion_flag set) the closure does not run and `Ok(T::default())`
    /// is returned. All mutating (INSERT/UPDATE/DELETE/REPLACE/CREATE) closures
    /// use this funnel. Reads use [`Self::with_conn_read`] /
    /// [`Self::read_only_query`].
    ///
    /// The parking_lot guard is acquired/released inside the spawn_blocking
    /// thread and is never held across an `.await`.
    pub(super) async fn with_conn<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Default + Send + 'static,
    {
        self.with_conn_skip(T::default(), f).await
    }

    /// Explicit skip-sentinel version of [`Self::with_conn`].
    ///
    /// Used by write operations where `T` is not `Default` (e.g. `FocusMetrics`)
    /// or that must return a specific sentinel other than `T::default()` on
    /// erase-skip. It goes through the same `write_lock` funnel
    /// (`deletion_flag || erasing` re-check), so the #4928 erase barrier is
    /// preserved.
    pub(super) async fn with_conn_skip<F, T>(&self, skipped: T, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || conn.write_lock().run(skipped, f))
            .await
            .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    /// Funnel that isolates a synchronous SQLite **write** transaction operation
    /// onto spawn_blocking.
    ///
    /// Like [`Self::with_conn`], it skips when deletion_flag is set, but passes
    /// the closure an exclusive (mutable) reference to the connection to allow
    /// mutable access such as `transaction()`.
    pub(super) async fn with_conn_mut<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StorageError> + Send + 'static,
        T: Default + Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || conn.write_lock().run_mut(T::default(), f))
            .await
            .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    /// Funnel that isolates a synchronous SQLite **read** operation onto
    /// spawn_blocking.
    ///
    /// Because it goes through [`GuardedConnection::read_lock`], it always runs
    /// regardless of deletion_flag (reads are never skipped). Pure SELECT queries
    /// use this funnel or [`Self::read_only_query`].
    pub(super) async fn with_conn_read<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || conn.read_lock().run(f))
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
            // Read funnel — always runs regardless of deletion_flag.
            conn.read_lock().run(f)
            // guard drops here, releasing the Mutex
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    // ── app_meta key-value helpers (V19) ────────────────────────────

    /// Retrieve a value from the `app_meta` table, or `None` if the key does not exist.
    ///
    /// `app_meta` is a retained table, but since this is a read, `read_lock` is sufficient.
    pub fn get_meta(&self, key: &str) -> Option<String> {
        self.conn
            .read_lock()
            .run::<_, String, rusqlite::Error>(|conn| {
                conn.query_row("SELECT value FROM app_meta WHERE key = ?1", [key], |row| {
                    row.get(0)
                })
            })
            .ok()
    }

    /// Insert or replace a value in the `app_meta` table.
    ///
    /// `app_meta` is an erase-retained table, so it uses `retained_write_lock`
    /// (ignores deletion_flag) — system-metadata writes are allowed even after
    /// revoke.
    pub fn set_meta(&self, key: &str, value: &str) {
        let _ = self
            .conn
            .retained_write_lock()
            .run::<_, usize, rusqlite::Error>(|conn| {
                conn.execute(
                    "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, value],
                )
            });
    }

    /// Delete a key from the `app_meta` table.
    pub fn delete_meta(&self, key: &str) {
        let _ = self
            .conn
            .retained_write_lock()
            .run::<_, usize, rusqlite::Error>(|conn| {
                conn.execute("DELETE FROM app_meta WHERE key = ?1", [key])
            });
    }

    /// Store a value in `app_meta`, propagating any error to the caller
    /// (R2: surface execution errors).
    ///
    /// Unlike `set_meta`, this does not swallow SQLite execution errors and
    /// returns them as `StorageError`. Used where a missing write is not
    /// acceptable, such as recording a GDPR retry marker. Uses
    /// `retained_write_lock` because this is a retained table.
    pub fn set_meta_checked(&self, key: &str, value: &str) -> Result<(), StorageError> {
        self.conn.retained_write_lock().run(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .map_err(|e| {
                StorageError::Internal(format!("app_meta set_meta_checked failed: {e}"))
            })?;
            Ok(())
        })
    }

    /// Delete a key from `app_meta`, propagating any error to the caller
    /// (R2: surface execution errors).
    ///
    /// Unlike `delete_meta`, this does not swallow SQLite execution errors and
    /// returns them as `StorageError`. Used where a missing write is not
    /// acceptable, such as clearing a GDPR retry marker. Uses
    /// `retained_write_lock` because this is a retained table.
    pub fn delete_meta_checked(&self, key: &str) -> Result<(), StorageError> {
        self.conn.retained_write_lock().run(|conn| {
            conn.execute("DELETE FROM app_meta WHERE key = ?1", [key])
                .map_err(|e| {
                    StorageError::Internal(format!("app_meta delete_meta_checked failed: {e}"))
                })?;
            Ok(())
        })
    }
}

// ── Audit log persistence (V25) ────────────────────────────

impl SqliteStorage {
    /// Persist a single audit entry to the `audit_log` table (V25) and extend the
    /// SHA-256 hash chain (V37, #4834).
    ///
    /// Designed to be called from a persistence callback wired by `src-tauri`.
    /// Failures are logged and swallowed to avoid disrupting the audit buffer.
    ///
    /// # Hash chain (#4834, ADR-072 client mirror)
    /// Within a single `retained_write_lock`, this: (1) reads the chain tip
    /// (the `MAX(seq)` row), (2) determines `next_seq`/`prev_hash`, (3) computes
    /// `entry_hash`, and (4) performs the INSERT including seq/prev_hash/
    /// entry_hash. A single write lock (mutex) serializes the tip-read and the
    /// link-write, so the chain is never torn even under concurrency.
    ///
    /// So that a duplicate `entry_id` causing `INSERT OR IGNORE` to no-op does
    /// not consume/skip a seq, the link is considered committed only when the
    /// INSERT's affected-rows count is 1 (gap-free guarantee). On a duplicate,
    /// `next_seq` is reused on the next call.
    ///
    /// `audit_log` is an erase-retained table, so it must keep using
    /// `retained_write_lock` (ignores deletion_flag) — the consent_revoked audit
    /// happens at erase time and the chain must keep extending during/after the
    /// erase (#4928).
    pub fn save_audit_entry(&self, entry: &maekon_core::models::audit::AuditEntry) {
        use crate::audit_chain::{compute_entry_hash, CanonicalRecord, GENESIS_PREV_HASH};

        let status_str = format!("{:?}", entry.status);
        let timestamp_str = entry.timestamp.to_rfc3339();
        let exec_time = entry.execution_time_ms.map(|v| v as i64);

        // The closure returns `StorageError` (not `rusqlite::Error`) so the
        // canonical-serialization overflow guard in `compute_entry_hash` can
        // propagate via `?`; `conn.execute` errors auto-convert through the
        // `#[from] rusqlite::Error` arm. Best-effort write is preserved: any
        // error (SQLite or overflow) is logged below and the entry is dropped
        // rather than persisted with a truncated/wrong hash.
        let res: Result<(), StorageError> = self.conn.retained_write_lock().run(|conn| {
            // (1) Read the chain tip — the row with the largest seq. Excludes
            //     legacy NULL-chain rows.
            let tip: Option<(i64, String)> = conn
                .query_row(
                    "SELECT seq, entry_hash FROM audit_log \
                     WHERE seq IS NOT NULL ORDER BY seq DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();

            // (2) Determine next_seq / prev_hash.
            let (next_seq, prev_hash) = match tip {
                Some((tip_seq, tip_hash)) => (tip_seq + 1, tip_hash),
                None => (0, GENESIS_PREV_HASH.to_string()),
            };

            // (3) Compute entry_hash after canonical serialization.
            let record = CanonicalRecord {
                entry_id: &entry.entry_id,
                timestamp_rfc3339: &timestamp_str,
                session_id: &entry.session_id,
                command_id: &entry.command_id,
                action_type: &entry.action_type,
                status: &status_str,
                details: entry.details.as_deref(),
                execution_time_ms: entry.execution_time_ms,
            };
            let entry_hash = compute_entry_hash(&prev_hash, &record)?;

            // (4) INSERT OR IGNORE including seq/prev_hash/entry_hash.
            let affected = conn.execute(
                "INSERT OR IGNORE INTO audit_log \
                 (entry_id, timestamp, session_id, command_id, action_type, status, \
                  details, execution_time_ms, seq, prev_hash, entry_hash) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    entry.entry_id,
                    timestamp_str,
                    entry.session_id,
                    entry.command_id,
                    entry.action_type,
                    status_str,
                    entry.details,
                    exec_time,
                    next_seq,
                    prev_hash,
                    entry_hash,
                ],
            )?;
            // affected == 0 → duplicate entry_id no-op. Since no seq was consumed
            // (the INSERT itself was ignored), the chain stays gap-free.
            let _ = affected;
            Ok(())
        });
        if let Err(e) = res {
            warn!("audit persistence: INSERT failed: {e}");
        }
    }

    /// Persist one AI conversation session audit entry to `session_audit_log`.
    ///
    /// Mirrors [`Self::save_audit_entry`]: synchronous, best-effort, infallible
    /// (logs `warn!` on SQLite error, never returns/propagates). The session
    /// audit log is a security/compliance control that must never block or fail
    /// the conversation path.
    ///
    /// `session_audit_log` is a retain-on-erase table (same class as
    /// `audit_log`), so the write goes through `retained_write_lock` — the
    /// consent-erasure barrier must not skip session-audit writes.
    ///
    /// The `category` enum serializes to its `snake_case` wire string (e.g.
    /// `ToolUse` → `"tool_use"`); `payload` is stored as compact JSON text.
    pub fn save_session_audit_entry(
        &self,
        entry: &maekon_core::models::ai_session::SessionAuditEntry,
    ) {
        let timestamp_str = entry.timestamp.to_rfc3339();
        // Match the `snake_case` serde rename used everywhere else for the
        // category column (json string without the surrounding quotes).
        let category_str = serde_json::to_value(entry.category)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("{:?}", entry.category));
        let payload_str = entry
            .payload
            .as_ref()
            .and_then(|p| serde_json::to_string(p).ok());
        let duration = entry.duration_ms.map(|v| v as i64);

        let res: Result<(), rusqlite::Error> = self.conn.retained_write_lock().run(|conn| {
            conn.execute(
                "INSERT INTO session_audit_log \
                 (timestamp, session_id, category, event_type, provider, payload, duration_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    timestamp_str,
                    entry.session_id,
                    category_str,
                    entry.event_type,
                    entry.provider,
                    payload_str,
                    duration,
                ],
            )?;
            Ok(())
        });
        if let Err(e) = res {
            warn!("session audit persistence: INSERT failed: {e}");
        }
    }
}

impl SqliteStorage {
    /// Return terminal-status aggregates from the durable audit log.
    ///
    /// `Started` rows are intentionally excluded from `total`, matching
    /// `AuditLogger::stats`. On a SQLite error, logs a warning and returns the
    /// fail-safe empty aggregate.
    pub fn audit_stats(&self) -> maekon_core::models::audit::AuditStats {
        let read = self.conn.read_lock();
        let conn = read.conn();
        let result = conn.query_row(
            "SELECT
                COALESCE(SUM(CASE WHEN status = 'Completed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'Failed' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'Denied' THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN status = 'Timeout' THEN 1 ELSE 0 END), 0)
             FROM audit_log",
            [],
            |row| {
                let completed: i64 = row.get(0)?;
                let failed: i64 = row.get(1)?;
                let denied: i64 = row.get(2)?;
                let timeout: i64 = row.get(3)?;
                Ok(maekon_core::models::audit::AuditStats {
                    total: (completed + failed + denied + timeout) as usize,
                    completed: completed as usize,
                    failed: failed as usize,
                    denied: denied as usize,
                    timeout: timeout as usize,
                })
            },
        );

        match result {
            Ok(stats) => stats,
            Err(error) => {
                warn!(err = %error, "audit: audit_stats query failed");
                maekon_core::models::audit::AuditStats::default()
            }
        }
    }

    /// Return audit entries whose `command_id` equals the given value, ordered
    /// newest-first, up to `limit` rows.
    ///
    /// Synchronous, matching the existing [`Self::save_audit_entry`] pattern.
    /// Async callers wrap at the Adapter layer. Infallible — logs `warn!` on
    /// SQLite error and returns an empty `Vec`.
    ///
    /// Not an `impl AuditLogPort` — `SqliteStorage` does not implement the
    /// port trait directly. The `AuditLogAdapter` (in `maekon-automation`)
    /// holds `Arc<RwLock<AuditLogger>>` and may delegate here as a fall-through
    /// in a future task.
    pub fn entries_by_command_id(
        &self,
        command_id: &str,
        limit: usize,
    ) -> Vec<maekon_core::models::audit::AuditEntry> {
        let read = self.conn.read_lock();
        let conn = read.conn();

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

        let mapped = stmt.query_map(rusqlite::params![command_id, limit as i64], map_audit_row);

        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                warn!(err = %e, "audit: entries_by_command_id query_map failed");
                Vec::new()
            }
        }
    }

    /// Return the most recent audit entries ordered by `timestamp` descending,
    /// up to `limit` rows.
    ///
    /// Used by the OSS build's local audit-log export (#4819, regulatory
    /// compliance evidence). Sources from the durable SQLite `audit_log` table
    /// rather than the volatile `AuditLogger` buffer (~1000-entry cap). Reuses
    /// the same row→`AuditEntry` mapping ([`map_audit_row`]) as
    /// [`Self::entries_by_command_id`].
    ///
    /// Synchronous; on a SQLite error it only logs `warn!` and returns an empty
    /// `Vec` (infallible).
    pub fn recent_audit_entries(
        &self,
        limit: usize,
    ) -> Vec<maekon_core::models::audit::AuditEntry> {
        let read = self.conn.read_lock();
        let conn = read.conn();

        let mut stmt = match conn.prepare(
            "SELECT entry_id, timestamp, session_id, command_id, action_type,
                    status, details, execution_time_ms
             FROM audit_log
             ORDER BY timestamp DESC
             LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!(err = %e, "audit: recent_audit_entries prepare failed");
                return Vec::new();
            }
        };

        let mapped = stmt.query_map(rusqlite::params![limit as i64], map_audit_row);

        match mapped {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                warn!(err = %e, "audit: recent_audit_entries query_map failed");
                Vec::new()
            }
        }
    }
}

// ── Audit log hash-chain verification (V37, #4834/E20) ────────────────────────────

impl SqliteStorage {
    /// Verify the integrity of the `audit_log` SHA-256 hash chain (#4834,
    /// ADR-072 mirror).
    ///
    /// Traverses the chained rows (`seq IS NOT NULL`) in ascending `seq` order,
    /// checking:
    /// 1. that seq is contiguous (gap-free),
    /// 2. that `row[i].prev_hash == row[i-1].entry_hash` (link integrity),
    /// 3. that `SHA256(prev_hash || canonical(row)) == entry_hash` (row-tamper
    ///    detection),
    /// 4. that the first chained row's `prev_hash == GENESIS_PREV_HASH`.
    ///
    /// Legacy NULL-chain rows are excluded from verification and only counted.
    ///
    /// Fills `first_break` on the first violation and stops traversal.
    /// Synchronous; on a SQLite error it only logs `warn!` and returns an
    /// `ok=false` report (infallible).
    ///
    /// A SHA-256-only chain is tamper-**evident** (detects accidental/partial
    /// corruption, simple edits, deletions, and reordering) but not
    /// tamper-**proof** — a full-rewrite insider threat requires HMAC/Ed25519
    /// (out of scope, `audit_chain::HASH_VERSION` seam).
    pub fn verify_audit_chain(&self) -> maekon_core::models::audit::AuditChainReport {
        Self::verify_audit_chain_impl(&self.conn)
    }

    /// Async, non-blocking variant of [`Self::verify_audit_chain`] (#7600):
    /// offloads onto `spawn_blocking` via a cheap `Arc<GuardedConnection>`
    /// clone (mirrors the `with_conn_read` funnel pattern), so callers running
    /// on the async runtime (the `GET /audit/verify` HTTP handler,
    /// `AuditChainVerifierPort`) never block the reactor. The sync
    /// `verify_audit_chain` is kept unchanged for the existing desktop IPC
    /// command (which already offloads via its own
    /// `tokio::task::spawn_blocking` at the call site) and direct synchronous
    /// test callers.
    pub async fn verify_audit_chain_async(&self) -> maekon_core::models::audit::AuditChainReport {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || Self::verify_audit_chain_impl(&conn))
            .await
            .unwrap_or_else(|e| {
                warn!(err = %e, "audit: verify_audit_chain_async spawn_blocking join failed");
                maekon_core::models::audit::AuditChainReport {
                    ok: false,
                    first_break: Some(maekon_core::models::audit::AuditChainBreak {
                        seq: -1,
                        reason: format!("verify task join failed: {e}"),
                    }),
                    ..Default::default()
                }
            })
    }

    fn verify_audit_chain_impl(
        guarded: &GuardedConnection,
    ) -> maekon_core::models::audit::AuditChainReport {
        use crate::audit_chain::{compute_entry_hash, CanonicalRecord, GENESIS_PREV_HASH};
        use maekon_core::models::audit::{AuditChainBreak, AuditChainReport};

        let read = guarded.read_lock();
        let conn = read.conn();

        // Count of legacy (NULL-chain) rows.
        let legacy_unchained_count: u64 = conn
            .query_row(
                "SELECT COUNT(*) FROM audit_log WHERE seq IS NULL",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as u64)
            .unwrap_or(0);

        let mut stmt = match conn.prepare(
            "SELECT entry_id, timestamp, session_id, command_id, action_type, \
                    status, details, execution_time_ms, seq, prev_hash, entry_hash \
             FROM audit_log WHERE seq IS NOT NULL ORDER BY seq ASC",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!(err = %e, "audit: verify_audit_chain prepare failed");
                return AuditChainReport {
                    ok: false,
                    legacy_unchained_count,
                    first_break: Some(AuditChainBreak {
                        seq: -1,
                        reason: format!("prepare failed: {e}"),
                    }),
                    ..Default::default()
                };
            }
        };

        // Collect rows in one batch to simplify the verification logic
        // (audit_log is bounded).
        struct ChainRow {
            entry_id: String,
            timestamp: String,
            session_id: String,
            command_id: String,
            action_type: String,
            status: String,
            details: Option<String>,
            execution_time_ms: Option<i64>,
            seq: i64,
            prev_hash: String,
            entry_hash: String,
        }

        let rows_iter = stmt.query_map([], |row| {
            Ok(ChainRow {
                entry_id: row.get("entry_id")?,
                timestamp: row.get("timestamp")?,
                session_id: row.get("session_id")?,
                command_id: row.get("command_id")?,
                action_type: row.get("action_type")?,
                status: row.get("status")?,
                // #6419: read with `?` (like the other columns) so a column-type read
                // error propagates instead of silently becoming None. A verifier that
                // hashes None where the write-path hashed the real stored value produces
                // a false "row tampered" report. A genuine SQL NULL still maps to None
                // via FromSql for Option<T>; only an unreadable/type-mismatched cell errors.
                details: row.get("details")?,
                execution_time_ms: row.get("execution_time_ms")?,
                seq: row.get("seq")?,
                prev_hash: row.get("prev_hash")?,
                entry_hash: row.get("entry_hash")?,
            })
        });

        let rows: Vec<ChainRow> = match rows_iter {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                warn!(err = %e, "audit: verify_audit_chain query_map failed");
                return AuditChainReport {
                    ok: false,
                    legacy_unchained_count,
                    first_break: Some(AuditChainBreak {
                        seq: -1,
                        reason: format!("query failed: {e}"),
                    }),
                    ..Default::default()
                };
            }
        };

        if rows.is_empty() {
            // No chained rows means an intact empty chain.
            return AuditChainReport {
                ok: true,
                first_seq: None,
                last_seq: None,
                verified_count: 0,
                legacy_unchained_count,
                first_break: None,
            };
        }

        let first_seq = rows[0].seq;
        let last_seq = rows[rows.len() - 1].seq;
        let mut verified_count: u64 = 0;
        let mut expected_seq = first_seq;
        // #8056 P3: the compliance-window retention prune may have removed the
        // oldest chain prefix, recording the pruned prefix's final `entry_hash`
        // as the new chain root anchor. When present, the retained chain's first
        // row links to that anchor instead of GENESIS. Accept it so a pruned-but-
        // otherwise-intact chain still verifies (ADR-072 tamper-evidence across
        // retention). Absent anchor → the chain still starts at GENESIS.
        let mut expected_prev = conn
            .query_row(
                "SELECT value FROM app_meta WHERE key = ?1",
                [crate::audit_chain::AUDIT_CHAIN_PRUNED_ROOT_META_KEY],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_else(|_| GENESIS_PREV_HASH.to_string());

        for r in &rows {
            // 1) seq contiguity (gap detection).
            if r.seq != expected_seq {
                return AuditChainReport {
                    ok: false,
                    first_seq: Some(first_seq),
                    last_seq: Some(last_seq),
                    verified_count,
                    legacy_unchained_count,
                    first_break: Some(AuditChainBreak {
                        seq: r.seq,
                        reason: format!("seq gap: expected {expected_seq}, found {}", r.seq),
                    }),
                };
            }

            // 2) Link integrity: prev_hash == the prior row's entry_hash (the
            //    first row uses genesis).
            if r.prev_hash != expected_prev {
                let reason = if r.seq == first_seq {
                    "first chained row prev_hash != genesis/pruned-root".to_string()
                } else {
                    "prev_hash != prior entry_hash (broken link)".to_string()
                };
                return AuditChainReport {
                    ok: false,
                    first_seq: Some(first_seq),
                    last_seq: Some(last_seq),
                    verified_count,
                    legacy_unchained_count,
                    first_break: Some(AuditChainBreak { seq: r.seq, reason }),
                };
            }

            // 3) Row-tamper detection: recomputed hash == stored entry_hash.
            let record = CanonicalRecord {
                entry_id: &r.entry_id,
                timestamp_rfc3339: &r.timestamp,
                session_id: &r.session_id,
                command_id: &r.command_id,
                action_type: &r.action_type,
                status: &r.status,
                details: r.details.as_deref(),
                execution_time_ms: r.execution_time_ms.map(|v| v as u64),
            };
            // Re-derive the hash. A canonical-serialization overflow (a stored
            // field exceeding the u32 length prefix) is itself a chain break:
            // the row could never have been written by `save_audit_entry`, so
            // treat it as tampering rather than panicking on this infallible path.
            let recomputed = match compute_entry_hash(&r.prev_hash, &record) {
                Ok(h) => h,
                Err(e) => {
                    warn!(seq = r.seq, err = %e, "audit: verify_audit_chain canonicalization failed");
                    return AuditChainReport {
                        ok: false,
                        first_seq: Some(first_seq),
                        last_seq: Some(last_seq),
                        verified_count,
                        legacy_unchained_count,
                        first_break: Some(AuditChainBreak {
                            seq: r.seq,
                            reason: format!("canonicalization failed: {e}"),
                        }),
                    };
                }
            };
            if recomputed != r.entry_hash {
                return AuditChainReport {
                    ok: false,
                    first_seq: Some(first_seq),
                    last_seq: Some(last_seq),
                    verified_count,
                    legacy_unchained_count,
                    first_break: Some(AuditChainBreak {
                        seq: r.seq,
                        reason: "entry_hash mismatch (row tampered)".to_string(),
                    }),
                };
            }

            verified_count += 1;
            expected_seq = r.seq + 1;
            expected_prev = r.entry_hash.clone();
        }

        AuditChainReport {
            ok: true,
            first_seq: Some(first_seq),
            last_seq: Some(last_seq),
            verified_count,
            legacy_unchained_count,
            first_break: None,
        }
    }
}

/// #7600: port impl so `AppState.core.audit_chain_verifier` (maekon-web) can
/// reach the same real hash-chain verification the desktop `verify_audit_log`
/// IPC command uses, without maekon-web depending on the concrete
/// `SqliteStorage` type (Hexagonal boundary — maekon-web is an adapter and
/// must depend on `maekon-core` ports, not sibling adapter crates).
#[async_trait::async_trait]
impl maekon_core::ports::audit_chain_verifier::AuditChainVerifierPort for SqliteStorage {
    async fn verify_audit_chain(&self) -> maekon_core::models::audit::AuditChainReport {
        self.verify_audit_chain_async().await
    }
}

// ── Egress audit-ledger persistence (V36, #4803/E20) ────────────────────────────

impl SqliteStorage {
    /// Record one egress event in the `egress_ledger` table (V36).
    ///
    /// Retains events that left the device (`uploaded`) or were blocked by
    /// policy (`blocked`) as regulatory-compliance evidence. The `record_id`
    /// UNIQUE constraint dedupes `INSERT OR IGNORE` re-execution. On a SQLite
    /// error it does not return `Ok(())` after only logging `warn!`; instead it
    /// propagates a `StorageError` so the caller can observe the failure.
    pub fn record_egress(
        &self,
        record: &maekon_core::models::storage_records::EgressLedgerRecord,
    ) -> Result<(), StorageError> {
        // `egress_ledger` is an erase-retained table — `retained_write_lock`
        // (ignores deletion_flag).
        self.conn.retained_write_lock().run(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO egress_ledger \
                 (record_id, event_type, event_id, byte_count, recipient_count, destination, disposition, consent_state, occurred_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    record.record_id,
                    record.event_type,
                    record.event_id,
                    record.byte_count,
                    record.recipient_count,
                    record.destination,
                    record.disposition,
                    record.consent_state,
                    record.occurred_at,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("egress_ledger INSERT failed: {e}")))?;
            Ok(())
        })
    }

    /// Return the most recent egress-ledger entries ordered by `occurred_at`
    /// descending, up to `limit` rows.
    ///
    /// Synchronous; on a SQLite error it only logs `warn!` and returns an empty
    /// `Vec` (infallible).
    pub fn recent_egress(
        &self,
        limit: usize,
    ) -> Vec<maekon_core::models::storage_records::EgressLedgerRecord> {
        let read = self.conn.read_lock();
        let conn = read.conn();

        let mut stmt = match conn.prepare(
            "SELECT record_id, event_type, event_id, byte_count, recipient_count, destination,
                    disposition, consent_state, occurred_at
             FROM egress_ledger
             ORDER BY occurred_at DESC
             LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!(err = %e, "egress: recent_egress prepare failed");
                return Vec::new();
            }
        };

        let result = match stmt.query_map(rusqlite::params![limit as i64], map_egress_row) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                warn!(err = %e, "egress: recent_egress query_map failed");
                Vec::new()
            }
        };
        result
    }

    /// Return egress-ledger entries in the `[from, to]` range (inclusive RFC3339
    /// strings) ordered by `occurred_at` ascending. Used for regulatory-
    /// compliance evidence export.
    ///
    /// Synchronous; on a SQLite error it only logs `warn!` and returns an empty
    /// `Vec` (infallible).
    pub fn egress_between(
        &self,
        from: &str,
        to: &str,
    ) -> Vec<maekon_core::models::storage_records::EgressLedgerRecord> {
        let read = self.conn.read_lock();
        let conn = read.conn();

        let mut stmt = match conn.prepare(
            "SELECT record_id, event_type, event_id, byte_count, recipient_count, destination,
                    disposition, consent_state, occurred_at
             FROM egress_ledger
             WHERE occurred_at >= ?1 AND occurred_at <= ?2
             ORDER BY occurred_at ASC",
        ) {
            Ok(s) => s,
            Err(e) => {
                warn!(err = %e, "egress: egress_between prepare failed");
                return Vec::new();
            }
        };

        let result = match stmt.query_map(rusqlite::params![from, to], map_egress_row) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                warn!(err = %e, "egress: egress_between query_map failed");
                Vec::new()
            }
        };
        result
    }
}

/// #4803/#5143: object-safe egress-ledger sink. Delegates to the inherent
/// `record_egress` (erase-retained write), letting `SyncEngine` and other
/// callers record egress through `Arc<dyn EgressLedgerSink>` without depending
/// on the concrete `SqliteStorage`.
impl maekon_core::ports::egress_ledger::EgressLedgerSink for SqliteStorage {
    fn record_egress(
        &self,
        record: &maekon_core::models::storage_records::EgressLedgerRecord,
    ) -> Result<(), maekon_core::error::CoreError> {
        SqliteStorage::record_egress(self, record).map_err(Into::into)
    }
}

/// #7910: object-safe read-only view over the egress audit ledger. Delegates to
/// the infallible inherent `recent_egress` / `egress_between` readers, letting
/// the local dashboard render an egress transparency browser through
/// `Arc<dyn EgressLedgerReaderPort>` without depending on the concrete
/// `SqliteStorage`. Read-only by construction — the ledger is erase-retained
/// compliance evidence and exposes no mutation surface.
impl maekon_core::ports::egress_ledger_reader::EgressLedgerReaderPort for SqliteStorage {
    fn recent_egress(
        &self,
        limit: usize,
    ) -> Vec<maekon_core::models::storage_records::EgressLedgerRecord> {
        SqliteStorage::recent_egress(self, limit)
    }

    fn egress_between(
        &self,
        from: &str,
        to: &str,
    ) -> Vec<maekon_core::models::storage_records::EgressLedgerRecord> {
        SqliteStorage::egress_between(self, from, to)
    }
}

/// `app_meta` key for the last propagated Art. 17 erasure id (#5156).
const LAST_PUSHED_ERASURE_ID_KEY: &str = "sync.last_pushed_erasure_id";

/// #5156: object-safe store for the last propagated erasure id, backed by the
/// erase-retained `app_meta` table. Lets `SyncEngine` persist its fire-once gate
/// state through `Arc<dyn ErasurePropagationStore>` without depending on the
/// concrete `SqliteStorage`.
impl maekon_core::ports::erasure_propagation_store::ErasurePropagationStore for SqliteStorage {
    fn last_pushed_erasure_id(&self) -> Option<String> {
        self.get_meta(LAST_PUSHED_ERASURE_ID_KEY)
    }

    fn record_pushed_erasure_id(&self, id: &str) -> Result<(), maekon_core::error::CoreError> {
        self.set_meta_checked(LAST_PUSHED_ERASURE_ID_KEY, id)
            .map_err(Into::into)
    }
}

/// Map one `egress_ledger` row to
/// [`maekon_core::models::storage_records::EgressLedgerRecord`]. The column order
/// must match the SELECT clause.
fn map_egress_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<maekon_core::models::storage_records::EgressLedgerRecord> {
    use maekon_core::models::storage_records::EgressLedgerRecord;
    Ok(EgressLedgerRecord {
        record_id: row.get("record_id")?,
        event_type: row.get("event_type")?,
        event_id: row.get("event_id").ok(),
        byte_count: row.get("byte_count")?,
        recipient_count: row.get("recipient_count")?,
        destination: row.get("destination")?,
        disposition: row.get("disposition")?,
        consent_state: row.get("consent_state")?,
        occurred_at: row.get("occurred_at")?,
    })
}

/// Map one `audit_log` row to [`maekon_core::models::audit::AuditEntry`].
///
/// The single mapping logic shared by `entries_by_command_id` /
/// `recent_audit_entries`. The column order must match `SELECT entry_id,
/// timestamp, session_id, command_id, action_type, status, details,
/// execution_time_ms`.
fn map_audit_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<maekon_core::models::audit::AuditEntry> {
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

    let ts_str: String = row.get("timestamp")?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(&ts_str)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
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
}

/// Apply SQLCipher `PRAGMA key` and verify the key works.
///
/// On verification failure, do not allow reopening as plaintext.
///
/// When encryption was explicitly requested, opening a plaintext SQLite would let
/// subsequent writes proceed in plaintext, so legacy plaintext files are also
/// handled fail-closed (#6438).
fn apply_sqlcipher_key(
    conn: Connection,
    path: &Path,
    encryption_key: Option<&EncryptionKey>,
) -> Result<Connection, StorageError> {
    let Some(key) = encryption_key else {
        return Ok(conn);
    };

    // PRAGMA key must be the first statement after opening. The PRAGMA text
    // embeds the master key in hex, so wrap it in `Zeroizing` to wipe that
    // plaintext copy from the heap when it drops (#6242). `as_hex()` already
    // returns a `Zeroizing<String>`.
    let pragma = zeroize::Zeroizing::new(format!("PRAGMA key = \"x'{}'\";", key.as_hex().as_str()));
    if let Err(e) = conn.execute_batch(&pragma) {
        // #6418: applying the key PRAGMA itself failed (SQLCipher unavailable,
        // malformed key, or I/O) — this is NOT the documented unencrypted-DB
        // migration case, which surfaces below as a key-VERIFICATION failure.
        // Encryption was explicitly requested but could not be applied, so fail
        // CLOSED rather than silently reopening the database as plaintext.
        drop(conn);
        return Err(StorageError::Internal(format!(
            "SQLCipher key could not be applied though encryption was requested: {e}"
        )));
    }

    // Verify the key actually works by reading sqlite_master.
    match conn.execute_batch("SELECT count(*) FROM sqlite_master;") {
        Ok(()) => Ok(conn),
        Err(verify_err) => {
            drop(conn);
            if is_plaintext_sqlite(path) {
                Err(StorageError::Internal(format!(
                    "SQLCipher key verification failed and {} is a plaintext SQLite database; \
                     refusing to reopen as plaintext while encryption was requested: {verify_err}",
                    path.display()
                )))
            } else {
                Err(StorageError::Internal(format!(
                    "SQLCipher key verification failed and {} is not a plaintext SQLite \
                     database (wrong or rotated key); refusing to reopen as plaintext: {verify_err}",
                    path.display()
                )))
            }
        }
    }
}

/// #6438: Detect a plaintext SQLite via the 16-byte file header magic
/// (`"SQLite format 3\0"`). A SQLCipher DB has an encrypted header, so it does
/// not start with this magic.
fn is_plaintext_sqlite(path: &Path) -> bool {
    use std::io::Read;
    const MAGIC: &[u8; 16] = b"SQLite format 3\0";
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut header = [0u8; 16];
    match file.read_exact(&mut header) {
        Ok(()) => &header == MAGIC,
        Err(_) => false, // empty / shorter than the header → not a legacy plaintext DB
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

    // Seed the persistent HLC clock (V39) BEFORE the SqliteStorage handle is returned, so
    // no local write can read an un-seeded clock and stamp below a retained tombstone
    // (F0/#5186; the erasure epic's P1 premise). Idempotent: raises the floor, never lowers.
    hlc_clock::seed_from_db(conn)
        .map_err(|e| StorageError::Internal(format!("hlc_clock seed failed: {e}")))?;

    Ok(())
}

// Record types are canonical in maekon-core; re-exported here for backward compatibility.
pub use maekon_core::models::storage_records::{
    DeletedRangeCounts, EgressLedgerRecord, EventExportRecord, FocusInterruptionRecord,
    FocusWorkSessionRecord, FrameExportRecord, FrameRecord, FrameTagLinkRecord,
    HourlyMetricsRecord, LocalSuggestionRecord, MetricExportRecord, SearchEventRow, SearchFrameRow,
    StorageStatsSummaryRecord, TagRecord,
};
