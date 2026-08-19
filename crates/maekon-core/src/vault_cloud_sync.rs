//! ADR-033 §3.2: best-effort cloud-sync detection for a vault custom path.
//!
//! Detection's role is deliberately NARROW (§3.2): it enriches the §3.3
//! acknowledgement copy with a named provider and it gates the §3.4
//! egress-ledger record. It gates NO consent decision by itself — every custom
//! path requires the acknowledgement whether or not a provider is detected, so
//! a miss here degrades the warning's specificity, never its presence.
//!
//! Runs ONCE, at path-acceptance time, against the canonicalized target; the
//! result is stored in `analysis.memory_vault.cloud_provider` and that stored
//! value — not live re-detection — is the per-cycle truth.
//!
//! # Honesty bound (§3.5)
//! A user can symlink, mount, or run an arbitrary sync daemon over any folder.
//! This table defends nothing; it only names what it recognizes. Over-warning
//! is the safe direction, so the home-relative table is matched on EVERY host
//! rather than under `#[cfg(target_os)]`: a Linux user whose `~/Dropbox` is a
//! real Dropbox folder gets the named warning, and the only cost of a
//! cross-platform match is a more specific warning than strictly required.
//! Known Follow-up 2 of ADR-033 owns this table's drift.

use std::path::{Path, PathBuf};

/// iCloud Drive (`~/Library/Mobile Documents`).
pub const CLOUD_PROVIDER_ICLOUD: &str = "icloud";
/// The macOS `~/Library/CloudStorage` mount point shared by Dropbox/Google
/// Drive/OneDrive/Box provider folders — the provider is not distinguishable
/// from the path alone, so the coarse mount label is what gets recorded.
pub const CLOUD_PROVIDER_CLOUD_STORAGE: &str = "cloud_storage";
/// OneDrive (personal or commercial).
pub const CLOUD_PROVIDER_ONEDRIVE: &str = "onedrive";
/// Dropbox.
pub const CLOUD_PROVIDER_DROPBOX: &str = "dropbox";
/// Google Drive.
pub const CLOUD_PROVIDER_GOOGLE_DRIVE: &str = "google_drive";

/// ADR-033 §3.4: the closed set of coarse `destination` labels permitted in the
/// erase-retained, deliberately-no-PII egress ledger.
///
/// This is the SSOT for both producers: the detector may only mint a label from
/// this set, and the writer allowlists against this same array at the point of
/// ledger use (`cloud_provider` is a hand-editable free-string config field).
/// A label the detector could mint but the writer would drop is a silent
/// unledgered-egress gap, which sharing one array makes impossible.
pub const CLOUD_PROVIDER_LABELS: [&str; 5] = [
    CLOUD_PROVIDER_ICLOUD,
    CLOUD_PROVIDER_CLOUD_STORAGE,
    CLOUD_PROVIDER_ONEDRIVE,
    CLOUD_PROVIDER_DROPBOX,
    CLOUD_PROVIDER_GOOGLE_DRIVE,
];

/// Home-relative provider roots (§3.2), most specific first.
///
/// Matching is component-wise (`Path::starts_with`), so `~/Dropbox-backup`
/// does NOT match `~/Dropbox`.
const HOME_RELATIVE_ROOTS: &[(&str, &str)] = &[
    // macOS
    ("Library/Mobile Documents", CLOUD_PROVIDER_ICLOUD),
    ("Library/CloudStorage", CLOUD_PROVIDER_CLOUD_STORAGE),
    // Windows / Linux / macOS legacy provider folders
    ("OneDrive", CLOUD_PROVIDER_ONEDRIVE),
    ("Dropbox", CLOUD_PROVIDER_DROPBOX),
    ("Google Drive", CLOUD_PROVIDER_GOOGLE_DRIVE),
];

/// Environment variables naming an absolute OneDrive root (Windows).
const ONEDRIVE_ENV_VARS: &[&str] = &["OneDrive", "OneDriveCommercial", "OneDriveConsumer"];

