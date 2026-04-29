//! OS accessibility API integration for focused element extraction.
//!
//! Platform-dispatched module:
//! - macOS: Native AXUIElement FFI (`macos.rs`)
//! - Windows: UIAutomation COM API (`windows.rs`)
//! - Linux: AT-SPI2 over D-Bus (`linux.rs`, feature `linux-atspi`)
//!
//! The `create_extractor()` factory function returns the appropriate
//! platform implementation behind `Arc<dyn AccessibilityExtractor>`.

#[cfg(target_os = "macos")]
pub(crate) mod ffi_macos;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

use std::sync::Arc;

use oneshim_core::ports::accessibility::AccessibilityExtractor;

#[cfg(target_os = "macos")]
pub use macos::MacOsNativeAccessibility;

#[cfg(target_os = "macos")]
pub use macos::FocusObserverHandle;

#[cfg(target_os = "windows")]
pub use windows::WindowsUiaAccessibility;

#[cfg(target_os = "linux")]
pub use linux::LinuxAccessibility;

/// Create the platform-appropriate accessibility extractor.
///
/// Returns `None` on unsupported platforms or if the platform module
/// is unavailable.
pub fn create_extractor() -> Option<Arc<dyn AccessibilityExtractor>> {
    create_platform_extractor()
}

#[cfg(target_os = "macos")]
fn create_platform_extractor() -> Option<Arc<dyn AccessibilityExtractor>> {
    let extractor = MacOsNativeAccessibility::new();
    if extractor.has_permission() {
        tracing::info!("macOS Accessibility permission granted; native extractor enabled");
    } else {
        tracing::warn!(
            "macOS Accessibility permission not granted; \
             accessibility extraction will return None on each tick. \
             Grant permission in System Settings > Privacy & Security > Accessibility."
        );
    }
    // Always return the extractor -- it will gracefully return None
    // on each call when permission is denied, and the circuit breaker
    // limits log noise.
    Some(Arc::new(extractor))
}

#[cfg(target_os = "windows")]
fn create_platform_extractor() -> Option<Arc<dyn AccessibilityExtractor>> {
    Some(Arc::new(WindowsUiaAccessibility::new()))
}

#[cfg(target_os = "linux")]
fn create_platform_extractor() -> Option<Arc<dyn AccessibilityExtractor>> {
    let extractor = LinuxAccessibility::new();
    #[cfg(feature = "linux-atspi")]
    tracing::info!("Linux AT-SPI2 accessibility extractor enabled");
    #[cfg(not(feature = "linux-atspi"))]
    tracing::warn!(
        "Linux AT-SPI2 feature not enabled — extractor will return None. \
         Rebuild with --features linux-atspi to enable full AT-SPI2 integration."
    );
    Some(Arc::new(extractor))
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn create_platform_extractor() -> Option<Arc<dyn AccessibilityExtractor>> {
    tracing::debug!("Accessibility extraction not available on this platform");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_extractor_returns_some_on_supported_platform() {
        // On macOS, Windows, and Linux, create_extractor should return Some.
        let extractor = create_extractor();
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        assert!(extractor.is_some());
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        assert!(extractor.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_extractor_name() {
        let extractor = create_extractor().expect("should return Some on macOS");
        assert_eq!(extractor.name(), "macos-native-accessibility");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_extractor_name() {
        let extractor = create_extractor().expect("should return Some on Windows");
        assert_eq!(extractor.name(), "windows-uia-accessibility");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_extractor_name() {
        let extractor = create_extractor().expect("should return Some on Linux");
        assert_eq!(extractor.name(), "linux-atspi2-accessibility");
    }
}
