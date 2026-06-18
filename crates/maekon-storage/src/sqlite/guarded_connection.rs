//! `GuardedConnection` — consent-erasure barrier chokepoint (#4928 PHASE 1).
//!
//! # Design intent (by-construction completeness)
//!
//! The previous approach (per-method guards) missed sibling writers, causing leaks.
//! This newtype enforces **the single path that reaches the one SQLite connection**:
//! no adapter can obtain a barrier-free raw `Mutex<Connection>` handle.
//!
//! `SqliteStorage::connection_arc()` returns an `Arc<GuardedConnection>`, and the
//! raw `Arc<Mutex<Connection>>` accessor has been removed. All shared-connection
//! adapters (`SqliteSyncMerger`/`SqliteSyncExtractor`/`SqliteRegimeManagerStateStore`/
//! `SqliteVectorStore`/`SqliteVectorIndex`/memory_graph) access the connection only
//! through this funnel.
//!
//! # Synchronous + non-poisoning (parking_lot)
//!
//! The inner mutex is a `parking_lot::Mutex`. `.lock()` is synchronous + infallible
//! and works without panicking from any thread (including bare-async callers). This
//! eliminates the v1 defect (B2) where tokio `RwLock::blocking_read()` panicked on a
//! runtime thread. The parking_lot guard is never held across an `.await` (only
//! synchronous rusqlite operations run, then it is dropped immediately).
//!
//! # deletion_flag re-check (in one place only)
//!
//! `write_lock()` re-checks `deletion_flag` *after* acquiring the mutex. If set, it
//! returns a no-op sentinel that skips the write (returns `Ok`, never touches the DB).
//! This skip is encoded in **the single funnel only**, so every caller inherits it
//! automatically (no scattering of consent checks across call sites). Because the
//! mutex serializes all SQLite access, the check-and-write is atomic with respect to
//! erase's wipe.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rusqlite::Connection;

/// consent-erasure barrier chokepoint connection wrapper.
///
/// `conn` is a single `parking_lot::Mutex` that serializes all SQLite access.
/// `deletion_flag` is a shared signal set to `true` on consent revoke; in PHASE 2,
/// `ConsentManager`/`revoke_consent` sets it just before erase.
///
/// # Why `ArcSwap<AtomicBool>` (PHASE 2 seam)
///
/// Because `SqliteStorage.conn: Arc<GuardedConnection>`, once wrapped in an `Arc` we
/// cannot obtain `&mut self` to replace the `deletion_flag` field directly. In the
/// real composition root, `SqliteStorage` is created before `ConsentManager`
/// (storage_runtime → capture_wiring order), so sharing the same `Arc` via
/// "injection at construction time" alone is impossible. We therefore make the flag
/// cell itself an `ArcSwap`, so even `&self` can **install (swap in)** a shared
/// `Arc<AtomicBool>` (spec §3bis seam reinforcement: "the field must be swappable via
/// arc_swap").
pub struct GuardedConnection {
    conn: Mutex<Connection>,
    deletion_flag: ArcSwap<AtomicBool>,
    /// #4928 round-3 (FIX B): signal that blocks the grant_consent-during-erase TOCTOU.
    ///
    /// `grant_consent` can reset `deletion_flag` back to `false`, so if a re-consent
    /// slips into the erase window (after the Phase-1 commit ~ while Phase-2 is in
    /// progress), an in-flight writer could observe the flag as cleared and write
    /// surviving rows after the wipe. `erasing` is set at the start of
    /// `erase_all_local_data` and cleared on every exit path (success/error/panic) via
    /// RAII Drop, and `grant_consent` can never touch it. Thus, even if a re-consent
    /// race occurs, writes do not resume until erase has actually finished.
    ///
    /// The write-skip predicate is `deletion_flag || erasing` (skip if either is set).
    erasing: ArcSwap<AtomicBool>,
}

impl GuardedConnection {
    /// Creates the wrapper from a shared `deletion_flag`.
    ///
    /// In PHASE 2 (erase wiring), the composition root injects the same
    /// `Arc<AtomicBool>` as `ConsentManager`.
    pub fn new(conn: Connection, deletion_flag: Arc<AtomicBool>) -> Self {
        Self {
            conn: Mutex::new(conn),
            deletion_flag: ArcSwap::new(deletion_flag),
            // #4928 round-3: defaults to false. The composition root installs the shared `erasing`.
            erasing: ArcSwap::new(Arc::new(AtomicBool::new(false))),
        }
    }

    /// Creates the wrapper with a new `deletion_flag` (defaults to `false`).
    ///
    /// Default constructor for callers without erase wiring (storage unit tests,
    /// non-erase paths). In PHASE 2, [`Self::set_deletion_flag`] connects the shared flag.
    pub fn new_unflagged(conn: Connection) -> Self {
        Self::new(conn, Arc::new(AtomicBool::new(false)))
    }

