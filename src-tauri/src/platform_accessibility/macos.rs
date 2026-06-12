use maekon_core::error::CoreError;
use std::process::Command;

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

    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: format!("macOS AX probe launch failed: {e}"),
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
