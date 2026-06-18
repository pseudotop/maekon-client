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
//! Optionally, a `FocusEventListenerHandle` can be started to receive
//! event-driven focus change notifications via AT-SPI `StateChangedEvent`
//! subscriptions. This avoids polling the entire accessibility tree on every
//! scheduler tick. The listener caches the D-Bus coordinates (bus name +
//! object path) of the last focused element so the scheduler can check it
//! cheaply.
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
pub mod atspi;
#[cfg(target_os = "linux")]
mod circuit;
#[cfg(target_os = "linux")]
mod extractor;

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests;

// Re-export public event-listener types so callers can use the flat path
// `crate::accessibility::linux::FocusEventListenerHandle`.
#[cfg(target_os = "linux")]
#[cfg(feature = "linux-atspi")]
pub use atspi::FocusEventListenerHandle;

use async_trait::async_trait;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::focused_element::FocusedElementInfo;
use maekon_core::ports::accessibility::AccessibilityExtractor;

#[cfg(feature = "linux-atspi")]
use maekon_core::models::focused_element::AccessibilityElement;
#[cfg(feature = "linux-atspi")]
use tracing::debug;

#[cfg(feature = "linux-atspi")]
use crate::error::VisionError;

#[cfg(feature = "linux-atspi")]
use std::time::Duration;

/// Timeout budget for establishing an AT-SPI2 `AccessibilityConnection`.
///
/// `AccessibilityConnection::new()` performs a D-Bus `Hello` handshake against
/// the session bus. A wedged `dbus-daemon` (or a session bus that never
/// finishes authenticating) makes this await hang indefinitely, which would
/// stall the 1 s scheduler tick — the very failure mode the per-call
/// `DBUS_CALL_TIMEOUT` guards in `extractor.rs` exist to prevent. We mirror
/// that invariant here so connection (re)establishment is also bounded. Three
/// seconds is generous for a healthy bus yet tight enough to protect the loop.
#[cfg(feature = "linux-atspi")]
const ATSPI_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

// ── Public extractor struct ───────────────────────────────────────────────────

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
        atspi::FocusEventListener::spawn().await
    }
}

// ── AccessibilityExtractor impl ───────────────────────────────────────────────

#[async_trait]
impl AccessibilityExtractor for LinuxAccessibility {
    #[cfg(feature = "linux-atspi")]
    async fn extract_focused_element(
        &self,
        pii_level: PiiFilterLevel,
        has_full_text_consent: bool,
    ) -> Result<Option<FocusedElementInfo>, CoreError> {
        use ::atspi::connection::AccessibilityConnection;

        if !circuit::circuit_allows() {
            debug!("LinuxAccessibility: circuit breaker open");
            return Ok(None);
        }

        let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
            PiiFilterLevel::Standard
        } else {
            pii_level
        };

        // AT-SPI is async-native — no spawn_blocking needed, but the D-Bus
        // Hello handshake must still be timeout-guarded so a wedged dbus-daemon
        // cannot stall the scheduler tick (mirrors DBUS_CALL_TIMEOUT).
        let conn = match tokio::time::timeout(ATSPI_CONNECT_TIMEOUT, AccessibilityConnection::new())
            .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => {
                circuit::record_failure();
                debug!("AT-SPI2 connection failed: {e}");
                return Ok(None); // graceful degradation, not an error
            }
            Err(_elapsed) => {
                circuit::record_failure();
                debug!(
                    timeout_secs = ATSPI_CONNECT_TIMEOUT.as_secs(),
                    "AT-SPI2 connection timed out — wedged dbus-daemon; \
                     returning None to protect the scheduler loop"
                );
                return Ok(None); // graceful degradation, not an error
            }
        };

        let active_window = match extractor::find_active_window(&conn).await {
            Some(w) => w,
            None => {
                circuit::record_success(); // no window is not a failure
                return Ok(None);
            }
        };

        let focused_info =
            extractor::find_focused_in_window(&conn, &active_window, effective_level).await;

        match focused_info {
            Some(info) => {
                circuit::record_success();
                debug!(role = %info.role, "AT-SPI2 focused element extracted");
                Ok(Some(info))
            }
            None => {
                circuit::record_success();
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
        use ::atspi::connection::AccessibilityConnection;

        if !circuit::circuit_allows() {
            return Ok(Vec::new());
        }

        let effective_level = if pii_level == PiiFilterLevel::Off && !has_full_text_consent {
            PiiFilterLevel::Standard
        } else {
            pii_level
        };

        // AT-SPI is async-native, no spawn_blocking needed. The D-Bus Hello
        // handshake is timeout-guarded (mirrors DBUS_CALL_TIMEOUT) so a wedged
        // dbus-daemon cannot stall the scheduler tick.
        // Explicit type annotation avoids E0282 inference errors with zbus 5.x.
        let conn: AccessibilityConnection =
            match tokio::time::timeout(ATSPI_CONNECT_TIMEOUT, AccessibilityConnection::new()).await
            {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => {
                    circuit::record_failure();
                    return Err(CoreError::PermissionDenied {
                        code: maekon_core::error_codes::PermissionCode::PermissionDenied,
                        message: format!(
                            "AT-SPI2 D-Bus connection failed. \
                             Ensure at-spi2-core is installed: {e}"
                        ),
                    });
                }
                Err(_elapsed) => {
                    circuit::record_failure();
                    debug!(
                        timeout_secs = ATSPI_CONNECT_TIMEOUT.as_secs(),
                        "AT-SPI2 connection timed out — wedged dbus-daemon; \
                         returning empty to protect the scheduler loop"
                    );
                    return Ok(Vec::new()); // graceful degradation, not an error
                }
            };

        let active_window = match extractor::find_active_window(&conn).await {
            Some(w) => w,
            None => {
                debug!("AT-SPI2: no active window found");
                circuit::record_success();
                return Ok(Vec::new());
            }
        };

        let mut remaining = max_elements;
        let elements = extractor::traverse_tree(
            &conn,
            &active_window,
            0,
            max_depth,
            &mut remaining,
            effective_level,
        )
        .await;

        if elements.is_empty() {
            circuit::record_failure();
        } else {
            circuit::record_success();
            debug!(count = elements.len(), "AT-SPI2 window tree extracted");
        }

        Ok(elements)
    }

    fn has_permission(&self) -> bool {
        // AT-SPI2 does not require special permissions on Linux.
        // Any user-session process can connect to the accessibility bus.
        circuit::check_atspi_available()
    }

    fn name(&self) -> &str {
        "linux-atspi2-accessibility"
    }
}
