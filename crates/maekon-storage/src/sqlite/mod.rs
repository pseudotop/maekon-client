mod annotation_storage_impl;
mod calibration_store_impl;
pub(crate) mod cjk_shadow;
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
pub mod guarded_connection;
mod habit_storage;
pub(crate) mod hlc_clock;
mod integration_query_impl;
mod lan_pin_store;
mod maintenance;
mod memory_graph_impl;
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
    /// consent-erasure 차단 chokepoint (#4928). raw `Mutex<Connection>` 접근자는
    /// 제거되었고, 모든 SQLite 접근은 [`GuardedConnection`] funnel 을 통과한다.
    pub(super) conn: Arc<GuardedConnection>,
    pub(super) retention_days: u32,
    /// Persistent monotonic HLC clock for stamping local synced-table writes so they
    /// propagate via cross-device sync (F0/#5186). Its own instance is fine — the durable
    /// floor lives in the `hlc_clock` table reached through the same `GuardedConnection`
    /// mutex (shared with `SqliteVectorStore`'s clock), so RMW stays serialized + monotonic.
    pub(super) clock: Arc<hlc_clock::HlcClock>,
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
            conn: Arc::new(GuardedConnection::new_unflagged(conn)),
            retention_days,
            clock: Arc::new(hlc_clock::HlcClock::new()),
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
        })
    }

    /// Expose the shared [`GuardedConnection`] for shared-connection adapters
    /// (`SqliteVectorStore`/`SqliteVectorIndex`/`SqliteSyncMerger`/
    /// `SqliteSyncExtractor`/`SqliteRegimeManagerStateStore`/memory_graph).
    ///
    /// raw `Arc<Mutex<Connection>>` 접근자는 #4928 로 제거되었다 — 어떤 어댑터도
    /// deletion-barrier 를 우회한 핸들을 얻을 수 없다(by construction).
    pub fn connection_arc(&self) -> Arc<GuardedConnection> {
        self.conn.clone()
    }

    /// 공유 `deletion_flag` 를 노출한다(PHASE 2 composition-root 배선/ptr-eq 검증용).
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.conn.deletion_flag()
    }

    /// 공유 `deletion_flag` 를 install 한다(PHASE 2 composition-root 배선용 seam).
    ///
    /// `Arc<GuardedConnection>` 내부의 `ArcSwap` 셀을 `&self` 로 교체하므로,
    /// 이미 `Arc<SqliteStorage>` 로 공유된 인스턴스에도 적용된다. install 이후
    /// 모든 어댑터(`connection_arc()` 공유)의 `write_lock` 재검사가 동일 flag 를
    /// 본다 — `ConsentManager::deletion_flag()` 와 ptr-eq 로 연결된다.
    pub fn set_deletion_flag(&self, flag: Arc<AtomicBool>) {
        self.conn.set_deletion_flag(flag);
    }

    /// #4928 round-3 (FIX B): 공유 `erasing` 신호를 노출한다(배선/ptr-eq 검증용).
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.conn.erasing()
    }

    /// #4928 round-3 (FIX B): 공유 `erasing` 신호를 install 한다(composition-root 배선용 seam).
    ///
    /// `set_deletion_flag` 와 동일하게 `&self` 로 동작하며, `erase_all_local_data` 가
    /// RAII 로 set/clear 하는 동안 모든 어댑터의 `write_lock` 이 동일 신호를 본다.
    /// `grant_consent` 는 이 신호를 건드리지 못하므로 erase 윈도우 안의 재동의 race 를
    /// 차단한다.
    pub fn set_erasing(&self, erasing: Arc<AtomicBool>) {
        self.conn.set_erasing(erasing);
    }

    /// 동기 SQLite **쓰기** 연산을 spawn_blocking으로 격리하는 funnel.
    ///
    /// [`GuardedConnection::write_lock`] 을 통과하므로 consent revoke 후에는
    /// (deletion_flag set) 클로저가 실행되지 않고 `Ok(T::default())` 를 반환한다.
    /// 모든 변경(INSERT/UPDATE/DELETE/REPLACE/CREATE) 클로저는 이 funnel 을 사용한다.
    /// 읽기는 [`Self::with_conn_read`] / [`Self::read_only_query`] 를 사용한다.
    ///
    /// parking_lot 가드는 spawn_blocking 스레드 안에서 획득/해제되며 `.await` 를
    /// 가로질러 보유되지 않는다.
    pub(super) async fn with_conn<F, T>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&Connection) -> Result<T, StorageError> + Send + 'static,
        T: Default + Send + 'static,
    {
        self.with_conn_skip(T::default(), f).await
    }

    /// [`Self::with_conn`] 의 명시적 skip-sentinel 버전.
    ///
    /// `T: Default` 가 아니거나(예: `FocusMetrics`) erase-skip 시 `T::default()`
    /// 와 다른 특정 sentinel 을 반환해야 하는 쓰기 연산에서 사용한다. 동일한
    /// `write_lock` funnel(`deletion_flag || erasing` 재검사)을 통과하므로 #4928
    /// erase 배리어는 그대로 유지된다.
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

    /// 동기 SQLite **쓰기** 트랜잭션 연산을 spawn_blocking으로 격리하는 funnel.
    ///
    /// [`Self::with_conn`] 과 동일하게 deletion_flag set 시 스킵하나, 클로저에
    /// 커넥션의 배타적(가변) 참조를 넘겨 `transaction()` 등 가변 접근을 허용한다.
    #[allow(dead_code)]
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

    /// 동기 SQLite **읽기** 연산을 spawn_blocking으로 격리하는 funnel.
    ///
    /// [`GuardedConnection::read_lock`] 을 통과하므로 deletion_flag 와 무관하게 항상
    /// 실행된다(읽기는 절대 스킵하지 않음). 순수 SELECT 쿼리는 이 funnel 또는
    /// [`Self::read_only_query`] 를 사용한다.
    #[allow(dead_code)]
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
            // 읽기 funnel — deletion_flag 와 무관하게 항상 실행한다.
            conn.read_lock().run(f)
            // guard drops here, releasing the Mutex
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    // ── app_meta key-value helpers (V19) ────────────────────────────

    /// Retrieve a value from the `app_meta` table, or `None` if the key does not exist.
    ///
    /// `app_meta` 는 보존(retained) 테이블이지만 읽기이므로 read_lock 으로 충분하다.
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
    /// `app_meta` 는 erase 보존 테이블이므로 `retained_write_lock`(deletion_flag 무시)
    /// 을 사용한다 — revoke 후에도 시스템 메타데이터 기록은 허용된다.
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

    /// `app_meta`에 값을 저장하며 오류를 호출자에게 전파한다 (R2: 실행 오류 표면화).
    ///
    /// `set_meta`와 달리 SQLite 실행 오류를 삼키지 않고 `StorageError`로 반환한다.
    /// GDPR 재시도 마커 기록처럼 누락이 허용되지 않는 경우에 사용한다.
    /// 보존 테이블이므로 `retained_write_lock` 을 사용한다.
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

    /// `app_meta`에서 키를 삭제하며 오류를 호출자에게 전파한다 (R2: 실행 오류 표면화).
    ///
    /// `delete_meta`와 달리 SQLite 실행 오류를 삼키지 않고 `StorageError`로 반환한다.
    /// GDPR 재시도 마커 해제처럼 누락이 허용되지 않는 경우에 사용한다.
    /// 보존 테이블이므로 `retained_write_lock` 을 사용한다.
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
    /// # 해시 체인 (#4834, ADR-072 client mirror)
    /// 동일한 `retained_write_lock` 안에서 (1) 체인 tip(`MAX(seq)` 행) read,
    /// (2) `next_seq`/`prev_hash` 결정, (3) `entry_hash` 계산, (4) seq/prev_hash/
    /// entry_hash 를 포함해 INSERT 를 수행한다. 단일 write lock(mutex)이 tip-read
    /// 와 link-write 를 직렬화하므로 동시성에서도 체인이 찢기지 않는다.
    ///
    /// `INSERT OR IGNORE` 가 중복 `entry_id` 로 no-op 이 되어도 seq 가 소모/누락되지
    /// 않도록, INSERT 의 affected-rows 가 1 일 때만 링크가 commit 된 것으로 본다
    /// (gap-free 보장). 중복이면 next_seq 는 다음 호출에서 재사용된다.
    ///
    /// `audit_log` 는 erase 보존 테이블이므로 `retained_write_lock`(deletion_flag
    /// 무시)을 유지해야 한다 — consent_revoked 감사가 erase 시점에 일어나며 체인은
    /// erase 중/후에도 계속 연장되어야 한다(#4928).
    pub fn save_audit_entry(&self, entry: &maekon_core::models::audit::AuditEntry) {
        use crate::audit_chain::{compute_entry_hash, CanonicalRecord, GENESIS_PREV_HASH};

        let status_str = format!("{:?}", entry.status);
        let timestamp_str = entry.timestamp.to_rfc3339();
        let exec_time = entry.execution_time_ms.map(|v| v as i64);

        let res: Result<(), rusqlite::Error> = self.conn.retained_write_lock().run(|conn| {
            // (1) 체인 tip read — seq 가 가장 큰 행. legacy NULL-chain 행은 제외.
            let tip: Option<(i64, String)> = conn
                .query_row(
                    "SELECT seq, entry_hash FROM audit_log \
                     WHERE seq IS NOT NULL ORDER BY seq DESC LIMIT 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .ok();

            // (2) next_seq / prev_hash 결정.
            let (next_seq, prev_hash) = match tip {
                Some((tip_seq, tip_hash)) => (tip_seq + 1, tip_hash),
                None => (0, GENESIS_PREV_HASH.to_string()),
            };

            // (3) canonical 직렬화 후 entry_hash 계산.
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
            let entry_hash = compute_entry_hash(&prev_hash, &record);

            // (4) seq/prev_hash/entry_hash 포함 INSERT OR IGNORE.
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
            // affected == 0 → 중복 entry_id no-op. seq 를 소모하지 않았으므로(INSERT
            // 자체가 무시됨) 체인은 gap-free 로 유지된다.
            let _ = affected;
            Ok(())
        });
        if let Err(e) = res {
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

    /// 가장 최근 감사 항목을 `timestamp` 내림차순으로 최대 `limit`개 반환한다.
    ///
    /// OSS 빌드의 로컬 감사 로그 export(#4819, 규제 준수 증거)에서 사용한다.
    /// 휘발성 `AuditLogger` 버퍼(~1000개 cap)가 아닌 durable SQLite `audit_log`
    /// 테이블을 source로 삼는다. [`Self::entries_by_command_id`]와 동일한
    /// row→`AuditEntry` 매핑([`map_audit_row`])을 재사용한다.
    ///
    /// 동기 메서드이며, SQLite 오류 시 `warn!`만 남기고 빈 `Vec`를 반환한다(infallible).
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

// ── Audit log 해시 체인 검증 (V37, #4834/E20) ────────────────────────────

impl SqliteStorage {
    /// `audit_log` SHA-256 해시 체인의 무결성을 검증한다 (#4834, ADR-072 mirror).
    ///
    /// 체인 편입 행(`seq IS NOT NULL`)을 `seq` 오름차순으로 순회하며:
    /// 1. seq 가 연속(gap-free)인지,
    /// 2. `row[i].prev_hash == row[i-1].entry_hash` 인지(링크 무결성),
    /// 3. `SHA256(prev_hash || canonical(row)) == entry_hash` 인지(행 변조 탐지),
    /// 4. 첫 체인 행의 `prev_hash == GENESIS_PREV_HASH` 인지
    ///
    /// 를 확인한다. legacy NULL-chain 행은 검증 대상에서 제외하고 개수만 센다.
    ///
    /// 최초 위반에서 `first_break` 를 채우고 순회를 멈춘다. 동기 메서드이며
    /// SQLite 오류 시 `warn!` 만 남기고 `ok=false` 리포트를 반환한다(infallible).
    ///
    /// SHA-256-only 체인은 tamper-**evident**(우발적/부분적 손상·단순 편집·삭제·
    /// 재정렬 탐지)이지 tamper-**proof**가 아니다 — 전면 재기록 내부자 위협은
    /// HMAC/Ed25519(out-of-scope, `audit_chain::HASH_VERSION` seam)가 필요하다.
    pub fn verify_audit_chain(&self) -> maekon_core::models::audit::AuditChainReport {
        use crate::audit_chain::{compute_entry_hash, CanonicalRecord, GENESIS_PREV_HASH};
        use maekon_core::models::audit::{AuditChainBreak, AuditChainReport};

        let read = self.conn.read_lock();
        let conn = read.conn();

        // legacy(NULL chain) 행 개수.
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

        // 행을 일괄 수집해 검증 로직을 단순화한다(audit_log 는 bounded).
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
                details: row.get("details").ok(),
                execution_time_ms: row.get("execution_time_ms").ok(),
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
            // 체인 편입 행이 없으면 무결한 빈 체인으로 본다.
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
        let mut expected_prev = GENESIS_PREV_HASH.to_string();

        for r in &rows {
            // 1) seq 연속성(gap 탐지).
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

            // 2) 링크 무결성: prev_hash == 직전 entry_hash (첫 행은 genesis).
            if r.prev_hash != expected_prev {
                let reason = if r.seq == first_seq {
                    "first chained row prev_hash != genesis".to_string()
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

            // 3) 행 변조 탐지: 재계산 해시 == 저장된 entry_hash.
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
            let recomputed = compute_entry_hash(&r.prev_hash, &record);
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

// ── Egress 감사 원장 persistence (V36, #4803/E20) ────────────────────────────

impl SqliteStorage {
    /// egress 한 건을 `egress_ledger` 테이블에 기록한다 (V36).
    ///
    /// 디바이스를 떠난(`uploaded`) 또는 정책상 차단된(`blocked`) 이벤트를
    /// 규제 준수 증거로 남긴다. `record_id` UNIQUE 제약으로 `INSERT OR IGNORE`
    /// 재실행 중복을 제거한다. SQLite 오류 시 `warn!` 만 남기고 `Ok(())` 를
    /// 반환하지 않고 `StorageError` 로 전파하여 호출자가 실패를 관측할 수 있게 한다.
    pub fn record_egress(
        &self,
        record: &maekon_core::models::storage_records::EgressLedgerRecord,
    ) -> Result<(), StorageError> {
        // `egress_ledger` 는 erase 보존 테이블 — `retained_write_lock`(deletion_flag 무시).
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

    /// 가장 최근 egress 원장 항목을 `occurred_at` 내림차순으로 최대 `limit`개 반환한다.
    ///
    /// 동기 메서드이며, SQLite 오류 시 `warn!` 만 남기고 빈 `Vec` 를 반환한다(infallible).
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

    /// `[from, to]` 범위(RFC3339 문자열, inclusive)의 egress 원장 항목을
    /// `occurred_at` 오름차순으로 반환한다. 규제 준수 증거 export 에 사용한다.
    ///
    /// 동기 메서드이며, SQLite 오류 시 `warn!` 만 남기고 빈 `Vec` 를 반환한다(infallible).
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

/// `egress_ledger` 한 행을 [`maekon_core::models::storage_records::EgressLedgerRecord`]로
/// 매핑한다. 컬럼 순서는 SELECT 절과 일치해야 한다.
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

/// `audit_log` 한 행을 [`maekon_core::models::audit::AuditEntry`]로 매핑한다.
///
/// `entries_by_command_id`/`recent_audit_entries`가 공유하는 단일 매핑 로직.
/// 컬럼 순서는 `SELECT entry_id, timestamp, session_id, command_id, action_type,
/// status, details, execution_time_ms` 와 일치해야 한다.
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
