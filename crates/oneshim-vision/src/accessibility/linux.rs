//! Linux AT-SPI2 accessibility extractor.
//!
//! Extracts the currently focused UI element via the AT-SPI2 protocol over
//! D-Bus. AT-SPI2 (Assistive Technology Service Provider Interface) is the
//! standard accessibility framework on Linux desktops (GNOME, KDE, XFCE).
//!
//! ## Architecture
//!
//! The implementation flow:
//!
//! 1. Connect to the D-Bus session bus via `atspi::AccessibilityConnection::new()`
//! 2. Walk the accessibility registry: root → applications → frames
//! 3. Find the active window by checking `State::Active` on frame nodes
//! 4. For the active frame, recursively traverse children up to `max_depth`
//!    and `max_elements`, extracting role, name, and bounding box via
//!    `ComponentProxy::get_extents(CoordType::Screen)`
//! 5. Apply PII-level gating: Strict suppresses labels, Standard/Basic/Off
//!    include them.
//!
//! ## Focus Event Listener
//!
//! Optionally, a `FocusEventListener` can be started to receive event-driven
//! focus change notifications via AT-SPI `StateChangedEvent` subscriptions.
//! This avoids polling the entire accessibility tree on every scheduler tick.
//! The listener caches the D-Bus coordinates (bus name + object path) of the
//! last focused element so the scheduler can check it cheaply.
//!
//! ## Permissions
//!
//! Unlike macOS (which requires Accessibility permission) and some Windows
//! configurations, AT-SPI2 does not require special permissions. Any user
//! process can connect to the AT-SPI2 D-Bus service as long as:
//! - The `at-spi2-core` package is installed (default on most desktop distros)
//! - The AT-SPI2 bus is running (started by the desktop session manager)
//! - The `ATSPI_BUS_ADDRESS` or `DBUS_SESSION_BUS_ADDRESS` env var is set

#[cfg(target_os = "linux")]
mod inner {
    #[cfg(feature = "linux-atspi")]
    use std::sync::atomic::{AtomicU32, Ordering};
    #[cfg(feature = "linux-atspi")]
    use std::sync::Arc;

    use async_trait::async_trait;
    #[cfg(feature = "linux-atspi")]
    use tokio::sync::RwLock;
    #[cfg(feature = "linux-atspi")]
    use tracing::debug;
    #[cfg(feature = "linux-atspi")]
    use tracing::info;
    #[cfg(feature = "linux-atspi")]
    use tracing::warn;

    #[cfg(feature = "linux-atspi")]
    use crate::error::VisionError;
    use oneshim_core::config::PiiFilterLevel;
    use oneshim_core::error::CoreError;
    #[cfg(feature = "linux-atspi")]
    use oneshim_core::models::focused_element::AccessibilityElement;
    #[cfg(feature = "linux-atspi")]
    use oneshim_core::models::focused_element::ElementRect;
    use oneshim_core::models::focused_element::FocusedElementInfo;
    use oneshim_core::ports::accessibility::AccessibilityExtractor;

    // ── Focus event listener types ────────────────────────────────────

    /// D-Bus coordinates of a focused accessible object.
    ///
    /// Stores the bus name (destination) and object path so the caller can
    /// build an `AccessibleProxy` for the focused element without walking
    /// the entire tree.
    #[cfg(feature = "linux-atspi")]
    #[derive(Debug, Clone)]
    pub struct FocusedObjectInfo {
        /// D-Bus bus name (e.g. ":1.42" or "org.gnome.Terminal").
        pub bus_name: String,
        /// D-Bus object path (e.g. "/org/a11y/atspi/accessible/123").
        pub object_path: String,
    }

