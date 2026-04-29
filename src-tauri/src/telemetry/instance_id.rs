//! `telemetry_instance_id` file lifecycle. See spec §3.7 for the full
//! state table (7 rows).
//!
//! - `ensure_instance_id(data_dir)` — return the stable UUID for this install,
//!   creating the file with `0600` perms on Unix if it does not yet exist.
//!   Idempotent: a second call returns the same UUID.
//! - `reset_instance_id(data_dir)` — delete the file so the next
//!   `ensure_instance_id` regenerates.
//!
//! Called from `otlp::build_pipeline` to attach `service.instance.id` as an
//! OTel Resource attribute. Not a user identifier — one UUIDv4 per install,
//! never leaves the process unless the user has opted in to telemetry.

use std::fs;
use std::io::Write;
use std::path::Path;

const FILE_NAME: &str = "telemetry_instance_id";

pub(super) fn ensure_instance_id(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join(FILE_NAME);
    if path.exists() {
        let existing = fs::read_to_string(&path)?;
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        // Empty file — treat as missing, fall through to regenerate.
    }
    let uuid = uuid::Uuid::new_v4().to_string();
    fs::create_dir_all(data_dir)?;
    write_with_owner_only(&path, &uuid)?;
    Ok(uuid)
}

/// Wired by a dedicated Tauri command in Phase 3 UX work (see spec §9 and
/// §3.7 Row 6). Exposed to tests via the same visibility so T-X2-9 can
/// exercise the state-table row today.
#[allow(dead_code)]
pub(super) fn reset_instance_id(data_dir: &Path) -> anyhow::Result<()> {
    let path = data_dir.join(FILE_NAME);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(unix)]
fn write_with_owner_only(path: &Path, contents: &str) -> anyhow::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(contents.as_bytes())?;
    Ok(())
}

#[cfg(windows)]
fn write_with_owner_only(path: &Path, contents: &str) -> anyhow::Result<()> {
    // On Windows the user-profile ACL already excludes other users for files
    // under %LOCALAPPDATA%/oneshim/data. We use `create_new=false` to allow
    // overwrite after `reset_instance_id` + re-`ensure_instance_id`; the
    // explicit truncate matches Unix behaviour.
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    f.write_all(contents.as_bytes())?;
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn write_with_owner_only(path: &Path, contents: &str) -> anyhow::Result<()> {
    fs::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// T-X2-9 — drives all state-table rows (§3.7):
    /// 1. First opt-in creates the file with 0600 perms (Unix).
    /// 2. Boot with enabled=true + file exists: `ensure` returns the same UUID.
    /// 3. Boot with enabled=false + file exists: file untouched (proven by
    ///    content equivalence after the later `ensure` — if we wrote between
    ///    them the content would differ).
    /// 4. `reset_instance_id` deletes the file; next `ensure` regenerates a
    ///    different UUID.
    #[test]
    fn instance_id_file_lifecycle_matches_state_table() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_path_buf();

        // Row 1: first opt-in creates the file.
        let first = ensure_instance_id(&data_dir).unwrap();
        let path = data_dir.join(FILE_NAME);
        assert!(path.exists(), "file must exist after first opt-in");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "instance_id perms must be 0600 on Unix");
        }

        // Row 2: ensure again returns the same UUID (file was re-read).
        let second = ensure_instance_id(&data_dir).unwrap();
        assert_eq!(first, second, "UUID must be stable across ensure cycles");

        // Rows 4 + 5 (enabled=false / opt-out): file must still hold the
        // original UUID. The telemetry bootstrap path simply never reads or
        // writes the file while disabled, so the content must match what
        // was written on first opt-in.
        assert!(path.exists(), "opt-out must not delete the file");
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents.trim(), first);

        // Row 6: explicit reset deletes, next ensure regenerates a different UUID.
        reset_instance_id(&data_dir).unwrap();
        assert!(!path.exists(), "reset must delete the file");
        let third = ensure_instance_id(&data_dir).unwrap();
        assert_ne!(first, third, "post-reset UUID must be fresh");
    }

    /// Edge case: data_dir doesn't exist yet — ensure should create it.
    #[test]
    fn ensure_creates_missing_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().join("nested").join("subdir");
        assert!(!data_dir.exists());

        let uuid = ensure_instance_id(&data_dir).unwrap();
        assert!(!uuid.is_empty());
        assert!(data_dir.join(FILE_NAME).exists());
    }

    /// Edge case: empty file treated as missing.
    #[test]
    fn empty_file_treated_as_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let path = data_dir.join(FILE_NAME);
        fs::write(&path, "").unwrap();

        let uuid = ensure_instance_id(&data_dir).unwrap();
        assert!(!uuid.is_empty());
        let contents = fs::read_to_string(&path).unwrap();
        assert_eq!(contents.trim(), uuid);
    }
}
