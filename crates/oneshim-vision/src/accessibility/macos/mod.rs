//! macOS native accessibility extractor using AXUIElement FFI.
//!
//! Replaces the osascript-based stub with direct Core Accessibility API calls.
//! Requires Accessibility permission in System Settings > Privacy & Security.

#[cfg(target_os = "macos")]
mod extractor;
#[cfg(target_os = "macos")]
mod observer;

#[cfg(target_os = "macos")]
pub use extractor::MacOsNativeAccessibility;

#[cfg(target_os = "macos")]
pub use observer::FocusObserverHandle;

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests;
