//! Cross-process advisory lock on the data directory (#6864).
//!
//! The Tauri single-instance plugin enforces single-instance via DBUS on Linux;
//! in a headless / DBUS-absent session that enforcement degrades (see the
//! `single_instance_dbus_absent` warn path), leaving a window where two processes
//! could open the same DB file — the precondition for the SQLite WAL-reset
//! data-race (#6830 / supply-chain audit). This advisory `flock` on a lockfile in
//! the data dir closes that window independently of DBUS: a second instance that
//! reaches the storage-open chokepoint fails to acquire the lock and aborts.
//!
//! `flock` is released automatically when the file descriptor closes (process
//! exit / [`ProcessLock`] drop), so — unlike a PID file — there is no stale-lock
//! starvation if a previous instance was killed.
//!
//! On Windows this is a no-op: the OS single-instance path is robust there and the
//! DBUS-absent gap this guards is Linux-specific. On macOS the unix `flock` path is
//! used (defense-in-depth; the OS single-instance is also robust).

use std::path::Path;

use crate::error::StorageError;

/// RAII advisory lock on the data directory. Held for the process lifetime (stored
/// in the storage runtime bundle), it is released when dropped / the process exits.
#[derive(Debug)]
pub struct ProcessLock {
    /// Holds the locked file descriptor open; closing it releases the `flock`.
    #[cfg(unix)]
    _file: std::fs::File,
}

impl ProcessLock {
    /// Try to acquire the advisory lock at `lock_path` (e.g.
    /// `<data_dir>/maekon.lock`), non-blocking.
    ///
    /// # Errors
    /// - [`StorageError::Internal`] when another process already holds the lock
    ///   (a second instance reached the storage-open chokepoint).
    /// - [`StorageError::Io`] on a genuine filesystem / `flock` failure.
    pub fn try_acquire(lock_path: &Path) -> Result<Self, StorageError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;

            let file = std::fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false)
                .open(lock_path)?;
            // SAFETY: `file` owns a valid open fd for the duration of this call.
            // `LOCK_NB` makes the request non-blocking, so we never hang waiting on
            // a held lock — a contended lock returns immediately with EWOULDBLOCK.
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    return Err(StorageError::Internal(format!(
                        "another maekon instance holds the data-directory lock ({}); \
                         refusing to open the database concurrently",
                        lock_path.display()
                    )));
                }
                return Err(StorageError::Io(err));
            }
            Ok(Self { _file: file })
        }
        #[cfg(not(unix))]
        {
            // Windows: the OS single-instance path is robust and the DBUS-absent
            // gap this guards is Linux-specific, so the advisory lock is a no-op.
            let _ = lock_path;
            Ok(Self {})
        }
    }

    /// Hold the lock for the remainder of the process by intentionally leaking the
    /// guard: the underlying fd stays open (so the advisory `flock` stays held)
    /// until the process exits, at which point the OS closes the fd and releases it.
    ///
    /// This is the correct lifetime for a single-instance lock — it must outlive ALL
    /// startup wiring. Holding the RAII guard in a startup-scoped local instead (e.g.
    /// a builder bundle that is borrowed/cloned but never moved into long-lived
    /// state) would release the lock when that local drops at the end of startup,
    /// silently defeating the guarantee — exactly the regression caught for #6864.
    pub fn hold_for_process_lifetime(self) {
        std::mem::forget(self);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_contends_then_release_allows_reacquire() {
        // #6864: a second acquire on the same lock path (a separate open fd, the
        // cross-process case) must fail while the first is held, then succeed once
        // the first is released — proving the single-instance guarantee.
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join("maekon.lock");

        let first = ProcessLock::try_acquire(&lock_path).expect("first acquire must succeed");
        // #7012: assert the contended-lock variant, not just "is an error" — a held
        // lock returns StorageError::Internal (EWOULDBLOCK), whereas a genuine
        // filesystem/flock failure returns StorageError::Io. A value-blind
        // assert!(..is_err()) would pass on the Io path too (a different bug).
        let second_err = ProcessLock::try_acquire(&lock_path)
            .expect_err("a second acquire must fail while the lock is held");
        assert!(
            matches!(second_err, StorageError::Internal(_)),
            "second acquire must fail with Internal (lock held), got: {second_err:?}"
        );

        drop(first);
        // #7012: non-value-blind — `.expect()` asserts the re-acquire actually
        // succeeds (and binds the guard so the lock is held, not dropped instantly).
        let _third = ProcessLock::try_acquire(&lock_path)
            .expect("re-acquire must succeed after the first lock is released");
    }
}
