//! Debug CLI output helpers: path resolution, JSON emission, file I/O
//! (debug_assertions only).

use super::types::DebugNotificationBackend;

// ── Path resolution helpers ──────────────────────────────────────────────────

#[cfg(debug_assertions)]
pub(crate) fn debug_permissions_cli_output_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_output_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_ax_tree_cli_output_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_power_cli_output_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_marker_output_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_activation_output_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_audit_jsonl_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_cli_diagnostic_jsonl_path_from(
    env_value: Option<&str>,
) -> Option<std::path::PathBuf> {
    let value = env_value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(value))
    }
}

// ── JSON emission helpers ────────────────────────────────────────────────────

#[cfg(debug_assertions)]
pub(crate) fn debug_notification_audit_event_payload(
    command: &str,
    backend: DebugNotificationBackend,
    ok: bool,
    title: &str,
    body: &str,
    activation_route: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "event": format!("debug_notification.{command}"),
        "backend": backend.as_str(),
        "ok": ok,
        "title_present": !title.is_empty(),
        "title_len": title.len(),
        "body_present": !body.is_empty(),
        "body_len": body.len(),
        "activation_route": activation_route,
    })
}

#[cfg(debug_assertions)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn debug_macos_notification_category_identifier() -> &'static str {
    "maekon.debug.notification.activation"
}

#[cfg(debug_assertions)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn debug_macos_notification_open_action_identifier() -> &'static str {
    "maekon.debug.notification.open"
}

#[cfg(debug_assertions)]
pub(crate) fn emit_debug_permissions_cli_json(payload: &str) -> i32 {
    println!("{payload}");

    let output_path = debug_permissions_cli_output_path_from(
        std::env::var("MAEKON_DEBUG_PERMISSION_CLI_OUTPUT")
            .ok()
            .as_deref(),
    );
    let Some(output_path) = output_path else {
        return 0;
    };

    if let Some(parent) = output_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "debug-permissions output directory create failed at {}: {error}",
                parent.display()
            );
            return 1;
        }
    }

    if let Err(error) = std::fs::write(&output_path, format!("{payload}\n")) {
        eprintln!(
            "debug-permissions output write failed at {}: {error}",
            output_path.display()
        );
        return 1;
    }

    0
}

#[cfg(debug_assertions)]
pub(crate) fn emit_debug_ax_tree_cli_json(payload: &str) -> i32 {
    println!("{payload}");

    let output_path = debug_ax_tree_cli_output_path_from(
        std::env::var("MAEKON_DEBUG_AX_TREE_CLI_OUTPUT")
            .ok()
            .as_deref(),
    );
    let Some(output_path) = output_path else {
        return 0;
    };

    if let Some(parent) = output_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "debug-ax-tree output directory create failed at {}: {error}",
                parent.display()
            );
            return 1;
        }
    }

    if let Err(error) = std::fs::write(&output_path, format!("{payload}\n")) {
        eprintln!(
            "debug-ax-tree output write failed at {}: {error}",
            output_path.display()
        );
        return 1;
    }

    0
}

#[cfg(debug_assertions)]
pub(crate) fn emit_debug_notification_cli_json(payload: &str) -> i32 {
    println!("{payload}");

    let output_path = debug_notification_cli_output_path_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_CLI_OUTPUT")
            .ok()
            .as_deref(),
    );
    let Some(output_path) = output_path else {
        return 0;
    };

    if let Some(parent) = output_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "debug-notification output directory create failed at {}: {error}",
                output_path.display()
            );
            return 1;
        }
    }

    if let Err(error) = std::fs::write(&output_path, format!("{payload}\n")) {
        eprintln!(
            "debug-notification output write failed at {}: {error}",
            output_path.display()
        );
        return 1;
    }

    0
}

#[cfg(debug_assertions)]
pub(crate) fn emit_debug_power_cli_json(payload: &str) -> i32 {
    println!("{payload}");

    let output_path = debug_power_cli_output_path_from(
        std::env::var("MAEKON_DEBUG_POWER_CLI_OUTPUT")
            .ok()
            .as_deref(),
    );
    let Some(output_path) = output_path else {
        return 0;
    };

    if let Some(parent) = output_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "debug-power output directory create failed at {}: {error}",
                output_path.display()
            );
            return 1;
        }
    }

    if let Err(error) = std::fs::write(&output_path, format!("{payload}\n")) {
        eprintln!(
            "debug-power output write failed at {}: {error}",
            output_path.display()
        );
        return 1;
    }

    0
}

#[cfg(debug_assertions)]
pub(crate) fn append_debug_notification_audit_jsonl(payload: &serde_json::Value) -> i32 {
    let output_path = debug_notification_cli_audit_jsonl_path_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_AUDIT_JSONL")
            .ok()
            .as_deref(),
    );
    let Some(output_path) = output_path else {
        return 0;
    };

    if let Some(parent) = output_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "debug-notification audit directory create failed at {}: {error}",
                parent.display()
            );
            return 1;
        }
    }

    let line = match serde_json::to_string(payload) {
        Ok(line) => line,
        Err(error) => {
            eprintln!("debug-notification audit serialization failed: {error}");
            return 1;
        }
    };
    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&output_path)
    {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "debug-notification audit open failed at {}: {error}",
                output_path.display()
            );
            return 1;
        }
    };
    if let Err(error) = std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()) {
        eprintln!(
            "debug-notification audit write failed at {}: {error}",
            output_path.display()
        );
        return 1;
    }

    0
}

#[cfg(debug_assertions)]
pub(crate) fn emit_debug_notification_cli_marker_json(payload: &str) -> i32 {
    let output_path = debug_notification_cli_marker_output_path_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_CLI_MARKER_OUTPUT")
            .ok()
            .as_deref(),
    );
    let Some(output_path) = output_path else {
        return 0;
    };

    if let Some(parent) = output_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "debug-notification marker directory create failed at {}: {error}",
                parent.display()
            );
            return 1;
        }
    }

    if let Err(error) = std::fs::write(&output_path, format!("{payload}\n")) {
        eprintln!(
            "debug-notification marker write failed at {}: {error}",
            output_path.display()
        );
        return 1;
    }

    0
}

#[cfg(debug_assertions)]
pub(crate) fn hold_debug_permissions_cli_if_requested() {
    let hold_seconds = super::parsers::debug_permissions_cli_hold_seconds_from(
        std::env::var("MAEKON_DEBUG_DESKTOP_PERMISSION_CLI_HOLD_SECONDS")
            .ok()
            .as_deref(),
    );
    if hold_seconds > 0 {
        std::thread::sleep(std::time::Duration::from_secs(hold_seconds));
    }
}

#[cfg(debug_assertions)]
pub(crate) fn hold_debug_notification_cli_if_requested() {
    let hold_seconds = super::parsers::debug_notification_cli_hold_seconds_from(
        std::env::var("MAEKON_DEBUG_NOTIFICATION_CLI_HOLD_SECONDS")
            .ok()
            .as_deref(),
    );
    if hold_seconds > 0 {
        std::thread::sleep(std::time::Duration::from_secs(hold_seconds));
    }
}
