//! AT-SPI2 tree traversal helpers.
//!
//! Low-level functions that walk the accessibility tree using D-Bus proxies:
//! - `find_active_window` — registry root → apps → frames, State::Active check
//! - `traverse_tree`     — recursive depth-limited element collector
//! - `get_element_bounds`— ComponentProxy extents query
//! - `find_focused_in_window` — shallow focused-child scan
//! - `proxy_to_focused_info`  — proxy → owned `FocusedElementInfo`

#[cfg(feature = "linux-atspi")]
use maekon_core::config::PiiFilterLevel;
#[cfg(feature = "linux-atspi")]
use maekon_core::models::focused_element::{AccessibilityElement, ElementRect, FocusedElementInfo};

// ── Active window ─────────────────────────────────────────────────────────────

/// Find the active window frame across all AT-SPI applications.
///
/// Walks: registry root → applications → children (frames/windows),
/// checking each frame for `State::Active`. Returns the first active
/// frame's `AccessibleProxy`, or `None` if no active window is found.
#[cfg(feature = "linux-atspi")]
pub(super) async fn find_active_window<'a>(
    conn: &'a ::atspi::connection::AccessibilityConnection,
) -> Option<::atspi::proxy::accessible::AccessibleProxy<'a>> {
    use atspi_common::{Role, State};

    let root = conn.root_accessible_on_registry().await.ok()?;
    let apps = root.get_children().await.ok()?;

    for app_ref in &apps {
        // Build AccessibleProxy for the application
        let app_proxy = app_ref
            .name()
            .cloned()
            .and_then(|n| {
                ::atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
                    .destination(n)
                    .ok()
            })
            .and_then(|b| b.path(app_ref.path()).ok());
        let app_proxy = match app_proxy {
            Some(b) => match b.build().await {
                Ok(p) => p,
                Err(_) => continue,
            },
            None => continue,
        };

        let children = match app_proxy.get_children().await {
            Ok(c) => c,
            Err(_) => continue,
        };

        for child_ref in &children {
            // Build AccessibleProxy for each child (potential frame)
            let child_proxy = match child_ref
                .name()
                .cloned()
                .and_then(|n| {
                    ::atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
                        .destination(n)
                        .ok()
                })
                .and_then(|b| b.path(child_ref.path()).ok())
            {
                Some(builder) => match builder.build().await {
                    Ok(p) => p,
                    Err(_) => continue,
                },
                None => continue,
            };

            // Check if this is a frame/window with Active state
            let role = match child_proxy.get_role().await {
                Ok(r) => r,
                Err(_) => continue,
            };

            if !matches!(role, Role::Frame | Role::Window | Role::Dialog) {
                continue;
            }

            // Check the state set for Active
            let states = match child_proxy.get_state().await {
                Ok(s) => s,
                Err(_) => continue,
            };

            if states.contains(State::Active) {
                return Some(child_proxy);
            }
        }
    }

    None
}

// ── Tree traversal ────────────────────────────────────────────────────────────

/// Recursively traverse the AT-SPI accessibility tree starting from
/// `proxy`, collecting elements up to `max_depth` levels deep and
/// `remaining` total elements.
///
/// Each node's role, name, and bounding box (via `ComponentProxy`) are
/// extracted and converted to `AccessibilityElement`. Individual
/// element failures are skipped silently.
#[cfg(feature = "linux-atspi")]
pub(super) async fn traverse_tree(
    conn: &::atspi::connection::AccessibilityConnection,
    proxy: &::atspi::proxy::accessible::AccessibleProxy<'_>,
    depth: u32,
    max_depth: u32,
    remaining: &mut usize,
    pii_level: PiiFilterLevel,
) -> Vec<AccessibilityElement> {
    if depth > max_depth || *remaining == 0 {
        return Vec::new();
    }

    let mut results = Vec::new();

    // Extract role as a string
    let role_str = match proxy.get_role().await {
        Ok(role) => format!("{role:?}"),
        Err(_) => "Unknown".to_string(),
    };

    // Extract name/label (suppress at Strict PII level)
    let label = if pii_level != PiiFilterLevel::Strict {
        proxy.name().await.unwrap_or_default()
    } else {
        String::new()
    };

    // Extract bounding box via ComponentProxy
    let bounds = get_element_bounds(conn, proxy).await;

    results.push(AccessibilityElement {
        role: role_str,
        label,
        bounds,
    });
    *remaining = remaining.saturating_sub(1);

    // Recurse into children
    if depth < max_depth && *remaining > 0 {
        // get_children() returns Vec<(destination, object_path)>
        // representing child accessible objects on the D-Bus.
        let children = match proxy.get_children().await {
            Ok(c) => c,
            Err(_) => return results,
        };

        for child_ref in &children {
            if *remaining == 0 {
                break;
            }

            // Build an AccessibleProxy for the child.
            // child_ref has .name() (bus destination) and .path()
            // (D-Bus object path). Use .ok() chaining since we
            // are not in a Result-returning fn.
            let child_proxy = match child_ref
                .name()
                .cloned()
                .and_then(|n| {
                    ::atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
                        .destination(n)
                        .ok()
                })
                .and_then(|b| b.path(child_ref.path()).ok())
            {
                Some(builder) => match builder.build().await {
                    Ok(p) => p,
                    Err(_) => continue, // Skip inaccessible children
                },
                None => continue, // Skip if dest/path invalid
            };

            let child_elements = Box::pin(traverse_tree(
                conn,
                &child_proxy,
                depth + 1,
                max_depth,
                remaining,
                pii_level,
            ))
            .await;
            results.extend(child_elements);
        }
    }

    results
}

