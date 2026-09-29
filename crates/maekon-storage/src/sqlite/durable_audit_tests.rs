use super::*;
use crate::sqlite::SqliteStorage;
use chrono::Utc;
use maekon_core::models::audit::{AuditEntry, AuditStatus};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{atomic::Ordering, Arc, Barrier};
use tempfile::TempDir;

fn disk() -> (TempDir, SqliteStorage) {
    let dir = tempfile::tempdir().unwrap();
    let storage = SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap();
    (dir, storage)
}

fn entry(id: &str) -> AuditEntry {
    AuditEntry {
        entry_id: id.into(),
        timestamp: Utc::now(),
        session_id: "audit-test".into(),
        command_id: "candidate".into(),
        action_type: "candidate.advisory".into(),
        status: AuditStatus::Started,
        details: Some("metadata-only".into()),
        execution_time_ms: Some(7),
    }
}

fn settings(storage: &SqliteStorage) -> Settings {
    Settings::read(&storage.conn.test_lock()).unwrap()
}

fn count(storage: &SqliteStorage) -> i64 {
    storage
        .conn
        .test_lock()
        .query_row("SELECT count(*) FROM audit_log", [], |row| row.get(0))
        .unwrap()
}

fn assert_internal(error: StorageError, expected: &str) {
    let StorageError::Internal(message) = error else {
        panic!("expected internal storage failure: {error:?}");
    };
    assert_eq!(message, expected);
}

fn assert_sqlite(error: StorageError, expected: rusqlite::ErrorCode, detail: Option<&str>) {
    let StorageError::Sqlite(rusqlite::Error::SqliteFailure(code, message)) = error else {
        panic!("expected SQLite failure: {error:?}");
    };
    assert_eq!(code.code, expected);
    if let Some(detail) = detail {
        assert_eq!(message.as_deref(), Some(detail));
    }
}

