//! FocusObserverHandle — AXObserver lifecycle, FFI, observer tests.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use core_foundation::string::CFString;
use core_foundation_sys::base::CFRelease;
use core_foundation_sys::string::CFStringRef;
use tracing::{debug, info, warn};

use crate::accessibility::ffi_macos::ax::*;

/// Shared state between the observer callback and the owner.
///
/// The callback sets `focus_changed` to true. The owner reads and
/// resets it via `has_focus_changed()`. The `running` flag controls
/// the CFRunLoop thread lifetime.
struct ObserverState {
    /// Set to `true` by the AXObserver callback when focus changes.
    focus_changed: AtomicBool,
    /// Set to `false` to signal the CFRunLoop thread to exit.
    running: AtomicBool,
}

/// Handle to a running AXObserver that detects focus changes for a
/// specific application PID.
///
/// Dropping the handle stops the observer and joins the background thread.
pub struct FocusObserverHandle {
    state: Arc<ObserverState>,
    /// The dedicated thread running the CFRunLoop.
    thread: Option<std::thread::JoinHandle<()>>,
    /// PID being observed (for diagnostics).
    pid: PidT,
}

impl FocusObserverHandle {
    /// Start observing focus changes for the given application PID.
    ///
    /// Spawns a dedicated thread that runs a CFRunLoop to receive
    /// AXObserver notifications. Returns `None` if the observer
    /// cannot be created (e.g. permission denied, invalid PID).
    pub fn start(pid: PidT) -> Option<Self> {
        let state = Arc::new(ObserverState {
            focus_changed: AtomicBool::new(false),
            running: AtomicBool::new(true),
        });

        // Verify that the observer can be created before spawning the
        // thread. This catches permission errors early.
        if !Self::can_create_observer(pid) {
            warn!(
                pid,
                "AXObserver: cannot create observer for PID (permission denied or invalid PID)"
            );
            return None;
        }

        let state_clone = state.clone();
        let thread = std::thread::Builder::new()
            .name(format!("ax-focus-observer-{pid}"))
            .spawn(move || {
                Self::run_observer_loop(pid, state_clone);
            })
            .ok()?;

        info!(pid, "AXFocusObserver started");

        Some(Self {
            state,
            thread: Some(thread),
            pid,
        })
    }

    /// Check whether the focused element has changed since the last check.
    ///
    /// Returns `true` exactly once per focus change event. Thread-safe.
    pub fn has_focus_changed(&self) -> bool {
        self.state.focus_changed.swap(false, Ordering::Acquire)
    }

    /// The PID being observed.
    pub fn observed_pid(&self) -> PidT {
        self.pid
    }

    /// Stop the observer. Also called automatically on drop.
    pub fn stop(&mut self) {
        if !self.state.running.swap(false, Ordering::Release) {
            return; // already stopped
        }
        debug!(pid = self.pid, "AXFocusObserver stopping");

        if let Some(handle) = self.thread.take() {
            // The CFRunLoop will exit on its next iteration because
            // `running` is false and the 0.5s timeout will fire.
            if let Err(e) = handle.join() {
                debug!("join failed: {e:?}");
            }
        }
    }

    /// Quick check: can we create an AXObserver for this PID?
    ///
    /// Creates and immediately releases an observer to validate
    /// that the PID is valid and accessibility permission is granted.
    fn can_create_observer(pid: PidT) -> bool {
        unsafe {
            let mut observer: AXObserverRef = std::ptr::null();
            let err = AXObserverCreate(pid, Self::focus_callback, &mut observer);
            if err == kAXErrorSuccess && !observer.is_null() {
                CFRelease(observer);
                true
            } else {
                false
            }
        }
    }

