//! Privacy validation for one complete, selected-display native observation.

use std::collections::HashSet;

use maekon_core::config::PrivacyConfig;
use maekon_core::error::CoreError;
use maekon_core::models::context::WindowBounds;
use maekon_core::ports::vision::{capture_denied, CaptureScene, CaptureWindow};

use super::should_exclude_by_policy;

pub(crate) fn same_bounds(left: WindowBounds, right: WindowBounds) -> bool {
    (left.x, left.y, left.width, left.height) == (right.x, right.y, right.width, right.height)
}

pub(crate) fn same_foreground(left: &CaptureWindow, right: &CaptureWindow) -> bool {
    left.native_id == right.native_id
        && left.pid == right.pid
        && left.app_name == right.app_name
        && left.title == right.title
}

/// Reject incomplete identities, inconsistent foreground geometry, or any
/// excluded visible window, including windows behind an allowed foreground.
///
/// The observer must enumerate every visible window intersecting the selected
/// display and reject unavailable observations before calling this function.
/// This validates a scene; it does not grant consent, native permission or a
/// capture permit. Live authority must bind and revalidate those separately.
pub fn validate_capture_scene(
    policy: &PrivacyConfig,
    scene: &CaptureScene,
) -> Result<(), CoreError> {
    let mut identities = HashSet::new();
    if scene.display_bounds.width == 0
        || scene.display_bounds.height == 0
        || !scene.windows.iter().any(|window| {
            same_foreground(window, &scene.foreground)
                && same_bounds(window.bounds, scene.foreground.bounds)
        })
        || scene.windows.iter().any(|window| {
            window.native_id == 0
                || !identities.insert(window.native_id)
                || window.pid == 0
                || window.app_name.trim().is_empty()
                || window.bounds.width == 0
                || window.bounds.height == 0
                || should_exclude_by_policy(policy, &window.app_name, &window.title)
        })
    {
        return Err(capture_denied("capture scene policy denied"));
    }
    Ok(())
}
