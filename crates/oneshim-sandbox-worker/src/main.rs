//! Sandboxed automation action executor.
//!
//! Spawned by the parent process with platform sandbox constraints already
//! applied. Reads SandboxRequest JSON from stdin, runs the action, writes
//! SandboxResponse JSON to stdout.

use oneshim_core::models::automation::AutomationAction;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
struct SandboxRequest {
    action: AutomationAction,
}

#[derive(Serialize)]
struct SandboxResponse {
    success: bool,
    error: Option<String>,
}

fn main() {
    let response = match run() {
        Ok(()) => SandboxResponse {
            success: true,
            error: None,
        },
        Err(e) => SandboxResponse {
            success: false,
            error: Some(e),
        },
    };

    if let Ok(json) = serde_json::to_string(&response) {
        let _ = io::stdout().write_all(json.as_bytes());
        let _ = io::stdout().write_all(b"\n");
        let _ = io::stdout().flush();
    }
}

fn run() -> Result<(), String> {
    let stdin = io::stdin();
    let line = stdin
        .lock()
        .lines()
        .next()
        .ok_or_else(|| "no input on stdin".to_string())?
        .map_err(|e| format!("stdin read error: {e}"))?;

    let request: SandboxRequest =
        serde_json::from_str(&line).map_err(|e| format!("invalid request JSON: {e}"))?;

    run_action(&request.action)
}

fn run_action(action: &AutomationAction) -> Result<(), String> {
    match action {
        AutomationAction::MouseMove { x, y } => {
            eprintln!("sandbox-worker: mouse move ({x}, {y})");
        }
        AutomationAction::MouseClick { button, x, y } => {
            eprintln!("sandbox-worker: mouse click {button} ({x}, {y})");
        }
        AutomationAction::KeyType { text } => {
            eprintln!("sandbox-worker: key type len={}", text.len());
        }
        AutomationAction::KeyPress { key } => {
            eprintln!("sandbox-worker: key press {key}");
        }
        AutomationAction::KeyRelease { key } => {
            eprintln!("sandbox-worker: key release {key}");
        }
        AutomationAction::Hotkey { keys } => {
            eprintln!("sandbox-worker: hotkey {:?}", keys);
        }
    }
    Ok(())
}