    /// Swaps in the shared `deletion_flag` (seam for PHASE 2 composition-root wiring).
    ///
    /// Callable with `&self` alone via `Arc<GuardedConnection>` (`ArcSwap`). Once the
    /// composition root installs `ConsentManager::deletion_flag()`, every subsequent
    /// `write_lock` re-check sees the same cell.
    pub fn set_deletion_flag(&self, flag: Arc<AtomicBool>) {
        self.deletion_flag.store(flag);
    }

    /// Returns a clone of the currently installed `deletion_flag` (for ptr-eq sharing verification/tests).
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.load_full()
    }

    /// #4928 round-3 (FIX B): swaps in the shared `erasing` signal (seam for composition-root wiring).
    ///
    /// Works with `&self`, the same as `set_deletion_flag` (`ArcSwap`); the composition
    /// root installs `ConsentManager::erasing()` (the same Arc as `FrameFileStorage`).
    pub fn set_erasing(&self, erasing: Arc<AtomicBool>) {
        self.erasing.store(erasing);
    }

    /// #4928 round-3: returns a clone of the currently installed `erasing` signal (for ptr-eq verification/tests).
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.erasing.load_full()
    }

    /// Write funnel. After acquiring the mutex, re-checks the skip predicate `deletion_flag || erasing`.
    ///
    /// If either is set, returns [`WriteGuard::Skipped`] to make the write a no-op.
    /// If both are clear, [`WriteGuard::Active`] holds the connection lock.
    ///
    /// #4928 round-3 (FIX B): because `erasing` cannot be cleared by `grant_consent`,
    /// even if a re-consent resets `deletion_flag` to false within the erase window,
    /// in-flight writes are skipped until erase has actually finished (TOCTOU barrier).
    pub fn write_lock(&self) -> WriteGuard<'_> {
        let guard = self.conn.lock();
        // Re-check *after* acquiring the lock — serialized with and atomic against erase's delete.
        let skip = self.deletion_flag.load().load(Ordering::Acquire)
            || self.erasing.load().load(Ordering::Acquire);
        if skip {
            WriteGuard::Skipped
        } else {
            WriteGuard::Active(guard)
        }
    }

    /// Read funnel. Acquires only the mutex and does not check deletion_flag (reads are never skipped).
    pub fn read_lock(&self) -> ReadGuard<'_> {
        ReadGuard {
            guard: self.conn.lock(),
        }
    }

    /// Retained-table write funnel — no deletion_flag re-check.
    ///
    /// For writers that must record into tables that erase intentionally preserves
    /// (`audit_log`/`session_audit_log`/`egress_ledger`/`app_meta`, etc.) during/after erase.
    pub fn retained_write_lock(&self) -> RetainedGuard<'_> {
        RetainedGuard {
            guard: self.conn.lock(),
        }
    }

    /// Test-only direct lock — bypasses the funnel. Never use in production code
    /// (the CI gate exempts only `*/tests.rs`/`#[cfg(test)]`). A helper to preserve
    /// the existing test pattern of handling the raw connection like `storage.conn.lock()`.
    #[cfg(test)]
    pub(crate) fn test_lock(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.conn.lock()
    }

    /// The lock used by the erase body (`delete_all_data`) — no deletion_flag re-check.
    ///
    /// Erase runs while the flag is set, so re-checking would skip itself and the wipe
    /// would not happen. Behaves identically to [`Self::retained_write_lock`] and is
    /// named separately for semantic distinction.
    pub fn lock_for_erase(&self) -> RetainedGuard<'_> {
        self.retained_write_lock()
    }
}

/// Write guard returned by [`GuardedConnection::write_lock`].
///
/// `Active` holds the connection lock; `Skipped` is a no-op sentinel that holds no
/// lock because deletion_flag is set.
pub enum WriteGuard<'a> {
    /// deletion_flag clear — holds the connection lock.
    Active(parking_lot::MutexGuard<'a, Connection>),
    /// deletion_flag set — no-op. The closure is not executed.
    Skipped,
}

impl WriteGuard<'_> {
    /// Runs a closure over `&Connection`.
    ///
    /// If `Skipped` (deletion_flag set), does not run `f` and returns `Ok(skipped)`.
    /// If `Active`, runs `f` on the held connection.
    pub fn run<F, T, E>(self, skipped: T, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
    {
        match self {
            WriteGuard::Active(guard) => f(&guard),
            WriteGuard::Skipped => Ok(skipped),
        }
    }

    /// Runs a closure over `&mut Connection` (mutable access such as transactions).
    ///
    /// If `Skipped`, does not run `f` and returns `Ok(skipped)`.
    pub fn run_mut<F, T, E>(mut self, skipped: T, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Connection) -> Result<T, E>,
    {
        match &mut self {
            WriteGuard::Active(guard) => f(guard),
            WriteGuard::Skipped => Ok(skipped),
        }
    }

    /// Whether the write was skipped due to deletion_flag.
    pub fn is_skipped(&self) -> bool {
        matches!(self, WriteGuard::Skipped)
    }
}

