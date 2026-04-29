use oneshim_core::models::automation::AutomationAction;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxRequest {
    pub action: AutomationAction,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxResponse {
    pub success: bool,
    pub error: Option<String>,
}

/// Resolve the sandbox worker binary path.
/// Search order: exact name adjacent to binary, Tauri platform-suffixed, then PATH.
pub fn resolve_worker_path() -> Result<PathBuf, oneshim_core::error::CoreError> {
    let base_name = "oneshim-sandbox-worker";
    let ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 1. Exact name (dev builds: cargo puts binaries in target/debug/)
            let candidate = dir.join(format!("{base_name}{ext}"));
            if candidate.exists() {
                return Ok(candidate);
            }
            // 2. Tauri sidecar: platform-suffixed name
            let target = target_triple();
            let suffixed = dir.join(format!("{base_name}-{target}{ext}"));
            if suffixed.exists() {
                return Ok(suffixed);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    if let Ok(output) = std::process::Command::new("which")
        .arg("oneshim-sandbox-worker")
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
    }

    #[cfg(target_os = "windows")]
    if let Ok(output) = std::process::Command::new("where.exe")
        .arg("oneshim-sandbox-worker")
        .output()
    {
        if output.status.success() {
            if let Some(first_line) = String::from_utf8_lossy(&output.stdout).lines().next() {
                let path = first_line.trim().to_string();
                if !path.is_empty() {
                    return Ok(PathBuf::from(path));
                }
            }
        }
    }

    Err(oneshim_core::error::CoreError::SandboxExecution { code: oneshim_core::error_codes::SandboxCode::ExecutionFailed, message: "sandbox worker binary not found: checked adjacent to executable, Tauri sidecar, and PATH"
            .into() })
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
) -> Result<SandboxResponse, oneshim_core::error::CoreError> {
    let stdout_str = String::from_utf8_lossy(stdout);
    let trimmed = stdout_str.trim();
    if trimmed.is_empty() {
        return Err(oneshim_core::error::CoreError::SandboxExecution {
            code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
            message: "worker produced no output on stdout".into(),
        });
    }
    serde_json::from_str(trimmed).map_err(|e| oneshim_core::error::CoreError::SandboxExecution {
        code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
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
        assert!(parse_worker_response(b"").is_err());
    }

    #[test]
    fn parse_worker_response_malformed() {
        assert!(parse_worker_response(b"not json").is_err());
    }
}
