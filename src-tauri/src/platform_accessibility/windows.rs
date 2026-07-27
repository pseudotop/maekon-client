use maekon_core::error::CoreError;
use std::os::windows::process::CommandExt;
use std::process::Command;

use super::command_timeout::{command_output_with_timeout, ACCESSIBILITY_COMMAND_TIMEOUT};
use super::types::{parse_accessibility_lines, AccessibilityNode};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(target_os = "windows")]
pub(super) fn query_windows_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    let script = r#"
        Add-Type -AssemblyName UIAutomationClient
        Add-Type -AssemblyName UIAutomationTypes

        $focused = [System.Windows.Automation.AutomationElement]::FocusedElement
        if ($null -eq $focused) {
            return
        }

        $walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
        $window = $focused
        while ($window -ne $null -and $window.Current.ControlType.ProgrammaticName -ne 'ControlType.Window') {
            $window = $walker.GetParent($window)
        }
        if ($window -eq $null) {
            $window = $focused
        }

        $procName = ''
        try {
            $proc = Get-Process -Id $window.Current.ProcessId -ErrorAction Stop
            $procName = $proc.ProcessName
        } catch {
            $procName = ''
        }

        # Include the traversal root. Some WebView/native surfaces expose the
        # focused surface itself (for example, a Pane) but no descendants.
        $all = $window.FindAll([System.Windows.Automation.TreeScope]::Subtree, [System.Windows.Automation.Condition]::TrueCondition)
        $limit = [Math]::Min(300, $all.Count)
        for ($i = 0; $i -lt $limit; $i++) {
            $node = $all.Item($i)
            $label = $node.Current.Name
            if ([string]::IsNullOrWhiteSpace($label)) {
                $label = $node.Current.AutomationId
            }

            $role = $node.Current.ControlType.ProgrammaticName
            $rect = $node.Current.BoundingRectangle
            if ($rect.Width -le 0 -or $rect.Height -le 0) {
                continue
            }

            Write-Output "$procName|||$role|||$label|||$([int]$rect.X)|||$([int]$rect.Y)|||$([int]$rect.Width)|||$([int]$rect.Height)"
        }
    "#;

    // SEC-MON-01: resolve against the shared trusted-directory allowlist
    // (maekon-monitor) instead of spawning a bare `powershell` — fail closed
    // (no PATH fallback) when the binary is not found under the trusted
    // System32 directories.
    let powershell_path =
        maekon_monitor::resolve_trusted_binary("powershell").ok_or_else(|| {
            CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "powershell not found under the trusted directory allowlist".to_string(),
            }
        })?;

    let mut command = Command::new(powershell_path);
    // A visible console can steal focus before `FocusedElement` is read,
    // causing the probe to inspect its own PowerShell window instead of the
    // user's foreground application.
    command.creation_flags(CREATE_NO_WINDOW);
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        script,
    ]);
    let output = command_output_with_timeout(&mut command, ACCESSIBILITY_COMMAND_TIMEOUT)
        .map_err(|e| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: format!("Windows UIA probe launch failed: {e}"),
        })?
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "Windows UIA probe timed out".to_string(),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: format!("Windows UIA probe failed: {}", stderr.trim()),
        });
    }

    parse_accessibility_lines(&String::from_utf8_lossy(&output.stdout))
}