#[test]
fn synced_audit_observes_full_commit_settings_and_restores_each_prior_mode() {
    let (_dir, storage) = disk();
    storage
        .conn
        .test_lock()
        .execute_batch(
            "CREATE TABLE observed_sync(entry_id TEXT, sync INTEGER, full INTEGER);
         CREATE TRIGGER observe_audit_sync BEFORE INSERT ON audit_log BEGIN
           INSERT INTO observed_sync SELECT NEW.entry_id, synchronous, fullfsync
             FROM pragma_synchronous, pragma_fullfsync;
         END;",
        )
        .unwrap();
    for synchronous in 0..=3 {
        for fullfsync in 0..=1 {
            let prior = Settings {
                synchronous,
                fullfsync,
            };
            prior.apply(&storage.conn.test_lock()).unwrap();
            let id = format!("mode-{synchronous}-{fullfsync}");
            assert!(storage.try_save_durable_audit_entry(&entry(&id)).unwrap());
            let observed: (i64, i64) = storage
                .conn
                .test_lock()
                .query_row(
                    "SELECT sync, full FROM observed_sync WHERE entry_id=?1",
                    [&id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(observed, (synchronous.max(2), 1));
            assert_eq!(settings(&storage), prior);
        }
    }
    assert_eq!(count(&storage), 8);
    assert!(storage.verify_audit_chain().ok);
}

#[test]
fn synced_audit_duplicate_is_not_acknowledged_and_chain_survives_reopen() {
    let (dir, storage) = disk();
    let first = entry("first");
    assert!(storage.try_save_durable_audit_entry(&first).unwrap());
    assert!(!storage.try_save_durable_audit_entry(&first).unwrap());
    assert!(storage.try_save_audit_entry(&entry("ordinary")).unwrap());
    assert!(storage
        .try_save_durable_audit_entry(&entry("last"))
        .unwrap());
    drop(storage);
    // Reopening observes persistence and hash-chain integrity, not power-loss
    // resistance. The INSERT trigger in the other test observes sync policy.
    let reopened = SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap();
    assert_eq!(count(&reopened), 3);
    let chain = reopened.verify_audit_chain();
    assert!(chain.ok);
    assert_eq!(chain.first_seq, Some(0));
    assert_eq!(chain.last_seq, Some(2));
    assert_eq!(chain.verified_count, 3);
    let details: String = reopened
        .conn
        .test_lock()
        .query_row(
            "SELECT details FROM audit_log WHERE entry_id='first'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(details, "metadata-only");
}

#[test]
fn synced_audit_rejects_memory_temporary_and_non_wal_databases() {
    let memory = SqliteStorage::open_in_memory(30).unwrap();
    let prior = settings(&memory);
    assert_internal(
        memory
            .try_save_durable_audit_entry(&entry("memory"))
            .unwrap_err(),
        "Durable audit requires an independent file-backed commit",
    );
    assert_eq!(count(&memory), 0);
    assert_eq!(settings(&memory), prior);
    // The legacy commit-only API remains usable by ordinary in-memory callers.
    assert!(memory
        .try_save_audit_entry(&entry("ordinary-memory"))
        .unwrap());
    let temporary = Connection::open("").unwrap();
    assert_internal(
        commit(&temporary, |_| panic!("temporary DB must not append")).unwrap_err(),
        "Durable audit requires an independent file-backed commit",
    );

    let (_dir, storage) = disk();
    for mode in ["DELETE", "TRUNCATE", "PERSIST", "MEMORY", "OFF"] {
        storage
            .conn
            .test_lock()
            .pragma_update(None, "journal_mode", mode)
            .unwrap();
        let prior = settings(&storage);
        assert_internal(
            storage
                .try_save_durable_audit_entry(&entry(mode))
                .unwrap_err(),
            "Durable audit requires a WAL database",
        );
        assert_eq!(count(&storage), 0);
        assert_eq!(settings(&storage), prior);
    }
}

#[test]
fn synced_audit_does_not_commit_an_existing_caller_transaction() {
    let (_dir, storage) = disk();
    let prior = settings(&storage);
    storage
        .conn
        .test_lock()
        .execute_batch(
            "CREATE TABLE caller_work(value INTEGER);
         BEGIN; INSERT INTO caller_work VALUES (7);",
        )
        .unwrap();
    assert_internal(
        storage
            .try_save_durable_audit_entry(&entry("nested"))
            .unwrap_err(),
        "Audit acknowledgment requires an independent commit",
    );
    let conn = storage.conn.test_lock();
    assert!(!conn.is_autocommit());
    assert_internal(
        commit(&conn, |_| panic!("existing transaction must not append")).unwrap_err(),
        "Durable audit requires an independent file-backed commit",
    );
    assert_eq!(Settings::read(&conn).unwrap(), prior);
    let pending: i64 = conn
        .query_row("SELECT value FROM caller_work", [], |row| row.get(0))
        .unwrap();
    assert_eq!(pending, 7);
    conn.execute_batch("ROLLBACK").unwrap();
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM caller_work", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    drop(conn);
    assert_eq!(count(&storage), 0);
}

#[test]
fn synced_audit_sql_abort_and_integer_failures_restore_settings_without_rows() {
    let (_dir, storage) = disk();
    let prior = settings(&storage);
    storage
        .conn
        .test_lock()
        .execute_batch(
            "CREATE TRIGGER reject_audit BEFORE INSERT ON audit_log BEGIN
           SELECT RAISE(ABORT, 'fixture rejection');
         END;",
        )
        .unwrap();
    assert_sqlite(
        storage
            .try_save_durable_audit_entry(&entry("abort"))
            .unwrap_err(),
        rusqlite::ErrorCode::ConstraintViolation,
        Some("fixture rejection"),
    );
    assert_eq!(settings(&storage), prior);
    assert!(storage.conn.test_lock().is_autocommit());
    assert_eq!(count(&storage), 0);
    storage
        .conn
        .test_lock()
        .execute_batch("DROP TRIGGER reject_audit")
        .unwrap();

    let mut oversized = entry("duration-overflow");
    oversized.execution_time_ms = Some(u64::MAX);
    let error = storage
        .try_save_durable_audit_entry(&oversized)
        .unwrap_err();
    let StorageError::Validation { field, message } = error else {
        panic!("expected audit duration validation failure: {error:?}");
    };
    assert_eq!(field, "execution_time_ms");
    assert_eq!(message, "Audit duration exceeds the SQLite integer range");
    assert_eq!(count(&storage), 0);
    assert!(storage
        .try_save_durable_audit_entry(&entry("valid"))
        .unwrap());
    storage
        .conn
        .test_lock()
        .execute(
            "UPDATE audit_log SET seq=?1 WHERE entry_id='valid'",
            [i64::MAX],
        )
        .unwrap();
    assert_internal(
        storage
            .try_save_durable_audit_entry(&entry("sequence-overflow"))
            .unwrap_err(),
        "audit sequence overflow",
    );
    assert_eq!(count(&storage), 1);
    assert_eq!(settings(&storage), prior);
}

#[test]
fn synced_audit_holds_database_write_lock_and_rolls_back_callback_errors() {
    let (dir, storage) = disk();
    storage
        .conn
        .test_lock()
        .execute_batch("CREATE TABLE tx_probe(value INTEGER)")
        .unwrap();
    let other = Connection::open(dir.path().join("audit.db")).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    let conn = storage.conn.test_lock();
    let prior = Settings::read(&conn).unwrap();
    let outcome = commit(&conn, |transaction| {
        assert!(!transaction.is_autocommit());
        assert_eq!(
            Settings::read(transaction).unwrap(),
            Settings {
                synchronous: 2,
                fullfsync: 1
            }
        );
        // The lock must exist before the callback's chain-tip read or writes.
        let conflict = other
            .execute("INSERT INTO tx_probe VALUES (1)", [])
            .unwrap_err();
        assert_eq!(
            conflict.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy)
        );
        transaction.execute("INSERT INTO tx_probe VALUES (2)", [])?;
        Err(StorageError::Internal("fixture callback failure".into()))
    });
    assert_internal(outcome.unwrap_err(), "fixture callback failure");
    assert!(conn.is_autocommit());
    assert_eq!(Settings::read(&conn).unwrap(), prior);
    let rows: i64 = other
        .query_row("SELECT count(*) FROM tx_probe", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn synced_audit_unwind_restores_settings_and_rolls_back_its_transaction() {
    let (_dir, storage) = disk();
    let conn = storage.conn.test_lock();
    let prior = Settings::read(&conn).unwrap();
    conn.execute_batch("CREATE TABLE unwind_probe(value INTEGER)")
        .unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = commit(&conn, |transaction| {
            transaction.execute("INSERT INTO unwind_probe VALUES (1)", [])?;
            panic!("fixture unwind");
        });
    }));
    let payload = result.unwrap_err();
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"fixture unwind"));
    assert!(conn.is_autocommit());
    assert_eq!(Settings::read(&conn).unwrap(), prior);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM unwind_probe", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn synced_audit_restoration_errors_are_errors_and_do_not_end_caller_work() {
    let (_dir, storage) = disk();
    let conn = storage.conn.test_lock();
    let original = Settings::read(&conn).unwrap();
    let mut restoration = Restore {
        conn: &conn,
        original: Some(original),
    };
    Settings {
        synchronous: 2,
        fullfsync: 1,
    }
    .apply(&conn)
    .unwrap();
    conn.execute_batch("BEGIN").unwrap();
    assert_sqlite(
        restoration.restore().unwrap_err(),
        rusqlite::ErrorCode::Unknown,
        Some("Safety level may not be changed inside a transaction"),
    );
    assert!(!conn.is_autocommit());
    // The guard attempts restoration once and cannot commit caller work.
    drop(restoration);
    assert!(!conn.is_autocommit());
    conn.execute_batch("ROLLBACK").unwrap();
    original.apply(&conn).unwrap();
}