    /// The CFRunLoop thread body.
    ///
    /// Creates an AXObserver, subscribes to focus change notifications
    /// on the application element, and runs the CFRunLoop until
    /// `running` is set to false.
    fn run_observer_loop(pid: PidT, state: Arc<ObserverState>) {
        unsafe {
            // SAFETY: AXObserverCreate allocates and returns a new
            // AXObserverRef. We own it and must release it.
            let mut observer: AXObserverRef = std::ptr::null();
            let err = AXObserverCreate(pid, Self::focus_callback, &mut observer);
            if err != kAXErrorSuccess || observer.is_null() {
                warn!(
                    pid,
                    ax_error = err,
                    "AXObserverCreate failed in observer thread"
                );
                return;
            }

            // SAFETY: AXUIElementCreateApplication returns a new
            // AXUIElementRef for the given PID. Caller owns it.
            let app_element = AXUIElementCreateApplication(pid);
            if app_element.is_null() {
                warn!(pid, "AXUIElementCreateApplication returned null");
                CFRelease(observer);
                return;
            }

            // Subscribe to kAXFocusedUIElementChangedNotification.
            //
            // The `refcon` pointer carries our shared state so the
            // callback can set the `focus_changed` flag. We convert
            // the Arc to a raw pointer. The Arc is kept alive by
            // `state` in this scope -- we do NOT call Arc::from_raw
            // in the callback (which would double-free).
            let notification_name = CFString::new(AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION);
            let refcon = Arc::as_ptr(&state) as *mut c_void;

            let add_err = AXObserverAddNotification(
                observer,
                app_element,
                Self::as_cf_string_ref(&notification_name),
                refcon,
            );

            if add_err != kAXErrorSuccess {
                warn!(pid, ax_error = add_err, "AXObserverAddNotification failed");
                CFRelease(app_element);
                CFRelease(observer);
                return;
            }

            // Get the run loop source and add it to the current
            // thread's CFRunLoop.
            let source = AXObserverGetRunLoopSource(observer);
            if source.is_null() {
                warn!(pid, "AXObserverGetRunLoopSource returned null");
                Self::cleanup_observer(observer, app_element, &notification_name);
                return;
            }

            let run_loop = CFRunLoopGetCurrent();
            let mode = CFString::new(K_CF_RUN_LOOP_DEFAULT_MODE);

            // SAFETY: CFRunLoopAddSource does not take ownership of
            // the source. The source remains valid as long as the
            // observer is alive.
            CFRunLoopAddSource(run_loop, source, Self::as_cf_string_ref(&mode));

            debug!(pid, "AXObserver run loop source added, entering loop");

            // Run the CFRunLoop with periodic wake-ups to check the
            // `running` flag. We use CFRunLoopRunInMode with a 0.5s
            // timeout so we can exit promptly when stop() is called.
            while state.running.load(Ordering::Acquire) {
                // CFRunLoopRunInMode returns after processing one
                // source or after the timeout, whichever comes first.
                let result = CFRunLoopRunInMode(
                    Self::as_cf_string_ref(&mode),
                    0.5,  // seconds
                    true, // returnAfterSourceHandled (1 = true)
                );

                // kCFRunLoopRunFinished (1) means no sources left --
                // the observer was invalidated. Exit the loop.
                if result == 1 {
                    debug!(pid, "CFRunLoop finished (no sources), exiting");
                    break;
                }
            }

            // Cleanup: remove notification, release observer and element.
            CFRunLoopRemoveSource(run_loop, source, Self::as_cf_string_ref(&mode));
            Self::cleanup_observer(observer, app_element, &notification_name);

            debug!(pid, "AXObserver thread exiting");
        }
    }

    /// The AXObserver callback invoked when focus changes.
    ///
    /// SAFETY: This is called by the macOS accessibility framework on
    /// the CFRunLoop thread. `refcon` must point to a valid
    /// `ObserverState` (guaranteed because the Arc is kept alive by
    /// the `run_observer_loop` scope).
    pub(super) unsafe extern "C" fn focus_callback(
        _observer: AXObserverRef,
        _element: AXUIElementRef,
        _notification: CFStringRef,
        refcon: *mut c_void,
    ) {
        if refcon.is_null() {
            return;
        }
        // SAFETY: `refcon` points to the ObserverState inside the Arc.
        // We only read/write atomics through it -- no ownership transfer.
        let state = &*(refcon as *const ObserverState);
        state.focus_changed.store(true, Ordering::Release);
    }

    /// Helper: get raw CFStringRef from a CFString.
    fn as_cf_string_ref(s: &CFString) -> CFStringRef {
        use core_foundation::base::TCFType;
        s.as_concrete_TypeRef()
    }

    /// Helper: remove notification and release observer + element.
    unsafe fn cleanup_observer(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification_name: &CFString,
    ) {
        let _ = AXObserverRemoveNotification(
            observer,
            element,
            Self::as_cf_string_ref(notification_name),
        );
        CFRelease(element);
        CFRelease(observer);
    }
}

