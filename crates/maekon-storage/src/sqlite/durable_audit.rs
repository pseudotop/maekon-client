//! Synced, independent WAL commits for audit acknowledgments (#12458).

use crate::error::StorageError;
use rusqlite::{Connection, Transaction, TransactionBehavior};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings {
    synchronous: i64,
    fullfsync: i64,
}

impl Settings {
    fn read(conn: &Connection) -> Result<Self, StorageError> {
        Ok(Self {
            synchronous: conn.pragma_query_value(Some("main"), "synchronous", |row| row.get(0))?,
            fullfsync: conn.pragma_query_value(None, "fullfsync", |row| row.get(0))?,
        })
    }

    fn apply(self, conn: &Connection) -> Result<(), StorageError> {
        // Attempt both restorations even when the first one fails.
        let synchronous = conn.pragma_update(Some("main"), "synchronous", self.synchronous);
        let fullfsync = conn.pragma_update(None, "fullfsync", self.fullfsync);
        synchronous?;
        fullfsync?;
        if Self::read(conn)? != self {
            return Err(StorageError::Internal(
                "Audit synchronization settings were not applied".into(),
            ));
        }
        Ok(())
    }
}

struct Restore<'a> {
    conn: &'a Connection,
    original: Option<Settings>,
}

impl Restore<'_> {
    fn restore(&mut self) -> Result<(), StorageError> {
        match self.original.take() {
            Some(original) => original.apply(self.conn),
            None => Ok(()),
        }
    }
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if self.restore().is_err() {
            tracing::warn!("Audit synchronization restoration failed");
        }
    }
}

fn require_wal(conn: &Connection) -> Result<(), StorageError> {
    let mode: String = conn.pragma_query_value(Some("main"), "journal_mode", |row| row.get(0))?;
    if mode != "wal" {
        return Err(StorageError::Internal(
            "Durable audit requires a WAL database".into(),
        ));
    }
    Ok(())
}

/// The caller retains the connection mutex through setting, append, commit and
/// restoration. FULL asks SQLite's VFS to sync each WAL commit; fullfsync also
/// requests the stronger platform flush where supported. This is not a claim
/// about a faulty filesystem/device or a measured power-cut experiment.
pub(super) fn commit(
    conn: &Connection,
    append: impl FnOnce(&Connection) -> Result<bool, StorageError>,
) -> Result<bool, StorageError> {
    if !conn.is_autocommit() || conn.path().is_none_or(str::is_empty) {
        return Err(StorageError::Internal(
            "Durable audit requires an independent file-backed commit".into(),
        ));
    }
    require_wal(conn)?;
    let original = Settings::read(conn)?;
    let mut restoration = Restore {
        conn,
        original: Some(original),
    };
    Settings {
        synchronous: original.synchronous.max(2),
        fullfsync: 1,
    }
    .apply(conn)?;

    let outcome = (|| {
        // Serialize the chain-tip read against other database connections too.
        let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
        // Recheck under the database write lock: another connection could have
        // changed journal mode between the first observation and BEGIN.
        require_wal(&transaction)?;
        let inserted = append(&transaction)?;
        transaction.commit()?;
        Ok(inserted)
    })();
    // The transaction has committed or rolled back before PRAGMA restoration.
    // A restoration failure must never acknowledge permission to send.
    restoration.restore()?;
    outcome
}

#[cfg(test)]
#[path = "durable_audit_tests.rs"]
mod tests;