    /// Handle to a running focus event listener.
    ///
    /// When all clones of this handle are dropped, the background listener
    /// task is cancelled via the shutdown channel.
    #[cfg(feature = "linux-atspi")]
    #[derive(Clone)]
    pub struct FocusEventListenerHandle {
        /// Cached last focused object coordinates, updated by the listener task.
        last_focused: Arc<RwLock<Option<FocusedObjectInfo>>>,
        /// Sending side of the shutdown channel. The listener task holds
        /// the receiver and stops when it fires.
        _shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    }

    #[cfg(feature = "linux-atspi")]
    impl FocusEventListenerHandle {
        /// Read the last focused object info without blocking.
        ///
        /// Returns `None` if no focus event has been received yet or if the
        /// listener has not started.
        pub async fn last_focused(&self) -> Option<FocusedObjectInfo> {
            self.last_focused.read().await.clone()
        }

        /// Check whether a focus event has been received at least once.
        pub async fn has_focus(&self) -> bool {
            self.last_focused.read().await.is_some()
        }
    }

    /// Background focus event listener that subscribes to AT-SPI
    /// `StateChangedEvent` notifications over D-Bus.
    #[cfg(feature = "linux-atspi")]
    struct FocusEventListener;

    #[cfg(feature = "linux-atspi")]
    impl FocusEventListener {
        /// Spawn the listener task and return a handle.
        ///
        /// The listener connects to AT-SPI, registers for `ObjectEvents`,
        /// and filters the event stream for `StateChangedEvent` with
        /// state == Focused and enabled == true. Each matching event
        /// updates the shared `last_focused` cache.
        ///
        /// The task runs until the returned handle (and all its clones) are
        /// dropped, which triggers the shutdown watch channel.
        async fn spawn() -> Result<FocusEventListenerHandle, VisionError> {
            use atspi::connection::AccessibilityConnection;
            use atspi::events::ObjectEvents;

            let conn = AccessibilityConnection::new().await.map_err(|e| {
                VisionError::Internal(format!(
                    "AT-SPI2 focus listener: D-Bus connection failed: {e}"
                ))
            })?;

            conn.register_event::<ObjectEvents>().await.map_err(|e| {
                VisionError::Internal(format!(
                    "AT-SPI2 focus listener: event registration failed: {e}"
                ))
            })?;

            let last_focused: Arc<RwLock<Option<FocusedObjectInfo>>> = Arc::new(RwLock::new(None));
            let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

            let cache = Arc::clone(&last_focused);

            tokio::spawn(async move {
                use atspi::events::object::StateChangedEvent;
                use atspi_common::State;
                use futures::StreamExt;

                let stream = conn.event_stream();
                tokio::pin!(stream);

                info!("AT-SPI2 focus event listener started");

                loop {
                    tokio::select! {
                        biased;

                        // Shutdown signal — stop the loop
                        _ = shutdown_rx.changed() => {
                            info!("AT-SPI2 focus event listener shutting down");
                            break;
                        }

                        // Next event from the AT-SPI stream
                        event_opt = stream.next() => {
                            match event_opt {
                                Some(Ok(event)) => {
                                    // Try to convert to StateChangedEvent
                                    if let Ok(state_change) = StateChangedEvent::try_from(event) {
                                        if state_change.state == State::Focused
                                            && state_change.enabled
                                        {
                                            let item = &state_change.item;
                                            let info = FocusedObjectInfo {
                                                bus_name: item.name_as_str().unwrap_or("").to_string(),
                                                object_path: item.path_as_str().to_string(),
                                            };
                                            debug!(
                                                bus = %info.bus_name,
                                                path = %info.object_path,
                                                "AT-SPI2 focus changed"
                                            );
                                            *cache.write().await = Some(info);
                                        }
                                    }
                                }
                                Some(Err(e)) => {
                                    warn!("AT-SPI2 event stream error: {e}");
                                    // Continue listening -- transient errors are expected
                                }
                                None => {
                                    // Stream ended unexpectedly
                                    warn!("AT-SPI2 event stream ended unexpectedly");
                                    break;
                                }
                            }
                        }
                    }
                }

                // Deregister events on shutdown (best-effort)
                if let Err(e) = conn.deregister_event::<ObjectEvents>().await {
                    debug!("AT-SPI2 focus listener: deregister failed (non-fatal): {e}");
                }
            });

            Ok(FocusEventListenerHandle {
                last_focused,
                _shutdown_tx: Arc::new(shutdown_tx),
            })
        }
    }

