pub(crate) mod benchmark;
mod command_timeout;
pub(super) mod types;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(all(unix, not(target_os = "macos")))]
mod linux;

#[cfg(test)]
mod tests;

use async_trait::async_trait;
use chrono::Utc;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::intent::{ElementBounds, FinderSource, UiElement};
use maekon_core::models::ui_scene::{
    NormalizedBounds, UiScene, UiSceneElement, UI_SCENE_SCHEMA_VERSION,
};
use maekon_core::ports::element_finder::ElementFinder;
use maekon_vision::privacy::sanitize_title_with_level;
use std::sync::Arc;

use types::AccessibilityNode;

pub fn create_platform_accessibility_finder(
    pii_filter_level: PiiFilterLevel,
) -> Arc<dyn ElementFinder> {
    Arc::new(PlatformAccessibilityElementFinder { pii_filter_level })
}

pub struct PlatformAccessibilityElementFinder {
    pii_filter_level: PiiFilterLevel,
}

#[async_trait]
impl ElementFinder for PlatformAccessibilityElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        // F-RR-C22-04: query_accessibility_nodes is a synchronous blocking function
        // that invokes std::process::Command (osascript / powershell / xdotool).
        // Isolate it with spawn_blocking so it does not block a tokio worker thread.
        let nodes = tokio::task::spawn_blocking(query_accessibility_nodes)
            .await
            .map_err(|join_err| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error (find_element): {join_err}"),
            })??;

        // The values the filter closure needs are not 'static, so handle them
        // outside spawn_blocking.
        let text_owned = text.map(ToString::to_string);
        let role_owned = role.map(ToString::to_string);
        let region_owned = region.cloned();

        let mut elements: Vec<UiElement> = nodes
            .into_iter()
            .filter(|node| {
                text_owned
                    .as_deref()
                    .map(|value| contains_ignore_case(&node.label, value))
                    .unwrap_or(true)
                    && role_owned
                        .as_deref()
                        .map(|value| {
                            node.role
                                .as_deref()
                                .map(|node_role| contains_ignore_case(node_role, value))
                                .unwrap_or(false)
                        })
                        .unwrap_or(true)
                    && region_owned
                        .as_ref()
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
                code: maekon_core::error_codes::UiCode::ElementMissing,
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
        // F-RR-C22-04: isolate the synchronous Command via spawn_blocking (same
        // pattern as find_element).
        let nodes = tokio::task::spawn_blocking(query_accessibility_nodes)
            .await
            .map_err(|join_err| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error (analyze_scene): {join_err}"),
            })??;

        if nodes.is_empty() {
            return Err(CoreError::ElementNotFound {
                code: maekon_core::error_codes::UiCode::ElementMissing,
                name: "No accessibility elements discovered".to_string(),
            });
        }

        let (screen_width, screen_height) = estimate_screen_size(&nodes);
        let app_from_nodes = nodes
            .iter()
            .find_map(|node| node.app_name.as_ref().cloned())
            .or_else(|| app_name.map(ToString::to_string));

        let elements = accessibility_nodes_to_scene_elements(
            nodes,
            screen_width,
            screen_height,
            self.pii_filter_level,
        );

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

// ── Platform dispatcher ────────────────────────────────────────────────────────

fn query_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    #[cfg(target_os = "macos")]
    {
        macos::query_macos_accessibility_nodes()
    }

    #[cfg(target_os = "windows")]
    {
        windows::query_windows_accessibility_nodes()
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux::query_linux_accessibility_nodes()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
    {
        Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "Accessibility adapter is not available on this platform".to_string(),
        })
    }
}

// ── Shared helpers (also used by tests module via `use super::*`) ──────────────

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

fn accessibility_nodes_to_scene_elements(
    nodes: Vec<AccessibilityNode>,
    screen_width: u32,
    screen_height: u32,
    pii_filter_level: PiiFilterLevel,
) -> Vec<UiSceneElement> {
    let width = screen_width.max(1) as f32;
    let height = screen_height.max(1) as f32;

    nodes
        .into_iter()
        .enumerate()
        .map(|(idx, node)| {
            let bbox_norm = NormalizedBounds::new(
                node.bounds.x as f32 / width,
                node.bounds.y as f32 / height,
                node.bounds.width as f32 / width,
                node.bounds.height as f32 / height,
            );
            let label_masked = sanitize_title_with_level(&node.label, pii_filter_level);

            UiSceneElement {
                element_id: format!("ax_{idx}"),
                bbox_abs: node.bounds,
                bbox_norm,
                label: label_masked.clone(),
                role: node.role,
                intent: None,
                state: None,
                confidence: node.confidence,
                text_masked: Some(label_masked),
                parent_id: None,
            }
        })
        .collect()
}

pub(super) fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

pub(super) fn intersects(a: &ElementBounds, b: &ElementBounds) -> bool {
    let a_right = a.x + a.width as i32;
    let a_bottom = a.y + a.height as i32;
    let b_right = b.x + b.width as i32;
    let b_bottom = b.y + b.height as i32;

    a.x < b_right && b.x < a_right && a.y < b_bottom && b.y < a_bottom
}
