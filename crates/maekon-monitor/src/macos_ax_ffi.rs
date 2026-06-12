//! Minimal Accessibility (AX) FFI shim for the macOS active-window path.
//!
//! Scope: just enough to read the focused window's `AXTitle` for a given PID,
//! gated on Accessibility (Privacy & Security > Accessibility) permission.
//!
//! We deliberately do NOT depend on `maekon-vision` (which has a richer AX
//! extractor): an adapter -> adapter crate dependency is Forbidden by the
//! client architecture rules. Instead this file copies a small, self-contained
//! FFI surface, mirroring the CoreFoundation Create-Rule release discipline
//! used by `maekon-vision::accessibility::ffi_macos`.
//!
//! Attribute key strings are built as `CFString` at call time rather than
//! imported as `extern static kAX*Attribute` symbols, because the Rust linker
//! does not reliably resolve those symbols from the ApplicationServices `.tbd`
//! stubs across toolchain versions (same rationale as the vision crate).
//!
//! Reference: Apple Developer Documentation — Accessibility / CGWindowList.

// This module is only declared under `#[cfg(target_os = "macos")]` in lib.rs,
// so no inner `#![cfg]` gate is needed here.
#![allow(non_snake_case, non_upper_case_globals)]

// `CFTypeRef` is re-exported by `core_foundation::base` (from core-foundation-sys),
// so we avoid adding an explicit core-foundation-sys dependency.
use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use std::ptr;

/// Opaque accessibility element reference (same layout as `CFTypeRef`).
type AXUIElementRef = CFTypeRef;

/// AXError codes (subset we care about).
type AXError = i32;
const kAXErrorSuccess: AXError = 0;

/// Attribute key string values, constructed as `CFString` at call time.
const AX_FOCUSED_WINDOW_ATTR: &str = "AXFocusedWindow";
const AX_TITLE_ATTR: &str = "AXTitle";

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// Check whether the calling process has been granted Accessibility
    /// permission. `options` may be NULL or a dictionary containing
    /// `kAXTrustedCheckOptionPrompt`. We always pass NULL: a passive check,
    /// never a prompt (consent escalation is a separate, explicit flow).
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;

    /// Create an accessibility element for a specific application PID.
    /// Follows the Create Rule: the caller owns the returned element and must
    /// release it with `CFRelease`.
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;

    /// Copy the value of an attribute from an accessibility element.
    /// Follows the Create Rule for the returned `value`.
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    /// Release a CoreFoundation object (decrement its retain count).
    fn CFRelease(cf: CFTypeRef);
}

/// Return `true` if this process currently holds Accessibility permission.
///
/// This is a passive, non-prompting check (NULL options). When `false`, the
/// caller MUST NOT attempt AX reads — they would silently return nothing — and
/// should fall back to the osascript path instead.
pub(crate) fn is_process_trusted() -> bool {
    // SAFETY: `AXIsProcessTrustedWithOptions` is a pure permission query. We
    // pass NULL (no prompt dictionary), which Apple documents as a valid
    // argument. It has no out-params and does not transfer ownership, so there
    // is nothing to release.
    unsafe { AXIsProcessTrustedWithOptions(ptr::null()) }
}

/// Read the focused window's `AXTitle` for the application with the given PID.
///
/// Returns `None` when Accessibility permission is not granted, when the app
/// has no focused window, or when the title attribute is unavailable. The
/// caller is expected to gate on [`is_process_trusted`] and fall back to
/// osascript on `None`.
pub(crate) fn focused_window_title(pid: i32) -> Option<String> {
    if !is_process_trusted() {
        return None;
    }
    if pid <= 0 {
        return None;
    }

    // SAFETY: Each FFI result is checked for NULL / error and released exactly
    // once. `AXUIElementCreateApplication` and `AXUIElementCopyAttributeValue`
    // both follow the CoreFoundation Create Rule, so we own (and must release)
    // their non-null results. Attribute key `CFString`s are owned by their Rust
    // wrappers and dropped normally; only the borrowed `CFStringRef` is handed
    // to the C API for the duration of the call.
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }

        // app.AXFocusedWindow -> the focused window element (Create Rule).
        let focused_key = CFString::new(AX_FOCUSED_WINDOW_ATTR);
        let mut window: CFTypeRef = ptr::null();
        let err =
            AXUIElementCopyAttributeValue(app, focused_key.as_concrete_TypeRef(), &mut window);
        CFRelease(app);
        if err != kAXErrorSuccess || window.is_null() {
            return None;
        }

        // window.AXTitle -> the title string (Create Rule).
        let title_key = CFString::new(AX_TITLE_ATTR);
        let mut title_ref: CFTypeRef = ptr::null();
        let title_err =
            AXUIElementCopyAttributeValue(window, title_key.as_concrete_TypeRef(), &mut title_ref);
        CFRelease(window);
        if title_err != kAXErrorSuccess || title_ref.is_null() {
            return None;
        }

        // `AXUIElementCopyAttributeValue` follows the Create Rule: we own
        // `title_ref`. Wrapping under the Create Rule transfers that ownership
        // into the Rust `CFString`, which releases it on drop — so we must NOT
        // also call `CFRelease(title_ref)` here.
        let cf_str = CFString::wrap_under_create_rule(title_ref as CFStringRef);
        let title = cf_str.to_string();
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    }
}
