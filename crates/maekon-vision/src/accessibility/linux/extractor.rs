//! AT-SPI2 tree traversal helpers.
//!
//! Low-level functions that walk the accessibility tree using D-Bus proxies:
//! - `find_active_window` — registry root → apps → frames, State::Active check
//! - `traverse_tree`     — recursive depth-limited element collector
//! - `get_element_bounds`— ComponentProxy extents query
//! - `find_focused_in_window` — shallow focused-child scan
//! - `proxy_to_focused_info`  — proxy → owned `FocusedElementInfo`
//!
//! ## Timeout policy
//!
//! Every D-Bus round-trip is guarded by `DBUS_CALL_TIMEOUT` (2 s). If a call
//! exceeds the budget (frozen daemon, crashed socket, deadlocked Electron a11y
//! responder), it is treated as a transient failure: the call site skips that
//! element and continues. `find_active_window` additionally wraps its entire
//! body in `FIND_ACTIVE_WINDOW_TIMEOUT` (3 s); on expiry a `warn!` is emitted
//! and `None` is returned so the 1 s scheduler loop is never stalled.

#[cfg(feature = "linux-atspi")]
use std::time::Duration;

#[cfg(feature = "linux-atspi")]
use maekon_core::config::PiiFilterLevel;
#[cfg(feature = "linux-atspi")]
use maekon_core::models::focused_element::{AccessibilityElement, ElementRect, FocusedElementInfo};
#[cfg(feature = "linux-atspi")]
use tracing::warn;

// ── Timeout budgets ───────────────────────────────────────────────────────────

/// Per-call D-Bus round-trip budget.
///
/// A frozen atspi-registryd or deadlocked Electron accessibility responder can
/// stall indefinitely without this guard. Two seconds is generous enough for a
/// healthy daemon yet tight enough to protect the 1 s scheduler tick.
#[cfg(feature = "linux-atspi")]
const DBUS_CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// Whole-function budget for `find_active_window`.
///
/// The function iterates over all running applications, so even with per-call
/// timeouts the total wall time can accumulate. Three seconds is the hard cap;
/// on expiry the function returns `None` gracefully.
#[cfg(feature = "linux-atspi")]
const FIND_ACTIVE_WINDOW_TIMEOUT: Duration = Duration::from_secs(3);

// ── Active window ─────────────────────────────────────────────────────────────

/// Find the active window frame across all AT-SPI applications.
///
/// Walks: registry root → applications → children (frames/windows),
/// checking each frame for `State::Active`. Returns the first active
/// frame's `AccessibleProxy`, or `None` if no active window is found or if
/// the whole-function timeout (`FIND_ACTIVE_WINDOW_TIMEOUT`) is exceeded.
///
/// Individual D-Bus round-trips are capped at `DBUS_CALL_TIMEOUT`; a stalled
/// call is treated as a transient failure for that element — the loop
/// continues to the next application or child rather than blocking.
#[cfg(feature = "linux-atspi")]
pub(super) async fn find_active_window<'a>(
    conn: &'a ::atspi::connection::AccessibilityConnection,
) -> Option<::atspi::proxy::accessible::AccessibleProxy<'a>> {
    match tokio::time::timeout(FIND_ACTIVE_WINDOW_TIMEOUT, find_active_window_inner(conn)).await {
        Ok(result) => result,
        Err(_elapsed) => {
            warn!(
                timeout_secs = FIND_ACTIVE_WINDOW_TIMEOUT.as_secs(),
                "AT-SPI2 find_active_window timed out — frozen daemon or deadlocked a11y responder; \
                 returning None to protect the scheduler loop"
            );
            None
        }
    }
}