// ── ComponentProxy bounds ─────────────────────────────────────────────────────

/// Extract the bounding rectangle for an element via `ComponentProxy`.
///
/// Returns `None` if the element does not support the Component
/// interface or if the extents query fails.
#[cfg(feature = "linux-atspi")]
pub(super) async fn get_element_bounds(
    conn: &::atspi::connection::AccessibilityConnection,
    proxy: &::atspi::proxy::accessible::AccessibleProxy<'_>,
) -> Option<ElementRect> {
    use atspi_common::CoordType;

    // Query the Component interface for extents.
    // AccessibleProxy wraps a zbus Proxy; we extract its
    // destination and path to build a ComponentProxy for the same
    // D-Bus object.
    let inner_proxy = proxy.inner();
    let dest = inner_proxy.destination().to_string();
    let path = inner_proxy.path().to_string();

    let component = ::atspi::proxy::component::ComponentProxy::builder(conn.connection())
        .destination(dest.as_str())
        .ok()?
        .path(path.as_str())
        .ok()?
        .build()
        .await
        .ok()?;

    let (x, y, w, h) = component.get_extents(CoordType::Screen).await.ok()?;

    // Filter out zero-sized or off-screen elements
    if w <= 0 || h <= 0 {
        return None;
    }

    Some(ElementRect {
        x: x as f32,
        y: y as f32,
        width: w as f32,
        height: h as f32,
    })
}

// ── Focused element helpers ───────────────────────────────────────────────────

/// Walk immediate children of the active window looking for `State::Focused`.
///
/// Returns owned `FocusedElementInfo` (not a proxy) to avoid lifetime issues.
/// Uses the same proxy-building pattern as `traverse_tree`.
#[cfg(feature = "linux-atspi")]
pub(super) async fn find_focused_in_window(
    conn: &::atspi::connection::AccessibilityConnection,
    window: &::atspi::proxy::accessible::AccessibleProxy<'_>,
    pii_level: PiiFilterLevel,
) -> Option<FocusedElementInfo> {
    use atspi_common::State;

    // Check if the window itself is focused
    if let Ok(states) = window.get_state().await {
        if states.contains(State::Focused) {
            return proxy_to_focused_info(conn, window, pii_level).await;
        }
    }

    // Walk immediate children (shallow -- O(children) not O(tree))
    let children = window.get_children().await.ok()?;
    for child_ref in &children {
        let child_proxy = match child_ref
            .name()
            .cloned()
            .and_then(|n| {
                ::atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
                    .destination(n)
                    .ok()
            })
            .and_then(|b| b.path(child_ref.path()).ok())
        {
            Some(builder) => match builder.build().await {
                Ok(p) => p,
                Err(_) => continue,
            },
            None => continue,
        };

        if let Ok(states) = child_proxy.get_state().await {
            if states.contains(State::Focused) {
                return proxy_to_focused_info(conn, &child_proxy, pii_level).await;
            }
        }
    }

    None
}

/// Extract `FocusedElementInfo` from an `AccessibleProxy`.
///
/// Extracts role, label (suppressed at Strict PII level), and bounds.
/// Returns owned data so the proxy can be dropped afterward.
#[cfg(feature = "linux-atspi")]
pub(super) async fn proxy_to_focused_info(
    conn: &::atspi::connection::AccessibilityConnection,
    proxy: &::atspi::proxy::accessible::AccessibleProxy<'_>,
    pii_level: PiiFilterLevel,
) -> Option<FocusedElementInfo> {
    use atspi_common::Role;

    // Explicit type annotation avoids E0282 inference errors with zbus 5.x proxy methods
    let role_result: Result<Role, _> = proxy.get_role().await;
    let role = role_result
        .map(|r| format!("{r:?}"))
        .unwrap_or_else(|_| "Unknown".to_string());

    let label = if pii_level != PiiFilterLevel::Strict {
        let name: String = proxy.name().await.unwrap_or_default();
        Some(name)
    } else {
        None
    };

    let position = get_element_bounds(conn, proxy).await;

    Some(FocusedElementInfo {
        role,
        position,
        label,
        value_length: None,
        extracted_text: None,
    })
}
