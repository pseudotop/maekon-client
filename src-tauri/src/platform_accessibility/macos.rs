use maekon_core::error::CoreError;
use std::process::Command;

use super::command_timeout::{command_output_with_timeout, ACCESSIBILITY_COMMAND_TIMEOUT};
use super::types::{parse_accessibility_lines, AccessibilityNode};

#[cfg(target_os = "macos")]
pub(super) fn query_macos_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    let script = r#"
        tell application "System Events"
            set outText to ""
            set frontProc to first application process whose frontmost is true
            set appName to name of frontProc
            tell frontProc
                try
                    set frontWin to front window
                    set uiElems to every UI element of entire contents of frontWin
                    set idx to 0
                    repeat with e in uiElems
                        if idx is greater than 300 then exit repeat
                        set idx to idx + 1
                        try
                            set roleName to role of e as text
                            set labelText to ""
                            try
                                set labelText to name of e as text
                            end try
                            if labelText is "" then
                                try
                                    set labelText to description of e as text
                                end try
                            end if
                            if labelText is "" then
                                try
                                    set labelText to value of e as text
                                end try
                            end if
                            set pos to {0, 0}
                            set siz to {0, 0}
                            try
                                set pos to position of e
                            end try
                            try
                                set siz to size of e
                            end try
                            set w to item 1 of siz as integer
                            set h to item 2 of siz as integer
                            if w > 0 and h > 0 then
                                set x to item 1 of pos as integer
                                set y to item 2 of pos as integer
                                set normalizedLabel to labelText as text
                                set outText to outText & appName & "|||" & roleName & "|||" & normalizedLabel & "|||" & x & "|||" & y & "|||" & w & "|||" & h & linefeed
                            end if
                        end try
                    end repeat
                end try
            end tell
            return outText
        end tell
    "#;

    // SEC-MON-01: resolve against the shared trusted-directory allowlist
    // (maekon-monitor) instead of spawning a bare `osascript` — fail closed
    // (no PATH fallback) when the binary is not found under any trusted
    // system directory. This is the SAME class of spawn maekon-monitor
    // already hardens for its own osascript calls; src-tauri routes through
    // the identical resolver to avoid a second, divergent allowlist.
    let osascript_path = maekon_monitor::resolve_trusted_binary("osascript").ok_or_else(|| {
        CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "osascript not found under the trusted directory allowlist".to_string(),
        }
    })?;

    let mut command = Command::new(osascript_path);
    command.arg("-e").arg(script);
    let output = command_output_with_timeout(&mut command, ACCESSIBILITY_COMMAND_TIMEOUT)
        .map_err(|e| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: format!("macOS AX probe launch failed: {e}"),
        })?
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "macOS AX probe timed out".to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: format!(
                "macOS AX probe failed (check Accessibility permission): {}",
                stderr.trim()
            ),
        });
    }

    parse_accessibility_lines(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    // SEC-MON-01 regression: `maekon_monitor::resolve_trusted_binary` must be
    // reachable as `pub` from `src-tauri` (compile is part of the proof —
    // this crate has no `pub(crate)`/private re-export shim of its own) and
    // must resolve the SAME `osascript` this module spawns to an absolute
    // path under the shared trusted-directory allowlist, mirroring the
    // equivalent regression test in `maekon-monitor::macos`.
    #[test]
    fn osascript_resolves_to_absolute_path_via_shared_resolver() {
        let resolved = maekon_monitor::resolve_trusted_binary("osascript").unwrap_or_else(|| {
            panic!("osascript must resolve under the shared trusted-directory allowlist")
        });
        assert!(
            resolved.is_absolute(),
            "resolved osascript path must be absolute"
        );
        assert!(
            resolved.is_file(),
            "resolved osascript path must exist as a regular file"
        );
    }

    #[test]
    fn unknown_tool_name_fails_closed_via_shared_resolver() {
        // Fail-closed contract: a name absent from every trusted directory
        // must resolve to `None` — no PATH fallback — through the SAME
        // shared resolver src-tauri now routes through.
        assert_eq!(
            maekon_monitor::resolve_trusted_binary(
                "definitely-not-a-real-oneshim-trusted-binary-src-tauri-xyz123"
            ),
            None
        );
    }
}
