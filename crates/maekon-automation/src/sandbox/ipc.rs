use maekon_core::models::automation::AutomationAction;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const WORKER_BASE_NAME: &str = "maekon-sandbox-worker";

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxRequest {
    pub action: AutomationAction,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxResponse {
    pub success: bool,
    pub error: Option<String>,
}

/// Resolve the sandbox worker binary path, searching only within the directory
/// adjacent to the executable.
pub fn resolve_worker_path() -> Result<PathBuf, maekon_core::error::CoreError> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(worker_path) = resolve_worker_path_in_dir(dir) {
                return Ok(worker_path);
            }

            // cargo-test layout only: the test executable lives in
            // `target/debug/deps/`, while `cargo build -p maekon-sandbox-worker`
            // places the worker one level up in `target/debug/`. Production
            // builds keep the strict adjacent-only lookup above.
            #[cfg(test)]
            if let Some(parent) = dir.parent() {
                if let Some(worker_path) = resolve_worker_path_in_dir(parent) {
                    return Ok(worker_path);
                }
            }
        }
    }

    Err(worker_not_found_error())
}

fn resolve_worker_path_in_dir(executable_dir: &Path) -> Option<PathBuf> {
    worker_path_candidates(executable_dir)
        .into_iter()
        .find(|candidate| worker_candidate_is_allowed(executable_dir, candidate))
}

fn worker_path_candidates(executable_dir: &Path) -> Vec<PathBuf> {
    let ext = worker_extension();
    let target = target_triple();
    vec![
        executable_dir.join(format!("{WORKER_BASE_NAME}{ext}")),
        executable_dir.join(format!("{WORKER_BASE_NAME}-{target}{ext}")),
    ]
}

fn worker_extension() -> &'static str {
    if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    }
}

fn worker_candidate_is_allowed(executable_dir: &Path, candidate: &Path) -> bool {
    if !candidate.is_file() {
        return false;
    }

    match (executable_dir.canonicalize(), candidate.canonicalize()) {
        (Ok(root), Ok(resolved_candidate)) => resolved_candidate.starts_with(root),
        _ => candidate.starts_with(executable_dir),
    }
}

fn worker_not_found_error() -> maekon_core::error::CoreError {
    maekon_core::error::CoreError::SandboxExecution {
        code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
        message:
            "sandbox worker binary not found: checked adjacent to executable and Tauri sidecar"
                .into(),
    }
}

/// Returns the Rust target triple for the current platform.
fn target_triple() -> &'static str {
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    {
        "aarch64-apple-darwin"
    }
    #[cfg(all(target_arch = "x86_64", target_os = "windows"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_arch = "aarch64", target_os = "windows"))]
    {
        "aarch64-pc-windows-msvc"
    }
}

