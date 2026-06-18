//! COM operations for Windows UIAutomation accessibility extraction.
//!
//! All functions in this module require COM to be initialized on the calling
//! thread before invocation. They are intended to be called exclusively from
//! `spawn_blocking` closures in `WindowsUiaAccessibility`.
//!
//! ## Legacy vtable reference (kept as documentation)
//!
//! Before migration to the type-safe `windows` crate (0.62), COM methods were
//! called via hard-coded vtable offsets. Those offsets are preserved below for
//! debugging reference only.
//!
//! IUIAutomation vtable:
//!   GetFocusedElement: index 8 | get_RawViewWalker: index 15 | CreateCacheRequest: index 20
//!
//! IUIAutomationElement vtable:
//!   GetCurrentPropertyValue: index 12 | get_CurrentControlType: index 21
//!   get_CachedControlType: index 22  | get_CurrentName: index 23
//!   get_CachedName: index 24         | get_CurrentBoundingRectangle: index 27
//!   get_CachedBoundingRectangle: index 28
//!
//! IUIAutomationTreeWalker vtable:
//!   GetFirstChildElement: index 4         | GetFirstChildElementBuildCache: index 5
//!   GetNextSiblingElement: index 6        | GetNextSiblingElementBuildCache: index 7
//!
//! IUIAutomationCacheRequest vtable:
//!   AddProperty: index 3 | put_TreeScope: index 7

use std::ptr;

use zeroize::Zeroizing;

use windows::core::BSTR;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::System::Variant::VT_BSTR;
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, TreeScope, TreeScope_Children, TreeScope_Element,
    UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId, UIA_NamePropertyId,
    UIA_ValueValuePropertyId,
};

use maekon_core::models::focused_element::ElementRect;

use super::roles::control_type_to_role;
use super::types::RawFocusedElement;

const UIA_WINDOW_CONTROL_TYPE_ID: i32 = 50032;
const MAX_WINDOW_ROOT_ASCENT: usize = 32;

// ── Geometry helpers ─────────────────────────────────────────────────────────

/// Convert a Win32 `RECT` (left/top/right/bottom, i32) to `ElementRect`
/// (x/y/width/height, f32). Returns `None` for zero-area rectangles.
fn rect_to_element_rect(rect: &windows::Win32::Foundation::RECT) -> Option<ElementRect> {
    // Clamp dimensions to non-negative so an inverted RECT (right < left or
    // bottom < top) cannot produce a negative width/height. Both dimensions
    // must be strictly positive to honor the documented "None for zero-area"
    // contract — a rect with either dimension == 0 has zero area and must be
    // rejected (previously `||` leaked such degenerate/negative rects).
    let width = (rect.right - rect.left).max(0);
    let height = (rect.bottom - rect.top).max(0);
    if width > 0 && height > 0 {
        Some(ElementRect {
            x: rect.left as f32,
            y: rect.top as f32,
            width: width as f32,
            height: height as f32,
        })
    } else {
        None
    }
}

/// Convert a `BSTR` to `Option<String>`, returning `None` for null/empty.
fn bstr_to_opt_string(bstr: &BSTR) -> Option<String> {
    if bstr.is_empty() {
        None
    } else {
        Some(bstr.to_string())
    }
}

// ── Focused element extraction ────────────────────────────────────────────────

