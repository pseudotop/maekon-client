//! Cfg-free reporting for the Windows restricted-token launch probe (#10959).
//!
//! The Win32 launch itself lives in `super::windows` (target-gated FFI), but
//! deciding *what to report* and writing it is plain string + file work.
//! Extracting it here (no `#[cfg]`, no FFI) makes it unit-testable on any OS,
//! mirroring `super::win_limits` (#5138) and `super::sbpl` (#5120).
//!
//! Why this exists at all: the #10288 skip signals via `eprintln!`, and libtest
//! captures a *passing* test's output unless `--nocapture` is passed — which
//! public CI does not pass. A skipped probe and a genuinely passing one both
//! print `... ok`, so a green Windows column cannot distinguish "the regression
//! is gone" from "the failure was swallowed". `$GITHUB_STEP_SUMMARY` is a file
//! the runner reads after the step, so writing there escapes that capture.
//!
//! Both outcomes are recorded, not just the skip. A report that only appears on
//! failure leaves silence ambiguous — it could mean the probe passed, or that
//! the reporting itself broke.

// Written by the Windows-only probe in `super::windows`; the unit tests below
// are the only callers on other targets.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// Which branch the restricted-token launch probe took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeVerdict {
    /// The child ran and exited cleanly — the #10288 regression is not present
    /// on this image.
    Passed,
    /// The child died at DLL initialization with `STATUS_DLL_INIT_FAILED`, and
    /// the #10288 release-unblock skip was taken.
    SkippedDllInitFailed,
}

impl fmt::Display for ProbeVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Passed => write!(f, "PASSED"),
            Self::SkippedDllInitFailed => write!(f, "SKIPPED (STATUS_DLL_INIT_FAILED)"),
        }
    }
}

/// Runner image identity, so a verdict can be attributed to a specific build.
///
/// `ImageOS`/`ImageVersion` are set by GitHub-hosted runners. Missing values
/// become `unknown` rather than dropping the field, because a verdict with no
/// image attached is nearly useless for this issue — the whole question is
/// which images are affected.
pub fn describe_image(image_os: Option<&str>, image_version: Option<&str>) -> String {
    format!(
        "{}/{}",
        image_os.filter(|v| !v.is_empty()).unwrap_or("unknown"),
        image_version.filter(|v| !v.is_empty()).unwrap_or("unknown"),
    )
}

/// One-line Markdown row for the job summary.
///
/// Kept to a single line so repeated runs append into a readable list rather
/// than a wall of headings.
pub fn format_probe_verdict(verdict: ProbeVerdict, image: &str) -> String {
    let note = match verdict {
        ProbeVerdict::Passed => "restricted-token launch probe ran to completion",
        ProbeVerdict::SkippedDllInitFailed => {
            "restricted-token launch probe skipped — #10288 regression still present"
        }
    };
    format!("- **{verdict}** on `{image}` — {note}")
}

/// Append a line to the GitHub step summary file.
///
/// Appends rather than truncates: other steps in the same job write here too,
/// and clobbering their content to report a test verdict would trade one blind
/// spot for another.
pub fn append_step_summary(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
}

