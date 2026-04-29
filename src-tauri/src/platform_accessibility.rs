use async_trait::async_trait;
use chrono::Utc;
use oneshim_core::error::CoreError;
use oneshim_core::models::intent::{ElementBounds, FinderSource, UiElement};
use oneshim_core::models::ui_scene::{
    NormalizedBounds, UiScene, UiSceneElement, UI_SCENE_SCHEMA_VERSION,
};
use oneshim_core::ports::element_finder::ElementFinder;
use std::process::Command;
use std::sync::Arc;

#[cfg_attr(not(test), allow(dead_code))]
const ACCESSIBILITY_MAX_ELEMENTS: usize = 300;
#[cfg_attr(not(test), allow(dead_code))]
const FIELD_DELIMITER: &str = "|||";

#[derive(Debug, Clone)]
struct AccessibilityNode {
    app_name: Option<String>,
    role: Option<String>,
    label: String,
    bounds: ElementBounds,
    confidence: f64,
}

pub fn create_platform_accessibility_finder() -> Arc<dyn ElementFinder> {
    Arc::new(PlatformAccessibilityElementFinder)
}

pub struct PlatformAccessibilityElementFinder;

#[async_trait]
impl ElementFinder for PlatformAccessibilityElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        let mut elements: Vec<UiElement> = query_accessibility_nodes()?
            .into_iter()
            .filter(|node| {
                text.map(|value| contains_ignore_case(&node.label, value))
                    .unwrap_or(true)
                    && role
                        .map(|value| {
                            node.role
                                .as_deref()
                                .map(|node_role| contains_ignore_case(node_role, value))
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
                    && region
                        .map(|bounds| intersects(bounds, &node.bounds))
                        .unwrap_or(true)
            })
            .map(|node| UiElement {
                text: node.label,
                bounds: node.bounds,
                role: node.role,
                confidence: node.confidence,
                source: FinderSource::Accessibility,
            })
            .collect();

        elements.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if elements.is_empty() {
            return Err(CoreError::ElementNotFound {
                code: oneshim_core::error_codes::UiCode::ElementMissing,
                name: "No accessibility candidates matched query".to_string(),
            });
        }

        Ok(elements)
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        let nodes = query_accessibility_nodes()?;
        if nodes.is_empty() {
            return Err(CoreError::ElementNotFound {
                code: oneshim_core::error_codes::UiCode::ElementMissing,
                name: "No accessibility elements discovered".to_string(),
            });
        }

        let (screen_width, screen_height) = estimate_screen_size(&nodes);
        let app_from_nodes = nodes
            .iter()
            .find_map(|node| node.app_name.as_ref().cloned())
            .or_else(|| app_name.map(ToString::to_string));

        let elements = nodes
            .into_iter()
            .enumerate()
            .map(|(idx, node)| {
                let bbox_norm = NormalizedBounds::new(
                    node.bounds.x as f32 / screen_width.max(1) as f32,
                    node.bounds.y as f32 / screen_height.max(1) as f32,
                    node.bounds.width as f32 / screen_width.max(1) as f32,
                    node.bounds.height as f32 / screen_height.max(1) as f32,
                );

                UiSceneElement {
                    element_id: format!("ax_{idx}"),
                    bbox_abs: node.bounds,
                    bbox_norm,
                    label: node.label.clone(),
                    role: node.role,
                    intent: None,
                    state: None,
                    confidence: node.confidence,
                    text_masked: Some(node.label),
                    parent_id: None,
                }
            })
            .collect();

        Ok(UiScene {
            schema_version: UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: format!("ax_scene_{}", Utc::now().timestamp_millis()),
            app_name: app_from_nodes,
            screen_id: screen_id.map(ToString::to_string),
            captured_at: Utc::now(),
            screen_width,
            screen_height,
            elements,
        })
    }

    fn name(&self) -> &str {
        "platform-accessibility"
    }
}

fn query_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    #[cfg(target_os = "macos")]
    {
        query_macos_accessibility_nodes()
    }

    #[cfg(target_os = "windows")]
    {
        query_windows_accessibility_nodes()
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        query_linux_accessibility_nodes()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
    {
        Err(CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: "Accessibility adapter is not available on this platform".to_string(),
        })
    }
}

#[cfg(target_os = "macos")]
fn query_macos_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
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
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: format!("macOS AX probe launch failed: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: format!(
                "macOS AX probe failed (check Accessibility permission): {}",
                stderr.trim()
            ),
        });
    }

    parse_accessibility_lines(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "windows")]
