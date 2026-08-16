//! Readable startup failures instead of `Abort trap: 6` (#10985).
//!
//! Tauri's run loop calls `panic!("Failed to setup app: {e}")` when the setup
//! hook returns `Err` (`patches/tauri-2.11.2/src/app.rs:1417`), and that panic
//! crosses a non-unwinding extern boundary, so the process aborts. A user who
//! rolls back to an older build sees:
//!
//! ```text
//! thread 'main' panicked at ... Failed to setup app: ...
//! panic in a function that cannot unwind
//! Abort trap: 6
//! ```
//!
//! …with a raw backtrace and nothing to act on. Worse, when the cause is a
//! profile written by a newer build, a pre-migration backup usually exists and
//! nothing points at it.
//!
//! The formatting lives here, cfg-free and unit-tested on any host, while the
//! process exit stays at the call site — same seam as `sandbox::sbpl` (#5120)
//! and `sandbox::win_limits` (#5138).

use std::path::Path;

/// Substrings that identify a database that cannot be opened at all, as opposed
/// to one that opens and then fails a version check.
///
/// SQLCipher reports a wrong or missing key as "file is not a database", which
/// is what a rollback produces. Matched on the message because the error
/// arrives as `Box<dyn Error>` by the time it reaches the setup hook.
const DB_OPEN_FAILURE_MARKERS: &[&str] = &[
    "file is not a database",
    "Failed to apply PRAGMA settings",
    "file is encrypted",
];

/// Whether `error` looks like the storage layer failing to open the profile
/// database, rather than some other setup failure.
pub fn looks_like_unopenable_database(error: &str) -> bool {
    let lowered = error.to_ascii_lowercase();
    DB_OPEN_FAILURE_MARKERS
        .iter()
        .any(|marker| lowered.contains(&marker.to_ascii_lowercase()))
}

/// Newest pre-migration backup in `data_dir`, if the storage layer wrote one.
///
/// Migrations name these `maekon.backup.v<schema>.<epoch>`, so the lexically
/// greatest name is the most recent — the epoch suffix is fixed-width for any
/// timestamp this product will see.
pub fn newest_backup_name(entries: impl IntoIterator<Item = String>) -> Option<String> {
    entries
        .into_iter()
        .filter(|name| name.starts_with("maekon.backup.v"))
        .max()
}

/// The message a user should see instead of an abort.
///
/// Names the backup when one exists, because "your data is recoverable" is the
/// only part of this a user can act on, and it is invisible otherwise.
pub fn format_startup_failure(error: &str, data_dir: &Path, backup: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str("Maekon could not start.\n\n");

    if looks_like_unopenable_database(error) {
        out.push_str(
            "The profile database could not be opened. This usually means it belongs to a \
             NEWER version of Maekon than the one you are running — for example after \
             installing an update and then going back to an older build.\n\n",
        );
        out.push_str("What to do:\n");
        out.push_str("  1. Reinstall the newer version; it can read this profile.\n");
        match backup {
            Some(name) => {
                out.push_str(&format!(
                    "  2. Or restore the pre-update backup, taken automatically before the \
                     database was migrated:\n       {}\n",
                    data_dir.join(name).display()
                ));
            }
            None => {
                out.push_str(&format!(
                    "  2. Or start from a fresh profile by moving this directory aside:\n\
                     \x20      {}\n",
                    data_dir.display()
                ));
            }
        }
        out.push('\n');
    }

    out.push_str(&format!("Details: {error}\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recognises_the_rollback_signature() {
        assert!(looks_like_unopenable_database(
            "internal error: Failed to apply PRAGMA settings: file is not a database"
        ));
        assert!(looks_like_unopenable_database(
            "file is encrypted or is not a database"
        ));
    }

    #[test]
    fn does_not_claim_every_startup_failure_is_a_rollback() {
        // A message that blames a rollback for an unrelated failure sends the
        // user to restore a backup that will not help.
        assert!(!looks_like_unopenable_database(
            "failed to bind web server port 10090: address already in use"
        ));
        assert!(!looks_like_unopenable_database("tray icon creation failed"));
    }

    #[test]
    fn database_failure_names_the_backup_so_recovery_is_discoverable() {
        let msg = format_startup_failure(
            "Failed to apply PRAGMA settings: file is not a database",
            &PathBuf::from("/data"),
            Some("maekon.backup.v41.1786806739"),
        );
        assert!(
            msg.contains("maekon.backup.v41.1786806739"),
            "the backup is the only actionable part; got: {msg}"
        );
        assert!(
            msg.contains("NEWER version"),
            "the message must explain the cause, not just restate the error; got: {msg}"
        );
    }

    #[test]
    fn without_a_backup_it_offers_the_next_best_step() {
        let msg = format_startup_failure("file is not a database", &PathBuf::from("/data"), None);
        assert!(
            msg.contains("/data"),
            "with no backup the user still needs to know which directory to move; got: {msg}"
        );
        assert!(
            !msg.contains("maekon.backup"),
            "must not point at a backup that does not exist; got: {msg}"
        );
    }

    #[test]
    fn unrelated_failures_report_the_error_without_rollback_advice() {
        let msg = format_startup_failure(
            "address already in use",
            &PathBuf::from("/data"),
            Some("maekon.backup.v41.1786806739"),
        );
        assert!(msg.contains("address already in use"), "got: {msg}");
        assert!(
            !msg.contains("maekon.backup.v41"),
            "a port conflict must not send the user to restore a database backup; got: {msg}"
        );
    }

    #[test]
    fn newest_backup_wins_and_non_backups_are_ignored() {
        let entries = vec![
            "maekon.db".to_string(),
            "maekon.backup.v41.1786806739".to_string(),
            "maekon.backup.v41.1786800000".to_string(),
            "logs".to_string(),
        ];
        assert_eq!(
            newest_backup_name(entries),
            Some("maekon.backup.v41.1786806739".to_string())
        );
    }

    #[test]
    fn no_backups_present_yields_none() {
        let entries = vec!["maekon.db".to_string(), "config.json".to_string()];
        assert_eq!(newest_backup_name(entries), None);
    }
}