/// Read guard returned by [`GuardedConnection::read_lock`].
///
/// Exposes `&Connection`. Reads are never skipped.
///
/// # ⚠️ Read-only (enforced by CI gate)
///
/// A rusqlite `&Connection` can technically call `execute(...)` too, but **writing
/// through this guard is forbidden** — writes must go through the
/// [`GuardedConnection::write_lock`] funnel to receive the `deletion_flag || erasing`
/// re-check (consent-erasure barrier). Smuggling a mutation through the read path
/// bypasses the erasure barrier. If a write-SQL verb appears within a `read_lock()`
/// scope, the `scripts/check-consent-erasure-barrier.sh` CI gate blocks it
/// (#4928 round-3 FIX A).
pub struct ReadGuard<'a> {
    guard: parking_lot::MutexGuard<'a, Connection>,
}

impl ReadGuard<'_> {
    /// Runs a read closure over `&Connection`.
    pub fn run<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
    {
        f(&self.guard)
    }

    /// Returns a read-only `&Connection` reference (for inline SELECT calls).
    pub fn conn(&self) -> &Connection {
        &self.guard
    }
}

/// Guard returned by [`GuardedConnection::retained_write_lock`]/[`GuardedConnection::lock_for_erase`]
/// — always writes, without checking deletion_flag.
pub struct RetainedGuard<'a> {
    guard: parking_lot::MutexGuard<'a, Connection>,
}

impl RetainedGuard<'_> {
    /// Runs a closure over `&Connection` (always runs, never skipped).
    pub fn run<F, T, E>(&self, f: F) -> Result<T, E>
    where
        F: FnOnce(&Connection) -> Result<T, E>,
    {
        f(&self.guard)
    }

    /// Runs a closure over `&mut Connection` (transactions, etc.; always runs).
    pub fn run_mut<F, T, E>(&mut self, f: F) -> Result<T, E>
    where
        F: FnOnce(&mut Connection) -> Result<T, E>,
    {
        f(&mut self.guard)
    }

    /// Returns a `&Connection` reference (for inline calls).
    pub fn conn(&self) -> &Connection {
        &self.guard
    }

    /// Returns a `&mut Connection` reference (for inline transactions).
    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.guard
    }
}

// RetainedGuard is an "always runs" guard that does not check deletion_flag, so it
// provides Deref for the convenience of inline `guard.execute(...)`/`guard.transaction()` calls.
// (The write/read guards do not provide Deref, to enforce their deletion-skip / read-only semantics.)
impl std::ops::Deref for RetainedGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.guard
    }
}