fn query_windows_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
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

    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .map_err(|e| CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: format!("Windows UIA probe launch failed: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: format!("Windows UIA probe failed: {}", stderr.trim()),
        });
    }

    parse_accessibility_lines(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn query_linux_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    let window_id = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: "Linux accessibility probe requires xdotool and active X11/XWayland session"
                .to_string(),
        })?;

    let title = Command::new("xdotool")
        .args(["getwindowname", &window_id])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "active-window".to_string());

    let pid = Command::new("xdotool")
        .args(["getwindowpid", &window_id])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
                .ok()
        });

    let app_name = pid
        .and_then(read_proc_name)
        .unwrap_or_else(|| "unknown".to_string());

    let geometry = Command::new("xdotool")
        .args(["getwindowgeometry", "--shell", &window_id])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: "Failed to read active window geometry".to_string(),
        })?;

    let mut x = 0i32;
    let mut y = 0i32;
    let mut w = 0u32;
    let mut h = 0u32;
    for line in geometry.lines() {
        if let Some(value) = line.strip_prefix("X=") {
            x = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("Y=") {
            y = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("WIDTH=") {
            w = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("HEIGHT=") {
            h = value.parse().unwrap_or(0);
        }
    }

    if w == 0 || h == 0 {
        return Err(CoreError::ServiceUnavailable {
            code: oneshim_core::error_codes::ServiceCode::Unavailable,
            message: "Invalid active window geometry from xdotool".to_string(),
        });
    }

    Ok(vec![AccessibilityNode {
        app_name: Some(app_name),
        role: Some("window".to_string()),
        label: title,
        bounds: ElementBounds {
            x,
            y,
            width: w,
            height: h,
        },
        confidence: 0.75,
    }])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_proc_name(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg_attr(not(test), allow(dead_code))]
fn parse_accessibility_lines(raw: &str) -> Result<Vec<AccessibilityNode>, CoreError> {
    let mut nodes = Vec::new();

    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let parts: Vec<&str> = line.split(FIELD_DELIMITER).collect();
        if parts.len() < 7 {
            continue;
        }

        let x = parts[3].parse::<i32>().unwrap_or(0);
        let y = parts[4].parse::<i32>().unwrap_or(0);
        let width = parts[5].parse::<u32>().unwrap_or(0);
        let height = parts[6].parse::<u32>().unwrap_or(0);
        if width == 0 || height == 0 {
            continue;
        }

        let label = parts[2].trim();
        if label.is_empty() {
            continue;
        }

        nodes.push(AccessibilityNode {
            app_name: trim_to_option(parts[0]),
            role: trim_to_option(parts[1]),
            label: label.to_string(),
            bounds: ElementBounds {
                x,
                y,
                width,
                height,
            },
            confidence: 0.98,
        });

        if nodes.len() >= ACCESSIBILITY_MAX_ELEMENTS {
            break;
        }
    }

    if nodes.is_empty() {
        return Err(CoreError::ElementNotFound {
            code: oneshim_core::error_codes::UiCode::ElementMissing,
            name: "Accessibility probe returned no actionable elements".to_string(),
        });
    }

    Ok(nodes)
}

fn estimate_screen_size(nodes: &[AccessibilityNode]) -> (u32, u32) {
    let max_x = nodes
        .iter()
        .map(|node| node.bounds.x.saturating_add(node.bounds.width as i32))
        .max()
        .unwrap_or(1920)
        .max(1);
    let max_y = nodes
        .iter()
        .map(|node| node.bounds.y.saturating_add(node.bounds.height as i32))
        .max()
        .unwrap_or(1080)
        .max(1);

    (max_x as u32, max_y as u32)
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg_attr(not(test), allow(dead_code))]
fn trim_to_option(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn intersects(a: &ElementBounds, b: &ElementBounds) -> bool {
    let a_right = a.x + a.width as i32;
    let a_bottom = a.y + a.height as i32;
    let b_right = b.x + b.width as i32;
    let b_bottom = b.y + b.height as i32;

    a.x < b_right && b.x < a_right && a.y < b_bottom && b.y < a_bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accessibility_lines_parses_expected_fields() {
        let raw = "Code|||button|||Save|||100|||200|||80|||30\n";
        let nodes = parse_accessibility_lines(raw).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].app_name.as_deref(), Some("Code"));
        assert_eq!(nodes[0].role.as_deref(), Some("button"));
        assert_eq!(nodes[0].label, "Save");
        assert_eq!(nodes[0].bounds.x, 100);
    }

    #[test]
    fn intersects_detects_overlap() {
        let a = ElementBounds {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
        };
        let b = ElementBounds {
            x: 5,
            y: 5,
            width: 10,
            height: 10,
        };
        assert!(intersects(&a, &b));
    }
}
