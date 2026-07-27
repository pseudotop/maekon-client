//! Foreground external-window fullscreen detection port (#8849).
//!
//! For the overlay fullscreen suppression policy (CRT-PRV-OVL-005), this is the
//! contract for detecting whether a foreground **external** window (a window this
//! application does not own — a fullscreen game, browser video, presentation, etc.)
//! is fullscreen (covers the monitor). It is used to keep an interactive overlay
//! from being shown over, or stealing focus from, a fullscreen external app.
//!
//! Platform-specific implementations live in `maekon-monitor` (Win32
//! `GetForegroundWindow` + monitor rect comparison, macOS CGWindowList frontmost +
//! display bounds comparison, Linux X11 `_NET_WM_STATE_FULLSCREEN`). Excluding
//! self-owned windows to make the "external" determination is the implementation's
//! responsibility.

/// Queries the fullscreen / monitor-covering status of the foreground external window.
pub trait ForegroundFullscreenProbe: Send + Sync {
    /// Return value:
    /// - `Some(true)` — a non-owned foreground window is fullscreen or covers the
    ///   monitor.
    /// - `Some(false)` — a foreground window exists but is not fullscreen, or that
    ///   window is owned by this application (excluded).
    /// - `None` — the status cannot be determined (unsupported platform, no X server,
    ///   no permission, or no foreground window).
    fn foreground_is_fullscreen(&self) -> Option<bool>;
}