/// Record the probe's verdict where CI can see it.
///
/// A no-op when `$GITHUB_STEP_SUMMARY` is unset, which is the normal local
/// case. Write failures are deliberately swallowed: this is reporting, and
/// failing a security probe because a summary file was read-only would be a
/// worse outcome than the missing line. The caller still prints to stderr for
/// anyone running with `--nocapture`.
pub fn record_probe_verdict(verdict: ProbeVerdict) {
    let Some(summary_path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return;
    };
    let image = describe_image(
        std::env::var("ImageOS").ok().as_deref(),
        std::env::var("ImageVersion").ok().as_deref(),
    );
    let _ = append_step_summary(
        Path::new(&summary_path),
        &format_probe_verdict(verdict, &image),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_verdict_names_the_issue_so_a_green_column_is_not_read_as_clean() {
        let line = format_probe_verdict(ProbeVerdict::SkippedDllInitFailed, "win25/20260810.198");
        assert!(
            line.contains("SKIPPED"),
            "a skipped probe must say so: {line}"
        );
        assert!(
            line.contains("#10288"),
            "the skip line must point at the tracking issue: {line}"
        );
        assert!(
            line.contains("win25/20260810.198"),
            "the verdict must name the image it applies to: {line}"
        );
    }

    #[test]
    fn pass_verdict_is_reported_too_so_silence_is_never_ambiguous() {
        let line = format_probe_verdict(ProbeVerdict::Passed, "win25/20260810.198");
        assert!(
            line.contains("PASSED"),
            "a passing probe must say so: {line}"
        );
        assert!(
            !line.contains("SKIPPED"),
            "a passing probe must not read as skipped: {line}"
        );
        assert!(
            line.contains("win25/20260810.198"),
            "the verdict must name the image it applies to: {line}"
        );
    }

    #[test]
    fn the_two_verdicts_are_distinguishable() {
        // The whole point: `... ok` is identical for both, so the recorded
        // lines must not be.
        let passed = format_probe_verdict(ProbeVerdict::Passed, "img");
        let skipped = format_probe_verdict(ProbeVerdict::SkippedDllInitFailed, "img");
        assert_ne!(passed, skipped);
    }

    #[test]
    fn missing_image_env_degrades_to_unknown_rather_than_dropping_the_field() {
        assert_eq!(describe_image(None, None), "unknown/unknown");
        assert_eq!(describe_image(Some(""), Some("")), "unknown/unknown");
        assert_eq!(describe_image(Some("win25"), None), "win25/unknown");
    }

    #[test]
    fn append_preserves_existing_summary_content() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("summary.md");
        std::fs::write(&path, "earlier step output\n").expect("seed summary");

        append_step_summary(&path, "- **PASSED** on `img` — x").expect("append");

        let contents = std::fs::read_to_string(&path).expect("read back");
        assert!(
            contents.starts_with("earlier step output\n"),
            "another step's summary content must survive: {contents:?}"
        );
        assert!(
            contents.contains("**PASSED**"),
            "the verdict must be appended: {contents:?}"
        );
    }

    #[test]
    fn append_creates_the_file_when_the_runner_has_not_yet() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("not-created-yet.md");

        append_step_summary(&path, "- **SKIPPED (STATUS_DLL_INIT_FAILED)** on `img` — y")
            .expect("append to a missing summary file");

        let contents = std::fs::read_to_string(&path).expect("read back");
        assert!(contents.contains("SKIPPED"), "got: {contents:?}");
    }

    // `record_probe_verdict` is the function `super::windows` actually calls, so
    // testing only its parts would leave the env wiring — the part that decides
    // whether anything is written at all — unverified. These tests mutate
    // process env, so they take a lock to avoid interleaving with each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_probe_env<T>(
        summary: Option<&Path>,
        image: Option<(&str, &str)>,
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        match summary {
            Some(path) => std::env::set_var("GITHUB_STEP_SUMMARY", path),
            None => std::env::remove_var("GITHUB_STEP_SUMMARY"),
        }
        match image {
            Some((os, version)) => {
                std::env::set_var("ImageOS", os);
                std::env::set_var("ImageVersion", version);
            }
            None => {
                std::env::remove_var("ImageOS");
                std::env::remove_var("ImageVersion");
            }
        }
        let result = f();
        std::env::remove_var("GITHUB_STEP_SUMMARY");
        std::env::remove_var("ImageOS");
        std::env::remove_var("ImageVersion");
        result
    }

    #[test]
    fn recording_a_skip_reaches_the_summary_file_that_ci_reads() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("summary.md");

        with_probe_env(Some(&path), Some(("win25", "20260810.198")), || {
            record_probe_verdict(ProbeVerdict::SkippedDllInitFailed);
        });

        let contents = std::fs::read_to_string(&path).expect("summary must have been written");
        assert!(contents.contains("SKIPPED"), "got: {contents:?}");
        assert!(contents.contains("#10288"), "got: {contents:?}");
        assert!(
            contents.contains("win25/20260810.198"),
            "the image must come from the runner env: {contents:?}"
        );
    }

    #[test]
    fn recording_a_pass_reaches_the_same_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("summary.md");

        with_probe_env(Some(&path), Some(("win25", "20260810.198")), || {
            record_probe_verdict(ProbeVerdict::Passed);
        });

        let contents = std::fs::read_to_string(&path).expect("summary must have been written");
        assert!(contents.contains("PASSED"), "got: {contents:?}");
        assert!(!contents.contains("SKIPPED"), "got: {contents:?}");
    }

    #[test]
    fn recording_off_ci_writes_nothing_and_does_not_panic() {
        // Local `cargo test` has no GITHUB_STEP_SUMMARY. The probe must stay
        // usable there rather than failing on a missing runner variable.
        with_probe_env(None, None, || {
            record_probe_verdict(ProbeVerdict::Passed);
        });
    }

    #[test]
    fn an_unwritable_summary_path_does_not_fail_the_probe() {
        // Reporting must never be able to turn a passing security probe red.
        let dir = tempfile::tempdir().expect("temp dir");
        let unwritable = dir.path().join("no-such-dir").join("summary.md");

        with_probe_env(Some(&unwritable), None, || {
            record_probe_verdict(ProbeVerdict::SkippedDllInitFailed);
        });

        assert!(!unwritable.exists(), "nothing should have been created");
    }
}
