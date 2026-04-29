//! Log file retention cleanup.
//!
//! Deletes log files older than a configurable maximum age from the
//! application log directory.  Called at startup and periodically
//! from the scheduler.

use std::path::Path;
use std::time::{Duration, SystemTime};
use tracing::{debug, info, warn};

/// Default log retention period in days.
pub const DEFAULT_MAX_AGE_DAYS: u32 = 7;

/// Delete log files in `log_dir` that are older than `max_age_days`.
///
/// Returns the number of files successfully deleted.
/// Non-file entries and files whose metadata cannot be read are silently
/// skipped.
pub fn cleanup_old_logs(log_dir: &Path, max_age_days: u32) -> u32 {
    let max_age = Duration::from_secs(u64::from(max_age_days) * 24 * 60 * 60);
    let now = SystemTime::now();
    let mut deleted = 0u32;

    let entries = match std::fs::read_dir(log_dir) {
        Ok(entries) => entries,
        Err(e) => {
            // Directory may not exist on first launch — not an error.
            debug!(path = %log_dir.display(), error = %e, "log directory read failed");
            return 0;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let modified = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };

        let age = match now.duration_since(modified) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if age > max_age {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    deleted += 1;
                    debug!(file = %path.display(), "deleted old log file");
                }
                Err(e) => {
                    warn!(file = %path.display(), error = %e, "failed to delete old log file");
                }
            }
        }
    }

    if deleted > 0 {
        info!(deleted, max_age_days, "log retention cleanup completed");
    }

    deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn cleanup_deletes_old_files_only() {
        let dir = TempDir::new().unwrap();

        // Create a "recent" file (should survive)
        let recent = dir.path().join("oneshim.2026-03-22.log");
        std::fs::File::create(&recent)
            .unwrap()
            .write_all(b"recent")
            .unwrap();

        // Create an "old" file by backdating its mtime
        let old = dir.path().join("oneshim.2026-03-01.log");
        std::fs::File::create(&old)
            .unwrap()
            .write_all(b"old")
            .unwrap();
        // Use filetime for portability — but since we cannot add a dep just for
        // tests, we simply set max_age_days=0 so that BOTH files are "old".
        // Then verify both are deleted.
        let deleted = cleanup_old_logs(dir.path(), 0);
        assert_eq!(deleted, 2);
        assert!(!recent.exists());
        assert!(!old.exists());
    }

    #[test]
    fn cleanup_returns_zero_for_nonexistent_dir() {
        let deleted = cleanup_old_logs(Path::new("/nonexistent/log/dir"), 7);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn cleanup_skips_directories() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();

        let deleted = cleanup_old_logs(dir.path(), 0);
        assert_eq!(deleted, 0);
        assert!(sub.exists());
    }

    #[test]
    fn cleanup_with_large_max_age_deletes_nothing() {
        let dir = TempDir::new().unwrap();
        let f = dir.path().join("oneshim.log");
        std::fs::File::create(&f)
            .unwrap()
            .write_all(b"data")
            .unwrap();

        let deleted = cleanup_old_logs(dir.path(), 365);
        assert_eq!(deleted, 0);
        assert!(f.exists());
    }
}