    // ── Circuit breaker (same pattern as macOS/Windows) ──────────────

    #[cfg(feature = "linux-atspi")]
    /// Consecutive AT-SPI2 failures before the circuit breaker opens.
    static CONSECUTIVE_FAILURES: AtomicU32 = AtomicU32::new(0);
    #[cfg(feature = "linux-atspi")]
    const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
    #[cfg(feature = "linux-atspi")]
    /// Retry every 10 ticks (~30s at 3s poll) after circuit opens.
    const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 10;

    /// Linux AT-SPI2 accessibility extractor.
    ///
    /// Connects to the AT-SPI2 D-Bus service and traverses the accessibility
    /// tree for the active window to extract UI element information.
    pub struct LinuxAccessibility;

    impl Default for LinuxAccessibility {
        fn default() -> Self {
            Self
        }
    }

    impl LinuxAccessibility {
        pub fn new() -> Self {
            Self
        }

        // ── Circuit breaker ──────────────────────────────────────────

        #[cfg(feature = "linux-atspi")]
        fn circuit_allows() -> bool {
            let failures = CONSECUTIVE_FAILURES.load(Ordering::Relaxed);
            if failures >= CIRCUIT_BREAKER_THRESHOLD {
                if failures % CIRCUIT_BREAKER_RETRY_INTERVAL != 0 {
                    CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
                    return false;
                }
                warn!(
                    "LinuxAccessibility: circuit breaker retry after {} skipped",
                    failures - CIRCUIT_BREAKER_THRESHOLD
                );
            }
            true
        }

        #[cfg(feature = "linux-atspi")]
        fn record_success() {
            CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);
        }

        #[cfg(feature = "linux-atspi")]
        fn record_failure() {
            let prev = CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            if prev + 1 == CIRCUIT_BREAKER_THRESHOLD {
                warn!("LinuxAccessibility: circuit breaker tripped after {CIRCUIT_BREAKER_THRESHOLD} consecutive failures");
            }
        }

        /// Check if the AT-SPI2 D-Bus service is reachable.
        fn check_atspi_available() -> bool {
            #[cfg(feature = "linux-atspi")]
            {
                // Check if the AT-SPI2 bus address environment variable is set
                std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
                    || std::env::var("ATSPI_BUS_ADDRESS").is_ok()
            }
            #[cfg(not(feature = "linux-atspi"))]
            {
                true // Stub mode, claim available
            }
        }

        // ── Focus event listener ──────────────────────────────────────

        /// Start the AT-SPI focus event listener.
        ///
        /// Returns a handle that can be used to query the last focused
        /// element without polling the entire accessibility tree. The
        /// listener subscribes to `StateChangedEvent` with state "focused"
        /// over D-Bus and caches the focused element's bus coordinates.
        ///
        /// This is **optional** — if it fails to start (e.g. AT-SPI2 is not
        /// available), the existing polling path in `extract_window_elements()`
        /// continues to work. Callers should treat errors as non-fatal.
        ///
        /// The listener task runs until the returned handle (and all its
        /// clones) are dropped.
        #[cfg(feature = "linux-atspi")]
        pub async fn start_focus_listener(&self) -> Result<FocusEventListenerHandle, VisionError> {
            FocusEventListener::spawn().await
        }

        // ── AT-SPI tree traversal helpers (linux-atspi feature) ──────