/// Pure detector: `target` against explicit absolute roots, then the
/// home-relative table.
///
/// `env_roots` is checked FIRST because a `%OneDrive%` redirected to a
/// non-default location is the case the home-relative table cannot see.
/// Returns `None` when nothing is recognized — which per §3.3 still requires
/// the acknowledgement, with the generic "any sync tool you run" copy.
pub fn detect_cloud_provider_with(
    target: &Path,
    home: Option<&Path>,
    env_roots: &[(PathBuf, &'static str)],
) -> Option<&'static str> {
    for (root, label) in env_roots {
        // An empty or relative env value would match every target via
        // `starts_with("")`; require an absolute root before trusting it.
        if root.is_absolute() && target.starts_with(root) {
            return Some(label);
        }
    }

    let home = home?;
    for (relative, label) in HOME_RELATIVE_ROOTS {
        if target.starts_with(home.join(relative)) {
            return Some(label);
        }
    }

    None
}

/// Host wrapper: resolves the home directory and the OneDrive env roots, then
/// delegates to [`detect_cloud_provider_with`].
///
/// `target` SHOULD already be canonicalized (§3.2) — a symlinked path that has
/// not been resolved defeats the table by construction.
pub fn detect_cloud_provider(target: &Path) -> Option<&'static str> {
    let env_roots: Vec<(PathBuf, &'static str)> = ONEDRIVE_ENV_VARS
        .iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(|value| (PathBuf::from(value), CLOUD_PROVIDER_ONEDRIVE))
        .collect();

    detect_cloud_provider_with(target, home_dir().as_deref(), &env_roots)
}

/// Home directory, using the same environment variables `path_resolution`
/// relies on (the repo does not depend on a `dirs`-style crate).
fn home_dir() -> Option<PathBuf> {
    for var in ["HOME", "USERPROFILE"] {
        if let Some(value) = std::env::var_os(var) {
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/tester")
    }

    #[test]
    fn icloud_drive_path_is_detected() {
        let target = home().join("Library/Mobile Documents/com~apple~CloudDocs/vault");
        assert_eq!(
            detect_cloud_provider_with(&target, Some(&home()), &[]),
            Some(CLOUD_PROVIDER_ICLOUD)
        );
    }

    #[test]
    fn cloud_storage_mount_is_detected_as_the_coarse_mount_label() {
        // The provider under ~/Library/CloudStorage is not distinguishable from
        // the path alone, so the mount label — not a guessed provider — is what
        // reaches the ledger.
        let target = home().join("Library/CloudStorage/Dropbox/notes/vault");
        assert_eq!(
            detect_cloud_provider_with(&target, Some(&home()), &[]),
            Some(CLOUD_PROVIDER_CLOUD_STORAGE)
        );
    }

    #[test]
    fn dropbox_and_google_drive_home_roots_are_detected() {
        assert_eq!(
            detect_cloud_provider_with(&home().join("Dropbox/vault"), Some(&home()), &[]),
            Some(CLOUD_PROVIDER_DROPBOX)
        );
        assert_eq!(
            detect_cloud_provider_with(&home().join("Google Drive/vault"), Some(&home()), &[]),
            Some(CLOUD_PROVIDER_GOOGLE_DRIVE)
        );
    }

    #[test]
    fn onedrive_env_root_outside_home_is_detected() {
        // %OneDrive% redirected to another volume — the case the home-relative
        // table structurally cannot see.
        let root = if cfg!(windows) {
            PathBuf::from(r"D:\work\OneDrive - Contoso")
        } else {
            PathBuf::from("/mnt/work/OneDrive - Contoso")
        };
        let target = root.join("vault");
        let env_roots = vec![(root, CLOUD_PROVIDER_ONEDRIVE)];
        assert_eq!(
            detect_cloud_provider_with(&target, Some(&home()), &env_roots),
            Some(CLOUD_PROVIDER_ONEDRIVE)
        );
    }

    #[test]
    fn relative_env_root_is_ignored_rather_than_matching_everything() {
        // A relative (or empty) env value would `starts_with`-match every
        // target and label every path as OneDrive.
        let target = PathBuf::from("/srv/data/vault");
        let env_roots = vec![(PathBuf::from("OneDrive"), CLOUD_PROVIDER_ONEDRIVE)];
        assert_eq!(
            detect_cloud_provider_with(&target, Some(&home()), &env_roots),
            None
        );
    }

    #[test]
    fn sibling_directory_sharing_a_name_prefix_is_not_detected() {
        // Component-wise matching: ~/Dropbox-backup is the user's own folder.
        let target = home().join("Dropbox-backup/vault");
        assert_eq!(
            detect_cloud_provider_with(&target, Some(&home()), &[]),
            None
        );
    }

    #[test]
    fn plain_local_path_is_not_detected() {
        let target = home().join("Documents/maekon-vault");
        assert_eq!(
            detect_cloud_provider_with(&target, Some(&home()), &[]),
            None
        );
    }

    #[test]
    fn unresolvable_home_yields_no_detection_rather_than_a_panic() {
        // §3.5: a miss degrades warning specificity; §3.3 still demands the
        // acknowledgement, so "unknown" is a safe answer.
        let target = PathBuf::from("/srv/vault");
        assert_eq!(detect_cloud_provider_with(&target, None, &[]), None);
    }

    #[test]
    fn every_detectable_label_is_ledger_allowlisted() {
        // The §3.4 gap this SSOT exists to prevent: a label the detector can
        // store but the writer would drop is silent unledgered egress.
        for (_, label) in HOME_RELATIVE_ROOTS {
            assert!(
                CLOUD_PROVIDER_LABELS.contains(label),
                "detectable label {label} is not in the ledger allowlist"
            );
        }
        assert!(CLOUD_PROVIDER_LABELS.contains(&CLOUD_PROVIDER_ONEDRIVE));
    }
}