impl std::ops::DerefMut for RetainedGuard<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn fresh() -> GuardedConnection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT);")
            .unwrap();
        GuardedConnection::new_unflagged(conn)
    }

    fn count(gc: &GuardedConnection) -> i64 {
        gc.read_lock()
            .run::<_, i64, rusqlite::Error>(|c| {
                c.query_row("SELECT COUNT(*) FROM t", [], |r| r.get(0))
            })
            .unwrap()
    }

    #[test]
    fn write_lock_writes_when_flag_clear() {
        let gc = fresh();
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('a')", []));
        assert_eq!(res.unwrap(), 1);
        assert_eq!(count(&gc), 1);
    }

    #[test]
    fn write_lock_skips_when_flag_set() {
        let gc = fresh();
        gc.deletion_flag().store(true, Ordering::Release);
        // The closure must not run (returns 0), and the row count must not change.
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('b')", []));
        assert_eq!(res.unwrap(), 0, "skipped write returns the skip sentinel");
        assert_eq!(count(&gc), 0, "skipped write must not touch the DB");
    }

    #[test]
    fn write_lock_run_mut_skips_when_flag_set() {
        let gc = fresh();
        gc.deletion_flag().store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc.write_lock().run_mut(0, |c| {
            let tx = c.transaction()?;
            tx.execute("INSERT INTO t (v) VALUES ('c')", [])?;
            tx.commit()?;
            Ok(1)
        });
        assert_eq!(res.unwrap(), 0);
        assert_eq!(count(&gc), 0);
    }

    #[test]
    fn read_lock_never_skips() {
        let gc = fresh();
        gc.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute("INSERT INTO t (v) VALUES ('a')", [])
            })
            .unwrap();
        // Reads work normally even when the flag is set.
        gc.deletion_flag().store(true, Ordering::Release);
        assert_eq!(count(&gc), 1);
    }

    #[test]
    fn retained_write_lock_writes_even_when_flag_set() {
        let gc = fresh();
        gc.deletion_flag().store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc
            .retained_write_lock()
            .run(|c| c.execute("INSERT INTO t (v) VALUES ('retained')", []));
        assert_eq!(
            res.unwrap(),
            1,
            "retained write must bypass the deletion flag"
        );
        assert_eq!(count(&gc), 1);
    }

    #[test]
    fn lock_for_erase_runs_when_flag_set() {
        let gc = fresh();
        gc.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute("INSERT INTO t (v) VALUES ('a')", [])
            })
            .unwrap();
        gc.deletion_flag().store(true, Ordering::Release);
        // The erase body must run even when the flag is set.
        gc.lock_for_erase()
            .run_mut::<_, (), rusqlite::Error>(|c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM t", [])?;
                tx.commit()?;
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&gc), 0, "erase must wipe regardless of the flag");
    }

    #[test]
    fn set_deletion_flag_shares_signal() {
        let gc = fresh();
        let shared = Arc::new(AtomicBool::new(false));
        // #4928 PHASE 2: set_deletion_flag now works with &self (ArcSwap).
        gc.set_deletion_flag(shared.clone());
        // When the shared flag is set externally, the funnel must skip.
        shared.store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('x')", []));
        assert_eq!(res.unwrap(), 0);
        assert_eq!(count(&gc), 0);
    }

    /// #4928 round-3 (FIX B): if `erasing` is set, the write must be skipped even when
    /// deletion_flag is clear (funnel-level proof of the grant_consent-during-erase TOCTOU barrier).
    #[test]
    fn write_lock_skips_when_erasing_set_even_if_deletion_flag_clear() {
        let gc = fresh();
        let erasing = Arc::new(AtomicBool::new(false));
        gc.set_erasing(erasing.clone());
        // deletion_flag is clear, only erasing is set.
        assert!(!gc.deletion_flag().load(Ordering::Acquire));
        erasing.store(true, Ordering::Release);

        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('e')", []));
        assert_eq!(res.unwrap(), 0, "write must be skipped when erasing is set");
        assert_eq!(count(&gc), 0);

        // After erasing is cleared, writes resume (since deletion_flag is also clear).
        erasing.store(false, Ordering::Release);
        let res2: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('ok')", []));
        assert_eq!(res2.unwrap(), 1);
        assert_eq!(count(&gc), 1);
    }

    /// #4928 round-3 (FIX B): verifies the cell installed by `set_erasing` is shared via ptr-eq.
    #[test]
    fn set_erasing_through_arc_is_ptr_eq() {
        let gc = Arc::new(fresh());
        let shared = Arc::new(AtomicBool::new(false));
        gc.set_erasing(shared.clone());
        let observed = gc.erasing();
        assert!(
            Arc::ptr_eq(&observed, &shared),
            "the installed erasing and the erasing() return value must be the same Arc (ptr-eq)"
        );
    }

    /// #4928 round-3 (FIX B): reads never skip, even when erasing is set.
    #[test]
    fn read_lock_never_skips_even_when_erasing_set() {
        let gc = fresh();
        gc.write_lock()
            .run::<_, usize, rusqlite::Error>(0, |c| {
                c.execute("INSERT INTO t (v) VALUES ('a')", [])
            })
            .unwrap();
        let erasing = Arc::new(AtomicBool::new(true));
        gc.set_erasing(erasing);
        assert_eq!(
            count(&gc),
            1,
            "reads must work normally even when erasing is set"
        );
    }

    /// #4928 PHASE 2: verifies `set_deletion_flag` can be called with `&self` via
    /// `Arc<GuardedConnection>`, and that after install `deletion_flag()` returns the
    /// same `Arc` (ptr-eq) — proving the seam actually rewires the LIVE flag.
    #[test]
    fn set_deletion_flag_through_arc_is_ptr_eq() {
        let gc = Arc::new(fresh());
        let shared = Arc::new(AtomicBool::new(false));
        // Called with &self (inside an Arc) — swapped in even without &mut.
        gc.set_deletion_flag(shared.clone());
        let observed = gc.deletion_flag();
        assert!(
            Arc::ptr_eq(&observed, &shared),
            "the installed flag and the deletion_flag() return value must be the same Arc (ptr-eq)"
        );
        // Setting the installed cell makes the funnel skip immediately.
        shared.store(true, Ordering::Release);
        let res: Result<usize, rusqlite::Error> = gc
            .write_lock()
            .run(0, |c| c.execute("INSERT INTO t (v) VALUES ('y')", []));
        assert_eq!(res.unwrap(), 0);
        assert_eq!(count(&gc), 0);
    }
}