#[test]
fn synced_audit_commits_with_a_reader_snapshot_and_fails_on_an_external_writer() {
    let (dir, storage) = disk();
    let other = Connection::open(dir.path().join("audit.db")).unwrap();
    other.execute_batch("BEGIN").unwrap();
    let snapshot: i64 = other
        .query_row("SELECT count(*) FROM audit_log", [], |row| row.get(0))
        .unwrap();
    assert_eq!(snapshot, 0);
    assert!(storage
        .try_save_durable_audit_entry(&entry("reader-active"))
        .unwrap());
    let old_snapshot: i64 = other
        .query_row("SELECT count(*) FROM audit_log", [], |row| row.get(0))
        .unwrap();
    assert_eq!(old_snapshot, 0);
    other.execute_batch("COMMIT; BEGIN IMMEDIATE").unwrap();
    storage
        .conn
        .test_lock()
        .busy_timeout(std::time::Duration::ZERO)
        .unwrap();
    let prior = settings(&storage);
    assert_sqlite(
        storage
            .try_save_durable_audit_entry(&entry("writer-active"))
            .unwrap_err(),
        rusqlite::ErrorCode::DatabaseBusy,
        Some("database is locked"),
    );
    assert_eq!(settings(&storage), prior);
    assert!(storage.conn.test_lock().is_autocommit());
    other.execute_batch("ROLLBACK").unwrap();
    assert_eq!(count(&storage), 1);
    assert!(storage
        .try_save_durable_audit_entry(&entry("writer-finished"))
        .unwrap());
    assert!(storage.verify_audit_chain().ok);
}

#[test]
fn synced_audit_serializes_clones_and_independent_database_connections() {
    let (dir, first) = disk();
    let second = SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap();
    let stores = [Arc::new(first), Arc::new(second)];
    let barrier = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4)
        .map(|worker| {
            let storage = stores[worker % 2].clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for index in 0..8 {
                    assert!(storage
                        .try_save_durable_audit_entry(&entry(&format!("worker-{worker}-{index}")),)
                        .unwrap());
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(count(&stores[0]), 32);
    let chain = stores[1].verify_audit_chain();
    assert!(chain.ok, "{chain:?}");
    assert_eq!(chain.last_seq, Some(31));
    assert_eq!(chain.verified_count, 32);
}

#[test]
fn synced_audit_retains_erasure_audits_after_the_deletion_flag() {
    let (_dir, storage) = disk();
    assert!(storage
        .try_save_durable_audit_entry(&entry("before"))
        .unwrap());
    storage.deletion_flag().store(true, Ordering::SeqCst);
    assert!(storage
        .try_save_durable_audit_entry(&entry("revoked"))
        .unwrap());
    assert_eq!(count(&storage), 2);
    assert!(storage.verify_audit_chain().ok);
}
