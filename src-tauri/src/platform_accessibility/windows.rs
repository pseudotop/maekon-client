use maekon_core::error::CoreError;
use std::process::Command;

use super::command_timeout::{command_output_with_timeout, ACCESSIBILITY_COMMAND_TIMEOUT};
use super::types::{parse_accessibility_lines, AccessibilityNode};

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

        $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        $limit = [Math]::Min(300, $all.Count)
        for ($i = 0; $i -lt $limit; $i++) {
            $node = $all.Item($i)
            $label = $node.Current.Name
            if ([string]::IsNullOrWhiteSpace($label)) {
                $label = $node.Current.AutomationId
            }
            if ([string]::IsNullOrWhiteSpace($label)) {
                continue
            }

            $role = $node.Current.ControlType.ProgrammaticName
            $rect = $node.Current.BoundingRectangle
            if ($rect.Width -le 0 -or $rect.Height -le 0) {
                continue
            }

            Write-Output "$procName|||$role|||$label|||$([int]$rect.X)|||$([int]$rect.Y)|||$([int]$rect.Width)|||$([int]$rect.Height)"
        }
    "#;

    let mut command = Command::new("powershell");
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