/// Extract focused element data via COM UIAutomation API.
///
/// Returns `None` if COM initialization fails, no element is focused,
/// or any step in the extraction chain fails. Errors are logged at
/// debug level to avoid noise under normal operation.
///
/// SAFETY: Performs COM calls that are `unsafe` in the `windows` crate.
/// Must be called from `spawn_blocking` to avoid blocking the tokio runtime.
pub(super) fn extract_via_uia() -> Option<RawFocusedElement> {
    unsafe {
        // Step 1: Initialize COM (COINIT_MULTITHREADED = 0x0)
        let hr = windows_sys::Win32::System::Com::CoInitializeEx(
            ptr::null(),
            windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32,
        );
        // S_OK (0) or S_FALSE (1, already initialized) are both acceptable.
        if hr < 0 {
            tracing::debug!(hresult = hr, "CoInitializeEx failed");
            return None;
        }
        let _com_guard = ComGuard;

        // Step 2: Create IUIAutomation instance (type-safe)
        let automation: IUIAutomation =
            match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                Ok(a) => a,
                Err(e) => {
                    tracing::debug!(error = %e, "CoCreateInstance(CUIAutomation) failed");
                    return None;
                }
            };

        // Step 3: GetFocusedElement
        let element: IUIAutomationElement = match automation.GetFocusedElement() {
            Ok(el) => el,
            Err(e) => {
                tracing::debug!(error = %e, "GetFocusedElement returned no element");
                return None;
            }
        };

        // Step 4a: CurrentControlType → role
        let role = match element.CurrentControlType() {
            Ok(ct) => control_type_to_role(ct.0).to_string(),
            Err(_) => "Unknown".to_string(),
        };

        // Step 4b: CurrentName → label
        let name = match element.CurrentName() {
            Ok(bstr) => bstr_to_opt_string(&bstr).map(Zeroizing::new),
            Err(_) => None,
        };

        // Step 4c: CurrentBoundingRectangle → position
        let position = match element.CurrentBoundingRectangle() {
            Ok(rect) => rect_to_element_rect(&rect),
            Err(_) => None,
        };

        // Step 4d: GetCurrentPropertyValue(UIA_ValueValuePropertyId) → text value
        let value = match element.GetCurrentPropertyValue(UIA_ValueValuePropertyId) {
            Ok(variant) => {
                if variant.vt() == VT_BSTR {
                    // SAFETY: vt() confirmed VT_BSTR, so bstrVal is valid.
                    // bstrVal is ManuallyDrop<BSTR> in windows 0.62 — clone to avoid double-free.
                    let bstr = std::mem::ManuallyDrop::into_inner(
                        variant.Anonymous.Anonymous.Anonymous.bstrVal.clone(),
                    );
                    bstr_to_opt_string(&bstr).map(Zeroizing::new)
                } else {
                    None
                }
            }
            Err(_) => None,
        };

        // element, automation are dropped here — Release called automatically
        Some(RawFocusedElement {
            role,
            name,
            value,
            position,
        })
    }
}

// ── Tree traversal ────────────────────────────────────────────────────────────

/// Extract the accessibility subtree of the focused element's parent window.
///
/// Uses IUIAutomation TreeWalker for breadth-first traversal with depth and
/// element count limits. When possible, a CacheRequest is used to batch-fetch
/// ControlType, Name, and BoundingRectangle in a single cross-process call per
/// subtree level (3x fewer COM roundtrips). Falls back to per-property fetching
/// if CacheRequest creation fails.
pub(super) fn extract_tree_via_uia(
    max_depth: u32,
    max_elements: usize,
) -> Vec<(String, Option<String>, Option<ElementRect>)> {
    unsafe {
        let hr = windows_sys::Win32::System::Com::CoInitializeEx(
            ptr::null(),
            windows_sys::Win32::System::Com::COINIT_MULTITHREADED as u32,
        );
        if hr < 0 {
            return Vec::new();
        }
        let _com_guard = ComGuard;

        let automation: IUIAutomation =
            match CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) {
                Ok(a) => a,
                Err(_) => return Vec::new(),
            };

        let element: IUIAutomationElement = match automation.GetFocusedElement() {
            Ok(el) => el,
            Err(_) => return Vec::new(),
        };

        let walker: IUIAutomationTreeWalker = match automation.RawViewWalker() {
            Ok(w) => w,
            Err(_) => return Vec::new(),
        };

        // Try to create a CacheRequest for bulk property fetching.
        let cache_request = create_cache_request(&automation);
        let use_cache = cache_request.is_some();
        if use_cache {
            tracing::debug!("UIA tree traversal: using CacheRequest (bulk property fetch)");
        } else {
            tracing::debug!(
                "UIA tree traversal: CacheRequest unavailable, using per-property calls"
            );
        }

        let traversal_root = resolve_window_root(&walker, &element);
        let mut results = Vec::new();
        let mut remaining = max_elements;
        collect_subtree(
            &walker,
            &traversal_root,
            0,
            max_depth,
            &mut remaining,
            &mut results,
            cache_request.as_ref(),
            use_cache,
        );

        // walker, element, cache_request, automation dropped here — Release automatic
        results
    }
}

/// Pick the nearest Window control type in a focused-element-to-root chain.
fn select_window_root_index(control_types_from_focus: &[Option<i32>]) -> usize {
    control_types_from_focus
        .iter()
        .position(|control_type| *control_type == Some(UIA_WINDOW_CONTROL_TYPE_ID))
        .unwrap_or(0)
}