/// Parse worker stdout into SandboxResponse.
pub fn parse_worker_response(
    stdout: &[u8],
) -> Result<SandboxResponse, maekon_core::error::CoreError> {
    let stdout_str = String::from_utf8_lossy(stdout);
    let trimmed = stdout_str.trim();
    if trimmed.is_empty() {
        return Err(maekon_core::error::CoreError::SandboxExecution {
            code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
            message: "worker produced no output on stdout".into(),
        });
    }
    serde_json::from_str(trimmed).map_err(|e| maekon_core::error::CoreError::SandboxExecution {
        code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
        message: format!("failed to parse worker response: {e} -- stdout: {trimmed}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_serde_roundtrip() {
        let req = SandboxRequest {
            action: AutomationAction::KeyType {
                text: "hello".into(),
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: SandboxRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{:?}", parsed.action), format!("{:?}", req.action));
    }

    #[test]
    fn response_success_roundtrip() {
        let resp = SandboxResponse {
            success: true,
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SandboxResponse = serde_json::from_str(&json).unwrap();
        assert!(parsed.success);
        assert!(parsed.error.is_none());
    }

    #[test]
    fn response_failure_roundtrip() {
        let resp = SandboxResponse {
            success: false,
            error: Some("denied".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: SandboxResponse = serde_json::from_str(&json).unwrap();
        assert!(!parsed.success);
        assert_eq!(parsed.error.as_deref(), Some("denied"));
    }

    #[test]
    fn parse_worker_response_valid() {
        let resp = parse_worker_response(br#"{"success":true,"error":null}"#).unwrap();
        assert!(resp.success);
    }

    #[test]
    fn parse_worker_response_empty_stdout() {
        let err = parse_worker_response(b"").unwrap_err();
        assert!(
            matches!(err, maekon_core::error::CoreError::SandboxExecution { .. }),
            "empty stdout must produce SandboxExecution, got: {err:?}"
        );
    }

    #[test]
    fn parse_worker_response_malformed() {
        let err = parse_worker_response(b"not json").unwrap_err();
        assert!(
            matches!(err, maekon_core::error::CoreError::SandboxExecution { .. }),
            "malformed stdout must produce SandboxExecution, got: {err:?}"
        );
    }

    #[test]
    fn worker_path_resolution_does_not_use_path_lookup() {
        let source = include_str!("ipc.rs");
        let unix_path_lookup = format!("Command::new({:?})", "which");
        let windows_path_lookup = format!("Command::new({:?})", "where.exe");
        assert!(!source.contains(&unix_path_lookup));
        assert!(!source.contains(&windows_path_lookup));
    }

    #[test]
    fn worker_path_resolution_accepts_adjacent_worker_only() {
        let dir = temp_worker_test_dir("adjacent");
        let worker_path = dir.join(worker_file_name());
        std::fs::write(&worker_path, b"").unwrap();

        let resolved = resolve_worker_path_in_dir(&dir).unwrap();

        assert_eq!(resolved, worker_path);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn worker_candidate_rejects_path_outside_executable_dir() {
        let executable_dir = temp_worker_test_dir("root");
        let path_dir = temp_worker_test_dir("path");
        let path_worker = path_dir.join(worker_file_name());
        std::fs::write(&path_worker, b"").unwrap();

        assert!(!worker_candidate_is_allowed(&executable_dir, &path_worker));

        std::fs::remove_dir_all(&executable_dir).ok();
        std::fs::remove_dir_all(&path_dir).ok();
    }

    fn temp_worker_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "maekon-automation-ipc-{name}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn worker_file_name() -> String {
        format!("{}{}", WORKER_BASE_NAME, worker_extension())
    }

    // #9573 (CRT-PRV-SBX-006): the catalog named a filter that never matched
    // any test. Adjacent-worker acceptance was already pinned under a
    // different name (#4059, `worker_path_resolution_accepts_adjacent_worker_
    // only`); genuinely new here: the target-triple SIDECAR candidate and the
    // empty-dir behavioral assertion (the directory-parameterized API itself
    // is the no-PATH-fallback guarantee).
    #[test]
    fn resolve_worker_path_in_dir_accepts_target_triple_sidecar() {
        let dir = tempfile::tempdir().expect("temp dir");
        let sidecar = dir.path().join(format!(
            "{}-{}{}",
            WORKER_BASE_NAME,
            target_triple(),
            worker_extension()
        ));
        std::fs::write(&sidecar, b"sidecar-binary").expect("stage sidecar");

        let resolved = resolve_worker_path_in_dir(dir.path())
            .expect("Tauri target-triple sidecar must be accepted");
        assert_eq!(resolved, sidecar);
    }

    #[test]
    fn resolve_worker_path_in_dir_resolves_nothing_from_empty_dir() {
        // Candidates are built ONLY from the given directory, so an empty dir
        // must yield None — no PATH or environment lookup can intervene.
        // (Out-of-dir REJECTION is pinned by the pre-existing
        // `worker_candidate_rejects_path_outside_executable_dir`.)
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(
            resolve_worker_path_in_dir(dir.path()).is_none(),
            "empty executable dir must resolve nothing (no PATH fallback)"
        );
    }
}
