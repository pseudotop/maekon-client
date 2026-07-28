//! Shared accessibility line-format parsing: consumed by the macOS and
//! Windows extraction paths — dead on Linux by design (AT-SPI uses its own
//! pipeline in maekon-vision).
#![cfg_attr(target_os = "linux", allow(dead_code))]

use maekon_core::error::CoreError;
use maekon_core::models::intent::ElementBounds;

pub(super) const ACCESSIBILITY_MAX_ELEMENTS: usize = 300;
pub(super) const FIELD_DELIMITER: &str = "|||";

#[derive(Debug, Clone)]
pub(super) struct AccessibilityNode {
    pub(super) app_name: Option<String>,
    pub(super) role: Option<String>,
    pub(super) label: String,
    pub(super) bounds: ElementBounds,
    pub(super) confidence: f64,
}

pub(super) fn parse_accessibility_lines(raw: &str) -> Result<Vec<AccessibilityNode>, CoreError> {
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

        let role = trim_to_option(parts[1]);
        let label = parts[2].trim();
        // A focused Windows surface can be a valid structural node (for
        // example, ControlType.Pane) without an accessible name or automation
        // id. Keep role-only nodes so scene analysis does not discard the
        // entire accessibility subtree. Lines with neither role nor label are
        // still non-actionable and remain fail-closed.
        if role.is_none() && label.is_empty() {
            continue;
        }

        nodes.push(AccessibilityNode {
            app_name: trim_to_option(parts[0]),
            role,
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
            code: maekon_core::error_codes::UiCode::ElementMissing,
            name: "Accessibility probe returned no actionable elements".to_string(),
        });
    }

    Ok(nodes)
}

pub(super) fn trim_to_option(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