/// Resolve the traversal root from a focused element to its nearest parent Window.
///
/// SAFETY: Requires COM to be initialized on the current thread. Falls back to the
/// focused element when parent traversal or control type reads are unavailable.
unsafe fn resolve_window_root(
    walker: &IUIAutomationTreeWalker,
    focused: &IUIAutomationElement,
) -> IUIAutomationElement {
    let mut elements = Vec::with_capacity(8);
    let mut control_types = Vec::with_capacity(8);
    let mut current = focused.clone();

    elements.push(current.clone());
    control_types.push(current.CurrentControlType().ok().map(|ct| ct.0));

    for _ in 0..MAX_WINDOW_ROOT_ASCENT {
        if select_window_root_index(&control_types) != 0 {
            break;
        }

        let parent = match walker.GetParentElement(&current) {
            Ok(parent) => parent,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    ancestor_count = elements.len(),
                    "UIA tree traversal: parent lookup stopped before Window root"
                );
                break;
            }
        };

        control_types.push(parent.CurrentControlType().ok().map(|ct| ct.0));
        elements.push(parent.clone());
        current = parent;
    }

    let selected_index = select_window_root_index(&control_types);
    if selected_index > 0 {
        tracing::debug!(
            ancestor_steps = selected_index,
            "UIA tree traversal: using nearest Window ancestor as root"
        );
    } else {
        tracing::debug!("UIA tree traversal: using focused element as root");
    }

    elements
        .get(selected_index)
        .cloned()
        .unwrap_or_else(|| focused.clone())
}

// ── Cache request helpers ─────────────────────────────────────────────────────

/// Create a CacheRequest that pre-fetches ControlType, Name, and
/// BoundingRectangle properties for Element + Children scope.
///
/// Returns `None` if creation or configuration fails.
///
/// SAFETY: Requires COM to be initialized on the current thread.
unsafe fn create_cache_request(automation: &IUIAutomation) -> Option<IUIAutomationCacheRequest> {
    let cache_request = match automation.CreateCacheRequest() {
        Ok(cr) => cr,
        Err(e) => {
            tracing::debug!(error = %e, "CreateCacheRequest failed");
            return None;
        }
    };

    if let Err(e) = cache_request.AddProperty(UIA_ControlTypePropertyId) {
        tracing::debug!(error = %e, "AddProperty(ControlType) failed");
        return None;
    }
    if let Err(e) = cache_request.AddProperty(UIA_NamePropertyId) {
        tracing::debug!(error = %e, "AddProperty(Name) failed");
        return None;
    }
    if let Err(e) = cache_request.AddProperty(UIA_BoundingRectanglePropertyId) {
        tracing::debug!(error = %e, "AddProperty(BoundingRectangle) failed");
        return None;
    }

    if let Err(e) =
        cache_request.SetTreeScope(TreeScope(TreeScope_Element.0 | TreeScope_Children.0))
    {
        tracing::debug!(error = %e, "SetTreeScope failed");
        return None;
    }

    Some(cache_request)
}

// ── Property extraction helpers ───────────────────────────────────────────────

/// Extract properties from an element's cache (populated by BuildCache).
///
/// Uses `CachedControlType`, `CachedName`, and `CachedBoundingRectangle`
/// instead of their `Current` counterparts to avoid cross-process COM calls.
///
/// SAFETY: `element` must have a populated cache obtained via a BuildCache
/// walker method.
unsafe fn extract_cached_properties(
    element: &IUIAutomationElement,
) -> (String, Option<String>, Option<ElementRect>) {
    let role = match element.CachedControlType() {
        Ok(ct) => control_type_to_role(ct.0).to_string(),
        Err(_) => "Unknown".to_string(),
    };
    let name = match element.CachedName() {
        Ok(bstr) => bstr_to_opt_string(&bstr),
        Err(_) => None,
    };
    let position = match element.CachedBoundingRectangle() {
        Ok(rect) => rect_to_element_rect(&rect),
        Err(_) => None,
    };
    (role, name, position)
}

/// Extract properties from an element using per-property Current calls.
///
/// Non-cached fallback path. Each property requires a separate cross-process
/// COM call.
///
/// SAFETY: `element` must be a valid IUIAutomationElement.
unsafe fn extract_current_properties(
    element: &IUIAutomationElement,
) -> (String, Option<String>, Option<ElementRect>) {
    let role = match element.CurrentControlType() {
        Ok(ct) => control_type_to_role(ct.0).to_string(),
        Err(_) => "Unknown".to_string(),
    };
    let name = match element.CurrentName() {
        Ok(bstr) => bstr_to_opt_string(&bstr),
        Err(_) => None,
    };
    let position = match element.CurrentBoundingRectangle() {
        Ok(rect) => rect_to_element_rect(&rect),
        Err(_) => None,
    };
    (role, name, position)
}