impl Drop for FocusObserverHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── CFRunLoopRunInMode FFI ──────────────────────────────────────────
//
// Not exposed in our ffi_macos.rs because it is a CoreFoundation
// function, not ApplicationServices. We declare it here privately.
extern "C" {
    /// Run the current thread's run loop in the given mode for up to
    /// `seconds`. Returns the reason the run loop exited:
    ///   0 = kCFRunLoopRunFinished (placeholder, unused in practice)
    ///   1 = kCFRunLoopRunStopped
    ///   2 = kCFRunLoopRunTimedOut
    ///   3 = kCFRunLoopRunHandledSource
    fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: f64,
        return_after_source_handled: bool,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_state_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FocusObserverHandle>();
    }

    #[test]
    fn has_focus_changed_returns_false_initially() {
        let state = Arc::new(ObserverState {
            focus_changed: AtomicBool::new(false),
            running: AtomicBool::new(false),
        });
        // Simulate the check without a real observer thread.
        assert!(!state.focus_changed.swap(false, Ordering::Acquire));
    }

    #[test]
    fn has_focus_changed_resets_after_read() {
        let state = Arc::new(ObserverState {
            focus_changed: AtomicBool::new(true),
            running: AtomicBool::new(false),
        });
        // First read should return true and reset.
        assert!(state.focus_changed.swap(false, Ordering::Acquire));
        // Second read should return false.
        assert!(!state.focus_changed.swap(false, Ordering::Acquire));
    }

    #[test]
    fn callback_sets_focus_changed_flag() {
        let state = Arc::new(ObserverState {
            focus_changed: AtomicBool::new(false),
            running: AtomicBool::new(true),
        });
        let refcon = Arc::as_ptr(&state) as *mut c_void;

        // SAFETY: state is valid and we pass it as refcon. The
        // callback only writes an atomic bool through the pointer.
        unsafe {
            FocusObserverHandle::focus_callback(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                refcon,
            );
        }
        assert!(state.focus_changed.load(Ordering::Acquire));
    }

    #[test]
    fn callback_handles_null_refcon() {
        // Should not panic or crash.
        unsafe {
            FocusObserverHandle::focus_callback(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
            );
        }
    }

    /// Integration test -- requires Accessibility permission and a running app.
    /// Run manually: `cargo test -p oneshim-vision -- focus_observer_integration --ignored`
    #[test]
    #[ignore]
    fn focus_observer_integration() {
        // Observe the current process (our own test binary).
        // This won't produce real focus events but validates the
        // create/subscribe/cleanup lifecycle.
        let pid = std::process::id() as PidT;
        let handle = FocusObserverHandle::start(pid);

        if let Some(mut handle) = handle {
            // No events expected -- just verify no crash.
            assert!(!handle.has_focus_changed());
            std::thread::sleep(std::time::Duration::from_millis(200));
            assert!(!handle.has_focus_changed());
            handle.stop();
        } else {
            eprintln!(
                "SKIP: FocusObserverHandle::start returned None \
                 (Accessibility permission not granted or invalid PID)"
            );
        }
    }

    /// Integration test -- observe a known app (Finder, PID 1 as launchd fallback).
    /// Run manually: `cargo test -p oneshim-vision -- focus_observer_finder --ignored`
    #[test]
    #[ignore]
    fn focus_observer_finder() {
        // Try to find Finder's PID via sysinfo or fall back to PID 1.
        let finder_pid = find_finder_pid().unwrap_or(1);
        let handle = FocusObserverHandle::start(finder_pid);

        match handle {
            Some(mut h) => {
                eprintln!(
                    "Observer started for PID {}. Switch focus in Finder within 3s...",
                    finder_pid
                );
                std::thread::sleep(std::time::Duration::from_secs(3));
                let changed = h.has_focus_changed();
                eprintln!("Focus changed: {changed}");
                h.stop();
            }
            None => {
                eprintln!("SKIP: could not create observer for Finder (PID {finder_pid})");
            }
        }
    }

    /// Helper: find Finder's PID by name.
    fn find_finder_pid() -> Option<PidT> {
        use std::process::Command;
        let output = Command::new("pgrep")
            .arg("-x")
            .arg("Finder")
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout.trim().lines().next()?.parse::<PidT>().ok()
    }
}