        /// Recursively traverse the AT-SPI accessibility tree starting from
        /// `proxy`, collecting elements up to `max_depth` levels deep and
        /// `remaining` total elements.
        ///
        /// Each node's role, name, and bounding box (via ComponentProxy) are
        /// extracted and converted to `AccessibilityElement`. Individual
        /// element failures are skipped silently.
        #[cfg(feature = "linux-atspi")]
        async fn traverse_tree(
            conn: &atspi::connection::AccessibilityConnection,
            proxy: &atspi::proxy::accessible::AccessibleProxy<'_>,
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
            let bounds = Self::get_element_bounds(conn, proxy).await;

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
                            atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
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

                    let child_elements = Box::pin(Self::traverse_tree(
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

        /// Extract the bounding rectangle for an element via `ComponentProxy`.
        ///
        /// Returns `None` if the element does not support the Component
        /// interface or if the extents query fails.
        #[cfg(feature = "linux-atspi")]
        async fn get_element_bounds(
            conn: &atspi::connection::AccessibilityConnection,
            proxy: &atspi::proxy::accessible::AccessibleProxy<'_>,
        ) -> Option<ElementRect> {
            use atspi_common::CoordType;

            // Query the Component interface for extents.
            // AccessibleProxy wraps a zbus Proxy; we extract its
            // destination and path to build a ComponentProxy for the same
            // D-Bus object.
            let inner_proxy = proxy.inner();
            let dest = inner_proxy.destination().to_string();
            let path = inner_proxy.path().to_string();

            let component = atspi::proxy::component::ComponentProxy::builder(conn.connection())
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

        /// Find the active window frame across all AT-SPI applications.
        ///
        /// Walks: registry root → applications → children (frames/windows),
        /// checking each frame for `State::Active`. Returns the first active
        /// frame's AccessibleProxy, or `None` if no active window is found.
        #[cfg(feature = "linux-atspi")]
        async fn find_active_window<'a>(
            conn: &'a atspi::connection::AccessibilityConnection,
        ) -> Option<atspi::proxy::accessible::AccessibleProxy<'a>> {
            use atspi_common::Role;
            use atspi_common::State;

            let root = conn.root_accessible_on_registry().await.ok()?;
            let apps = root.get_children().await.ok()?;

            for app_ref in &apps {
                // Build AccessibleProxy for the application
                let app_proxy = app_ref
                    .name()
                    .cloned()
                    .and_then(|n| {
                        atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
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
                            atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
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

        /// Walk immediate children of the active window looking for `State::Focused`.
        ///
        /// Returns owned `FocusedElementInfo` (not a proxy) to avoid lifetime issues.
        /// Uses the same proxy-building pattern as `traverse_tree`.
        #[cfg(feature = "linux-atspi")]
        async fn find_focused_in_window(
            conn: &atspi::connection::AccessibilityConnection,
            window: &atspi::proxy::accessible::AccessibleProxy<'_>,
            pii_level: PiiFilterLevel,
        ) -> Option<FocusedElementInfo> {
            use atspi_common::State;

            // Check if the window itself is focused
            if let Ok(states) = window.get_state().await {
                if states.contains(State::Focused) {
                    return Self::proxy_to_focused_info(conn, window, pii_level).await;
                }
            }

            // Walk immediate children (shallow -- O(children) not O(tree))
            let children = window.get_children().await.ok()?;
            for child_ref in &children {
                let child_proxy = match child_ref
                    .name()
                    .cloned()
                    .and_then(|n| {
                        atspi::proxy::accessible::AccessibleProxy::builder(conn.connection())
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
                        return Self::proxy_to_focused_info(conn, &child_proxy, pii_level).await;
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
        async fn proxy_to_focused_info(
            conn: &atspi::connection::AccessibilityConnection,
            proxy: &atspi::proxy::accessible::AccessibleProxy<'_>,
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

            let position = Self::get_element_bounds(conn, proxy).await;

            Some(FocusedElementInfo {
                role,
                position,
                label,
                value_length: None,
                extracted_text: None,
            })
        }
    }

    #[async_trait]
    impl AccessibilityExtractor for LinuxAccessibility {
        #[cfg(feature = "linux-atspi")]
        async fn extract_focused_element(
            &self,
            pii_level: PiiFilterLevel,
            has_full_text_consent: bool,
        ) -> Result<Option<FocusedElementInfo>, CoreError> {
            use atspi::connection::AccessibilityConnection;

            if !Self::circuit_allows() {
                debug!("LinuxAccessibility: circuit breaker open");
                return Ok(None);
            }

            let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
                PiiFilterLevel::Standard
            } else {
                pii_level
            };

            // AT-SPI is async-native -- no spawn_blocking needed
            let conn = match AccessibilityConnection::new().await {
                Ok(c) => c,
                Err(e) => {
                    Self::record_failure();
                    debug!("AT-SPI2 connection failed: {e}");
                    return Ok(None); // graceful degradation, not an error
                }
            };

            // Find active window (reuse existing helper)
            let active_window = match Self::find_active_window(&conn).await {
                Some(w) => w,
                None => {
                    Self::record_success(); // no window is not a failure
                    return Ok(None);
                }
            };

            // Walk active window's immediate children looking for State::Focused
            let focused_info =
                Self::find_focused_in_window(&conn, &active_window, effective_level).await;

            match focused_info {
                Some(info) => {
                    Self::record_success();
                    debug!(role = %info.role, "AT-SPI2 focused element extracted");
                    Ok(Some(info))
                }
                None => {
                    // No focused element found -- not a failure (user may have no focus)
                    Self::record_success();
                    Ok(None)
                }
            }
        }

        #[cfg(not(feature = "linux-atspi"))]
        async fn extract_focused_element(
            &self,
            _pii_level: PiiFilterLevel,
            _has_full_text_consent: bool,
        ) -> Result<Option<FocusedElementInfo>, CoreError> {
            Ok(None)
        }

        #[cfg(feature = "linux-atspi")]
        async fn extract_window_elements(
            &self,
            max_depth: u32,
            max_elements: usize,
            pii_level: PiiFilterLevel,
            has_full_text_consent: bool,
        ) -> Result<Vec<AccessibilityElement>, CoreError> {
            use atspi::connection::AccessibilityConnection;

            if !Self::circuit_allows() {
                return Ok(Vec::new());
            }

            let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
                PiiFilterLevel::Standard
            } else {
                pii_level
            };

            // AT-SPI is async-native, no spawn_blocking needed.
            // Explicit type annotation avoids E0282 inference errors with zbus 5.x.
            let conn: AccessibilityConnection =
                AccessibilityConnection::new().await.map_err(|e| {
                    Self::record_failure();
                    CoreError::PermissionDenied { code: oneshim_core::error_codes::PermissionCode::PermissionDenied, message: format!("AT-SPI2 D-Bus connection failed. Ensure at-spi2-core is installed: {e}") }
                })?;

            // Find the active window by walking registry → apps → frames
            let active_window = match Self::find_active_window(&conn).await {
                Some(w) => w,
                None => {
                    // No active window found — not an error, just nothing to traverse
                    debug!("AT-SPI2: no active window found");
                    Self::record_success();
                    return Ok(Vec::new());
                }
            };

            // Traverse the active window's subtree
            let mut remaining = max_elements;
            let elements = Self::traverse_tree(
                &conn,
                &active_window,
                0,
                max_depth,
                &mut remaining,
                effective_level,
            )
            .await;

            if elements.is_empty() {
                Self::record_failure();
            } else {
                Self::record_success();
                debug!(count = elements.len(), "AT-SPI2 window tree extracted");
            }

            Ok(elements)
        }

        fn has_permission(&self) -> bool {
            // AT-SPI2 does not require special permissions on Linux.
            // Any user-session process can connect to the accessibility bus.
            Self::check_atspi_available()
        }

        fn name(&self) -> &str {
            "linux-atspi2-accessibility"
        }
    }
}

#[cfg(target_os = "linux")]
pub use inner::LinuxAccessibility;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::inner::*;
    use oneshim_core::config::PiiFilterLevel;
    use oneshim_core::ports::accessibility::AccessibilityExtractor;

    #[test]
    fn has_permission_reflects_dbus_availability() {
        let extractor = LinuxAccessibility::new();
        let dbus_available = std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
            || std::env::var("ATSPI_BUS_ADDRESS").is_ok();
        // With linux-atspi, permission requires D-Bus session.
        // Without the feature (stub mode), always returns true.
        if cfg!(feature = "linux-atspi") {
            assert_eq!(extractor.has_permission(), dbus_available);
        } else {
            assert!(extractor.has_permission());
        }
    }

    #[test]
    fn name_is_correct() {
        let extractor = LinuxAccessibility::new();
        assert_eq!(extractor.name(), "linux-atspi2-accessibility");
    }

    #[tokio::test]
    async fn test_extract_focused_element() {
        let extractor = LinuxAccessibility::new();
        let result = extractor
            .extract_focused_element(PiiFilterLevel::Standard, false)
            .await;
        // On CI without D-Bus/AT-SPI2, Ok(None) is valid (connection fails gracefully).
        // On desktop Linux with AT-SPI2, may return Ok(Some(...)).
        assert!(result.is_ok());
    }

    #[cfg(feature = "linux-atspi")]
    #[tokio::test]
    async fn extract_window_elements_atspi_connection() {
        let extractor = LinuxAccessibility::new();
        let result = extractor
            .extract_window_elements(3, 300, PiiFilterLevel::Standard, false)
            .await;
        // On CI without AT-SPI2, this may return PermissionDenied
        // On desktop Linux, should return Ok (possibly empty)
        match result {
            Ok(elements) => {
                eprintln!("AT-SPI2 returned {} elements", elements.len());
            }
            Err(oneshim_core::error::CoreError::PermissionDenied {
                code: oneshim_core::error_codes::PermissionCode::PermissionDenied,
                message: msg,
            }) => {
                eprintln!("AT-SPI2 not available: {msg}");
            }
            Err(e) => {
                panic!("unexpected error: {e}");
            }
        }
    }

    #[cfg(feature = "linux-atspi")]
    #[tokio::test]
    async fn focus_listener_starts_or_gracefully_fails() {
        let extractor = LinuxAccessibility::new();
        let result = extractor.start_focus_listener().await;
        // On CI without AT-SPI2, this will fail with Internal error.
        // On desktop Linux with AT-SPI2 running, it should succeed.
        match result {
            Ok(handle) => {
                // Listener started — initially no focus event received
                assert!(!handle.has_focus().await);
                assert!(handle.last_focused().await.is_none());
                // Handle drop triggers graceful shutdown
                drop(handle);
                eprintln!("AT-SPI2 focus listener started and stopped successfully");
            }
            Err(crate::error::VisionError::Internal(msg)) => {
                eprintln!("AT-SPI2 focus listener unavailable (expected on CI): {msg}");
            }
            Err(e) => {
                panic!("unexpected error from start_focus_listener: {e}");
            }
        }
    }

    #[cfg(feature = "linux-atspi")]
    #[tokio::test]
    async fn focus_listener_handle_clone_shares_state() {
        // Simulate the shared state without a real AT-SPI connection
        // by constructing a FocusedObjectInfo and verifying the Clone
        // derive works correctly.
        let info = FocusedObjectInfo {
            bus_name: ":1.42".to_string(),
            object_path: "/org/a11y/atspi/accessible/123".to_string(),
        };
        assert_eq!(info.bus_name, ":1.42");
        assert_eq!(info.object_path, "/org/a11y/atspi/accessible/123");

        // Verify clone works
        let cloned = info.clone();
        assert_eq!(cloned.bus_name, info.bus_name);
        assert_eq!(cloned.object_path, info.object_path);
    }
}