// ── Tree walker ───────────────────────────────────────────────────────────────

/// Recursive depth-limited subtree collection with optional CacheRequest.
///
/// When `use_cache` is true and `cache_request` is `Some`, uses
/// `GetFirstChildElementBuildCache` / `GetNextSiblingElementBuildCache`
/// to populate the element cache, then reads via `Cached*` methods.
/// This reduces cross-process COM calls from 3 per element to 1 per walker step.
///
/// When `use_cache` is false, falls back to the original per-property
/// `Current*` calls (3 cross-process calls per element).
unsafe fn collect_subtree(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: u32,
    max_depth: u32,
    remaining: &mut usize,
    results: &mut Vec<(String, Option<String>, Option<ElementRect>)>,
    cache_request: Option<&IUIAutomationCacheRequest>,
    use_cache: bool,
) {
    if *remaining == 0 || depth > max_depth {
        return;
    }

    let (role, name, position) = if use_cache && depth > 0 {
        extract_cached_properties(element)
    } else {
        extract_current_properties(element)
    };

    results.push((role, name, position));
    *remaining = remaining.saturating_sub(1);

    if depth < max_depth && *remaining > 0 {
        let first_child = if use_cache {
            if let Some(cr) = cache_request {
                walker.GetFirstChildElementBuildCache(element, cr).ok()
            } else {
                walker.GetFirstChildElement(element).ok()
            }
        } else {
            walker.GetFirstChildElement(element).ok()
        };

        if let Some(child) = first_child {
            collect_subtree(
                walker,
                &child,
                depth + 1,
                max_depth,
                remaining,
                results,
                cache_request,
                use_cache,
            );

            let mut current = child;
            loop {
                if *remaining == 0 {
                    break;
                }

                let next_sibling = if use_cache {
                    if let Some(cr) = cache_request {
                        walker.GetNextSiblingElementBuildCache(&current, cr).ok()
                    } else {
                        walker.GetNextSiblingElement(&current).ok()
                    }
                } else {
                    walker.GetNextSiblingElement(&current).ok()
                };

                // current is dropped here when reassigned — Release automatic
                match next_sibling {
                    Some(sibling) => {
                        current = sibling;
                        collect_subtree(
                            walker,
                            &current,
                            depth + 1,
                            max_depth,
                            remaining,
                            results,
                            cache_request,
                            use_cache,
                        );
                    }
                    None => break,
                }
            }
        }
    }
}

// ── COM lifetime guard ────────────────────────────────────────────────────────

/// RAII guard: calls `CoUninitialize` on drop to balance `CoInitializeEx`.
pub(super) struct ComGuard;

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Com::CoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn focused_chain_root_selection_prefers_nearest_window_ancestor() {
        let focused_to_root_control_types = [Some(50004), Some(50033), Some(50032), Some(50032)];

        assert_eq!(
            super::select_window_root_index(&focused_to_root_control_types),
            2
        );
    }

    #[test]
    fn focused_chain_root_selection_falls_back_to_focused_element_without_window() {
        let focused_to_root_control_types = [Some(50004), Some(50033), None, Some(50025)];

        assert_eq!(
            super::select_window_root_index(&focused_to_root_control_types),
            0
        );
    }

    use windows::Win32::Foundation::RECT;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn rect_to_element_rect_accepts_positive_area() {
        let result = super::rect_to_element_rect(&rect(10, 20, 110, 70))
            .expect("positive-area rect should convert");
        assert_eq!(result.x, 10.0);
        assert_eq!(result.y, 20.0);
        assert_eq!(result.width, 100.0);
        assert_eq!(result.height, 50.0);
    }

    #[test]
    fn rect_to_element_rect_rejects_zero_width() {
        // Zero width → zero area → must be None (previously leaked via `||`).
        assert!(super::rect_to_element_rect(&rect(10, 20, 10, 70)).is_none());
    }

    #[test]
    fn rect_to_element_rect_rejects_zero_height() {
        // Zero height → zero area → must be None (previously leaked via `||`).
        assert!(super::rect_to_element_rect(&rect(10, 20, 110, 20)).is_none());
    }

    #[test]
    fn rect_to_element_rect_rejects_fully_collapsed() {
        assert!(super::rect_to_element_rect(&rect(10, 20, 10, 20)).is_none());
    }

    #[test]
    fn rect_to_element_rect_rejects_inverted_dimensions() {
        // Inverted RECT (right < left, bottom < top) must not produce a
        // negative-dimension ElementRect.
        assert!(super::rect_to_element_rect(&rect(110, 70, 10, 20)).is_none());
    }
}
