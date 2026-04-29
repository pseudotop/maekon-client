//! Raw FFI bindings for macOS Accessibility API functions.
//!
//! These functions are not exposed by the `core-foundation` crate.
//! We link to the ApplicationServices framework which provides them.
//!
//! Attribute key constants are constructed as `CFString` at call time rather
//! than imported as extern statics, because the Rust linker does not reliably
//! resolve `kAX*Attribute` symbols from the ApplicationServices `.tbd` stubs
//! in all toolchain versions.
//!
//! Reference: Apple Developer Documentation -- Accessibility Reference

#![allow(non_snake_case, non_upper_case_globals)]

#[cfg(target_os = "macos")]
#[allow(dead_code)] // consumed by sibling `macos.rs` module
pub(crate) mod ax {
    use std::ffi::c_void;

    use core_foundation::string::CFString;
    use core_foundation_sys::base::CFTypeRef;
    use core_foundation_sys::string::CFStringRef;

    /// Opaque type for an accessibility element (same layout as CFTypeRef).
    pub type AXUIElementRef = CFTypeRef;

    /// AXError codes.
    pub type AXError = i32;
    pub const kAXErrorSuccess: AXError = 0;
    pub const kAXErrorAPIDisabled: AXError = -25211;
    pub const kAXErrorNoValue: AXError = -25212;
    pub const kAXErrorAttributeUnsupported: AXError = -25205;
    pub const kAXErrorNotImplemented: AXError = -25208;

    // Attribute key string values -- constructed at call time.
    // These correspond to the Apple-defined kAX*Attribute constants.
    pub const AX_FOCUSED_UI_ELEMENT_ATTR: &str = "AXFocusedUIElement";
    pub const AX_ROLE_ATTR: &str = "AXRole";
    pub const AX_TITLE_ATTR: &str = "AXTitle";
    pub const AX_VALUE_ATTR: &str = "AXValue";
    pub const AX_DESCRIPTION_ATTR: &str = "AXDescription";
    pub const AX_POSITION_ATTR: &str = "AXPosition";
    pub const AX_SIZE_ATTR: &str = "AXSize";
    pub const AX_PLACEHOLDER_VALUE_ATTR: &str = "AXPlaceholderValue";
    pub const AX_CHILDREN_ATTR: &str = "AXChildren";
    pub const AX_WINDOW_ATTR: &str = "AXWindow";
    pub const AX_FOCUSED_WINDOW_ATTR: &str = "AXFocusedWindow";

    /// Create a CFStringRef from a Rust string constant.
    /// The returned CFString is autoreleased/owned by the caller through
    /// the core-foundation wrapper.
    pub fn ax_attr(name: &str) -> CFString {
        CFString::new(name)
    }

    /// Get the raw CFStringRef pointer from a CFString for use with AX API.
    pub fn as_cf_ref(s: &CFString) -> CFStringRef {
        use core_foundation::base::TCFType;
        s.as_concrete_TypeRef()
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        /// Create a system-wide accessibility element (the root of the
        /// accessibility tree across all applications).
        pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;

        /// Copy the value of an attribute from an accessibility element.
        pub fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;

        /// Check whether the calling process has been granted accessibility
        /// permission. `options` can be NULL or a dictionary containing
        /// kAXTrustedCheckOptionPrompt.
        pub fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;

        /// Copy multiple attribute values from an accessibility element in a single IPC.
        /// `attributes` is a CFArrayRef of CFStringRef attribute names.
        /// `values` receives a CFArrayRef of CFTypeRef values (same order).
        pub fn AXUIElementCopyMultipleAttributeValues(
            element: AXUIElementRef,
            attributes: core_foundation_sys::array::CFArrayRef,
            options: u32, // 0 = default
            values: *mut core_foundation_sys::array::CFArrayRef,
        ) -> AXError;
    }

