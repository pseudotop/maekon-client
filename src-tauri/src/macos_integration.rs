//! macOS-specific runtime integration for bare binary (non-.app bundle) execution.
//!
//! When the Maekon binary runs directly (e.g., installed via `install.sh`),
//! macOS has no Info.plist to read the dock icon from. This module sets it
//! programmatically via NSApplication API.

/// Set the macOS dock icon from the embedded icon.png at runtime.
///
/// Bare binaries (not .app bundles) don't have Info.plist, so macOS shows
/// a generic "exec" icon in the dock. This fixes that by calling
/// `[NSApplication setApplicationIconImage:]` with the brand icon.
pub fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let icon_bytes: &[u8] = include_bytes!("../icons/dock_icon.png");

    // MainThreadMarker::new() returns Some only when called from the main thread.
    // set_dock_icon() is called during Tauri setup, which runs on the main thread.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let data = NSData::with_bytes(icon_bytes);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };

    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: setApplicationIconImage is safe to call with a valid NSImage.
    // The method retains the image internally.
    unsafe {
        app.setApplicationIconImage(Some(&image));
    }
}