/// Inner (unbounded) implementation of `find_active_window`.
///
/// Called exclusively from `find_active_window`, which wraps it in the
/// whole-function timeout. Every individual D-Bus call here is also wrapped in
/// `DBUS_CALL_TIMEOUT` so a single stalled app does not consume the entire
/// budget.
#[cfg(feature = "linux-atspi")]
async fn find_active_window_inner<'a>(
    conn: &'a ::atspi::connection::AccessibilityConnection,
) -> Option<::atspi::proxy::accessible::AccessibleProxy<'a>> {
    use atspi_common::{Role, State};

    let root = tokio::time::timeout(DBUS_CALL_TIMEOUT, conn.root_accessible_on_registry())
        .await
        .ok()? // Err(_) = timeout
        .ok()?; // Err(_) = D-Bus error

    let apps = tokio::time::timeout(DBUS_CALL_TIMEOUT, root.get_children())
        .await
        .ok()?
        .ok()?;

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
            Some(b) => {
                match tokio::time::timeout(DBUS_CALL_TIMEOUT, b.build()).await {
                    Ok(Ok(p)) => p,
                    Ok(Err(_)) | Err(_) => continue, // D-Bus error or timeout
                }
            }
            None => continue,
        };

        let children = match tokio::time::timeout(DBUS_CALL_TIMEOUT, app_proxy.get_children()).await
        {
            Ok(Ok(c)) => c,
            Ok(Err(_)) | Err(_) => continue,
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
                Some(builder) => {
                    match tokio::time::timeout(DBUS_CALL_TIMEOUT, builder.build()).await {
                        Ok(Ok(p)) => p,
                        Ok(Err(_)) | Err(_) => continue,
                    }
                }
                None => continue,
            };

            // Check if this is a frame/window with Active state
            let role = match tokio::time::timeout(DBUS_CALL_TIMEOUT, child_proxy.get_role()).await {
                Ok(Ok(r)) => r,
                Ok(Err(_)) | Err(_) => continue,
            };

            if !matches!(role, Role::Frame | Role::Window | Role::Dialog) {
                continue;
            }

            // Check the state set for Active
            let states =
                match tokio::time::timeout(DBUS_CALL_TIMEOUT, child_proxy.get_state()).await {
                    Ok(Ok(s)) => s,
                    Ok(Err(_)) | Err(_) => continue,
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

    // Extract role as a string. Guard with per-call timeout — a stalled
    // D-Bus proxy causes this to return "Unknown" rather than blocking.
    let role_str = match tokio::time::timeout(DBUS_CALL_TIMEOUT, proxy.get_role()).await {
        Ok(Ok(role)) => format!("{role:?}"),
        Ok(Err(_)) | Err(_) => "Unknown".to_string(),
    };

    // Extract name/label (suppress at Strict PII level)
    let label = if pii_level != PiiFilterLevel::Strict {
        match tokio::time::timeout(DBUS_CALL_TIMEOUT, proxy.name()).await {
            Ok(name) => name.unwrap_or_default(),
            Err(_elapsed) => String::new(), // timeout — skip label, do not block
        }
    } else {
        String::new()
    };

    // Extract bounding box via ComponentProxy
    let bounds = get_element_bounds(conn, proxy).await;

    results.push(AccessibilityElement {
        role: role_str,
        // Mask the accessibility name at the configured level (review4 V15); Strict
        // already yields an empty label, Off is an identity pass.
        label: crate::privacy::sanitize_title_with_level(&label, pii_level),
        bounds,
    });
    *remaining = remaining.saturating_sub(1);

    // Recurse into children
    if depth < max_depth && *remaining > 0 {
        // get_children() returns Vec<(destination, object_path)>
        // representing child accessible objects on the D-Bus.
        let children = match tokio::time::timeout(DBUS_CALL_TIMEOUT, proxy.get_children()).await {
            Ok(Ok(c)) => c,
            Ok(Err(_)) | Err(_) => return results,
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
                Some(builder) => {
                    match tokio::time::timeout(DBUS_CALL_TIMEOUT, builder.build()).await {
                        Ok(Ok(p)) => p,
                        Ok(Err(_)) | Err(_) => continue, // Skip inaccessible or timed-out children
                    }
                }
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

    let component = tokio::time::timeout(
        DBUS_CALL_TIMEOUT,
        ::atspi::proxy::component::ComponentProxy::builder(conn.connection())
            .destination(dest.as_str())
            .ok()?
            .path(path.as_str())
            .ok()?
            .build(),
    )
    .await
    .ok()? // timeout → None
    .ok()?; // build error → None

    let (x, y, w, h) =
        tokio::time::timeout(DBUS_CALL_TIMEOUT, component.get_extents(CoordType::Screen))
            .await
            .ok()? // timeout → None
            .ok()?; // D-Bus error → None

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
    if let Ok(Ok(states)) = tokio::time::timeout(DBUS_CALL_TIMEOUT, window.get_state()).await {
        if states.contains(State::Focused) {
            return proxy_to_focused_info(conn, window, pii_level).await;
        }
    }

    // Walk immediate children (shallow -- O(children) not O(tree))
    let children = tokio::time::timeout(DBUS_CALL_TIMEOUT, window.get_children())
        .await
        .ok()? // timeout → None
        .ok()?; // D-Bus error → None
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
            Some(builder) => match tokio::time::timeout(DBUS_CALL_TIMEOUT, builder.build()).await {
                Ok(Ok(p)) => p,
                Ok(Err(_)) | Err(_) => continue,
            },
            None => continue,
        };

        if let Ok(Ok(states)) =
            tokio::time::timeout(DBUS_CALL_TIMEOUT, child_proxy.get_state()).await
        {
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

    // Explicit type annotation avoids E0282 inference errors with zbus 5.x proxy methods.
    // Per-call timeout prevents a frozen proxy from stalling the scheduler.
    // On timeout the role degrades to "Unknown"; the element is still returned.
    //
    // The inline async block carries the `Result<Role, _>` annotation so the
    // compiler resolves the generic — mirrors the original `let role_result: Result<Role, _>`
    // annotation trick, now wrapped in a timeout.
    let role: String = match tokio::time::timeout(DBUS_CALL_TIMEOUT, async {
        let r: Result<Role, _> = proxy.get_role().await;
        r
    })
    .await
    {
        Ok(Ok(r)) => format!("{r:?}"),
        Ok(Err(_)) | Err(_) => "Unknown".to_string(),
    };

    let label = if pii_level != PiiFilterLevel::Strict {
        let name: String = match tokio::time::timeout(DBUS_CALL_TIMEOUT, proxy.name()).await {
            Ok(n) => n.unwrap_or_default(),
            Err(_elapsed) => String::new(), // timeout — omit label
        };
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
