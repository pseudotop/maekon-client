//! Windows-targeted benchmark summaries: consumed by cfg(windows) paths
//! only — dead by design elsewhere; keep Windows strict.
#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::collections::{BTreeMap, BTreeSet};

use maekon_core::models::focused_element::{AccessibilityElement, ElementRect, FocusedElementInfo};
use maekon_core::models::gui::GuiBenchmarkPrivacyStatus;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::UiScene;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WindowsUiaBenchmarkSummary {
    pub(crate) com_focused: FocusedElementPathSummary,
    pub(crate) com_tree: ElementCollectionPathSummary,
    pub(crate) platform_scene: ElementCollectionPathSummary,
    pub(crate) convergence: WindowsUiaConvergenceSummary,
    pub(crate) privacy_status: GuiBenchmarkPrivacyStatus,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct FocusedElementPathSummary {
    pub(crate) success: bool,
    pub(crate) role: Option<String>,
    pub(crate) bounds_present: bool,
    pub(crate) bounds_sane: bool,
    pub(crate) label_observed: bool,
    pub(crate) value_length_observed: bool,
    pub(crate) extracted_text_exposed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ElementCollectionPathSummary {
    pub(crate) success: bool,
    pub(crate) element_count: usize,
    pub(crate) bounds_present_count: usize,
    pub(crate) bounds_sane_count: usize,
    pub(crate) first_role: Option<String>,
    pub(crate) role_counts: BTreeMap<String, usize>,
    pub(crate) roles: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WindowsUiaConvergenceSummary {
    pub(crate) focused_role_seen_in_com_tree: bool,
    pub(crate) focused_role_seen_in_platform_scene: bool,
    pub(crate) focused_bounds_overlap_com_tree: bool,
    pub(crate) focused_bounds_overlap_platform_candidate: bool,
    pub(crate) role_overlap_count: usize,
}

pub(crate) fn summarize_windows_uia_observations(
    focused: Option<&FocusedElementInfo>,
    com_tree: &[AccessibilityElement],
    platform_scene: Option<&UiScene>,
) -> WindowsUiaBenchmarkSummary {
    let com_focused = summarize_focused_element(focused);
    let com_tree_summary = summarize_accessibility_elements(com_tree);
    let platform_scene_summary = summarize_scene(platform_scene);
    let convergence = summarize_convergence(focused, com_tree, platform_scene);

    WindowsUiaBenchmarkSummary {
        com_focused,
        com_tree: com_tree_summary,
        platform_scene: platform_scene_summary,
        convergence,
        privacy_status: GuiBenchmarkPrivacyStatus::Redacted,
    }
}

fn summarize_focused_element(focused: Option<&FocusedElementInfo>) -> FocusedElementPathSummary {
    let Some(focused) = focused else {
        return FocusedElementPathSummary {
            success: false,
            role: None,
            bounds_present: false,
            bounds_sane: false,
            label_observed: false,
            value_length_observed: false,
            extracted_text_exposed: false,
        };
    };

    FocusedElementPathSummary {
        success: true,
        role: non_empty_string(&focused.role),
        bounds_present: focused.position.is_some(),
        bounds_sane: focused.position.as_ref().is_some_and(element_rect_sane),
        label_observed: focused
            .label
            .as_ref()
            .is_some_and(|label| !label.trim().is_empty()),
        value_length_observed: focused.value_length.is_some(),
        extracted_text_exposed: focused
            .extracted_text
            .as_ref()
            .is_some_and(|text| !text.is_empty()),
    }
}

fn summarize_accessibility_elements(
    elements: &[AccessibilityElement],
) -> ElementCollectionPathSummary {
    let role_counts = role_counts(
        elements
            .iter()
            .filter_map(|element| normalized_role(&element.role)),
    );
    let roles = role_counts.keys().cloned().collect::<Vec<_>>();

    ElementCollectionPathSummary {
        success: !elements.is_empty(),
        element_count: elements.len(),
        bounds_present_count: elements
            .iter()
            .filter(|element| element.bounds.is_some())
            .count(),
        bounds_sane_count: elements
            .iter()
            .filter(|element| element.bounds.as_ref().is_some_and(element_rect_sane))
            .count(),
        first_role: elements
            .first()
            .and_then(|element| normalized_role(&element.role)),
        role_counts,
        roles,
    }
}

fn summarize_scene(scene: Option<&UiScene>) -> ElementCollectionPathSummary {
    let Some(scene) = scene else {
        return ElementCollectionPathSummary {
            success: false,
            element_count: 0,
            bounds_present_count: 0,
            bounds_sane_count: 0,
            first_role: None,
            role_counts: BTreeMap::new(),
            roles: Vec::new(),
        };
    };

    let role_counts = role_counts(
        scene
            .elements
            .iter()
            .filter_map(|element| element.role.as_deref().and_then(normalized_role)),
    );
    let roles = role_counts.keys().cloned().collect::<Vec<_>>();

    ElementCollectionPathSummary {
        success: !scene.elements.is_empty(),
        element_count: scene.elements.len(),
        bounds_present_count: scene.elements.len(),
        bounds_sane_count: scene
            .elements
            .iter()
            .filter(|element| element_bounds_sane(&element.bbox_abs))
            .count(),
        first_role: scene
            .elements
            .first()
            .and_then(|element| element.role.as_deref().and_then(normalized_role)),
        role_counts,
        roles,
    }
}

fn role_counts<I>(roles: I) -> BTreeMap<String, usize>
where
    I: IntoIterator<Item = String>,
{
    let mut counts = BTreeMap::new();
    for role in roles {
        *counts.entry(role).or_insert(0) += 1;
    }
    counts
}

fn summarize_convergence(
    focused: Option<&FocusedElementInfo>,
    com_tree: &[AccessibilityElement],
    platform_scene: Option<&UiScene>,
) -> WindowsUiaConvergenceSummary {
    let focused_role = focused.and_then(|focused| normalized_role(&focused.role));
    let focused_bounds = focused.and_then(|focused| focused.position);

    let com_roles = com_tree
        .iter()
        .filter_map(|element| normalized_role(&element.role))
        .collect::<BTreeSet<_>>();
    let platform_roles = platform_scene
        .into_iter()
        .flat_map(|scene| scene.elements.iter())
        .filter_map(|element| element.role.as_deref().and_then(normalized_role))
        .collect::<BTreeSet<_>>();

    let focused_role_seen_in_com_tree = focused_role
        .as_ref()
        .is_some_and(|role| com_roles.contains(role));
    let focused_role_seen_in_platform_scene = focused_role
        .as_ref()
        .is_some_and(|role| platform_roles.contains(role));

    let focused_bounds_overlap_com_tree = focused_bounds.as_ref().is_some_and(|bounds| {
        com_tree.iter().any(|element| {
            element
                .bounds
                .as_ref()
                .is_some_and(|candidate| element_rects_overlap(bounds, candidate))
        })
    });
    let focused_bounds_overlap_platform_candidate = focused_bounds.as_ref().is_some_and(|bounds| {
        platform_scene
            .into_iter()
            .flat_map(|scene| scene.elements.iter())
            .any(|candidate| element_rect_overlaps_bounds(bounds, &candidate.bbox_abs))
    });

    WindowsUiaConvergenceSummary {
        focused_role_seen_in_com_tree,
        focused_role_seen_in_platform_scene,
        focused_bounds_overlap_com_tree,
        focused_bounds_overlap_platform_candidate,
        role_overlap_count: com_roles.intersection(&platform_roles).count(),
    }
}

fn normalized_role(role: &str) -> Option<String> {
    let trimmed = role.trim();
    if trimmed.is_empty() {
        return None;
    }
    let without_prefix = trimmed
        .strip_prefix("ControlType.")
        .or_else(|| trimmed.strip_prefix("AX"))
        .unwrap_or(trimmed);
    let normalized = without_prefix
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    non_empty_string(&normalized)
}

fn non_empty_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn element_rect_sane(rect: &ElementRect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width > 0.0
        && rect.height > 0.0
}

fn element_bounds_sane(bounds: &ElementBounds) -> bool {
    bounds.width > 0 && bounds.height > 0
}

fn element_rects_overlap(a: &ElementRect, b: &ElementRect) -> bool {
    let a_right = a.x + a.width;
    let a_bottom = a.y + a.height;
    let b_right = b.x + b.width;
    let b_bottom = b.y + b.height;

    a.x < b_right && b.x < a_right && a.y < b_bottom && b.y < a_bottom
}

fn element_rect_overlaps_bounds(rect: &ElementRect, bounds: &ElementBounds) -> bool {
    let bounds_rect = ElementRect {
        x: bounds.x as f32,
        y: bounds.y as f32,
        width: bounds.width as f32,
        height: bounds.height as f32,
    };
    element_rects_overlap(rect, &bounds_rect)
}