    // Extract a CGPoint / CGSize from an AXValue.
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        pub fn AXValueGetValue(value: CFTypeRef, value_type: u32, value_ptr: *mut c_void) -> bool;
    }

    // AXValueType constants
    pub const kAXValueCGPointType: u32 = 1;
    pub const kAXValueCGSizeType: u32 = 2;

    // ── AXObserver API ──────────────────────────────────────────────────
    //
    // AXObserver subscribes to accessibility notifications (e.g.
    // kAXFocusedUIElementChangedNotification) and delivers them via a
    // CFRunLoopSource. This enables event-driven focus change detection
    // instead of polling.

    /// Process ID type (matches libc::pid_t).
    pub type PidT = i32;

    /// Opaque AXObserver reference (same layout as CFTypeRef).
    pub type AXObserverRef = CFTypeRef;

    /// CFRunLoopSourceRef — opaque pointer to a CFRunLoopSource.
    pub type CFRunLoopSourceRef = *const c_void;

    /// CFRunLoopRef — opaque pointer to a CFRunLoop.
    pub type CFRunLoopRef = *const c_void;

    /// Callback signature for AXObserver notifications.
    ///
    /// Parameters:
    /// - `observer`: The AXObserver that received the notification
    /// - `element`: The accessibility element that triggered it
    /// - `notification`: The notification name (CFStringRef)
    /// - `refcon`: User-provided context pointer
    pub type AXObserverCallback = unsafe extern "C" fn(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    );

    /// Notification name for focus changes.
    /// Corresponds to Apple's kAXFocusedUIElementChangedNotification.
    pub const AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION: &str = "AXFocusedUIElementChanged";

    /// Notification name for application-level focus changes.
    /// Corresponds to Apple's kAXFocusedWindowChangedNotification.
    pub const AX_FOCUSED_WINDOW_CHANGED_NOTIFICATION: &str = "AXFocusedWindowChanged";

    /// Default run loop mode constant string.
    pub const K_CF_RUN_LOOP_DEFAULT_MODE: &str = "kCFRunLoopDefaultMode";

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        /// Create an AXObserver for the given PID with a notification callback.
        ///
        /// Returns kAXErrorSuccess on success. The caller owns the returned
        /// observer and must release it with CFRelease.
        pub fn AXObserverCreate(
            application: PidT,
            callback: AXObserverCallback,
            observer: *mut AXObserverRef,
        ) -> AXError;

        /// Register a notification on an element with the observer.
        ///
        /// `refcon` is passed through to the callback as user data.
        pub fn AXObserverAddNotification(
            observer: AXObserverRef,
            element: AXUIElementRef,
            notification: CFStringRef,
            refcon: *mut c_void,
        ) -> AXError;

        /// Unregister a previously registered notification.
        pub fn AXObserverRemoveNotification(
            observer: AXObserverRef,
            element: AXUIElementRef,
            notification: CFStringRef,
        ) -> AXError;

        /// Get the CFRunLoopSource for the observer.
        ///
        /// The source must be added to a CFRunLoop for the observer to
        /// receive notifications.
        pub fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;

        /// Create an accessibility element for a specific application PID.
        pub fn AXUIElementCreateApplication(pid: PidT) -> AXUIElementRef;
    }

    // CFRunLoop functions from CoreFoundation.
    //
    // We declare these directly instead of depending on the
    // core-foundation crate's higher-level wrappers, because we need
    // the raw C pointers for interop with AXObserverGetRunLoopSource.
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        /// Get the CFRunLoop for the current thread.
        pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;

        /// Add a CFRunLoopSource to a run loop in the given mode.
        pub fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);

        /// Remove a CFRunLoopSource from a run loop.
        pub fn CFRunLoopRemoveSource(
            rl: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFStringRef,
        );

        /// Run the current thread's run loop indefinitely.
        pub fn CFRunLoopRun();

        /// Stop the current thread's run loop.
        pub fn CFRunLoopStop(rl: CFRunLoopRef);
    }

    /// CGPoint for position extraction.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    /// CGSize for size extraction.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default)]
    pub struct CGSize {
        pub width: f64,
        pub height: f64,
    }
}
