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
use core_foundation::base::{CFType, CFTypeRef, TCFType};
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

/// Per-message AX IPC budget, in seconds. A synchronous AX query blocks the
/// calling thread until the target app responds or this timeout elapses. The
/// macOS *system default* is multiple tens of seconds, so a hung target app
/// would otherwise permanently consume this path's `spawn_blocking` thread
/// (the active-window probe runs every monitor cycle). 2s mirrors the
/// `maekon-vision` AX extractor budget. (#6089 follow-up.)
const AX_MESSAGING_TIMEOUT_SECS: f32 = 2.0;

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

    /// Create the system-wide accessibility element. Follows the Create Rule.
    /// Setting a messaging timeout on this element establishes the
    /// PROCESS-GLOBAL default that every other element without a per-element
    /// override (e.g. the focused-window child queried below) inherits.
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;

    /// Set the timeout, in seconds, for synchronous AX messaging on `element`.
    /// Setting it on the system-wide element sets the process-global default;
    /// a per-element value overrides the global default for that element; `0`
    /// resets to the system default.
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> AXError;

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
        // Bound synchronous AX IPC time so a hung target app cannot stall this
        // blocking thread for the system-default timeout (tens of seconds). We
        // set the PROCESS-GLOBAL default via the system-wide element — this is
        // what the focused-window child element (queried below, with no
        // per-element override) inherits — and additionally set a per-element
        // timeout on `app` for the root query. (#6089 follow-up.)
        let system_wide = AXUIElementCreateSystemWide();
        if !system_wide.is_null() {
            AXUIElementSetMessagingTimeout(system_wide, AX_MESSAGING_TIMEOUT_SECS);
            CFRelease(system_wide);
        }

        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(app, AX_MESSAGING_TIMEOUT_SECS);

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
        // `title_ref`. Wrap it under the Create Rule into an untyped `CFType`,
        // which takes ownership and releases it on drop in EVERY path below
        // (including the non-string path) — so we must NOT also call
        // `CFRelease(title_ref)` here.
        //
        // `AXTitle` is documented as a CFString, but an AX attribute value can
        // in principle be any CFType. Verify the CoreFoundation type with a
        // `downcast_into::<CFString>()` (which checks `CFGetTypeID` internally)
        // before treating it as a string, mirroring the `cf_dict_string` /
        // `cf_dict_i64` discipline in `macos.rs`. On a match, ownership of the
        // Create-Rule ref is transferred straight into the `CFString` (no
        // retain/release churn); on a non-string value `downcast_into` returns
        // `None` and the owning `CFType` releases the ref as it drops — so there
        // is no leak and no double-free on either path.
        let cf_value = CFType::wrap_under_create_rule(title_ref);
        let cf_str = cf_value.downcast_into::<CFString>()?;
        let title = cf_str.to_string();
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    }
}
