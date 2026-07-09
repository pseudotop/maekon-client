//! macOS key event observer using CGEventTap.
//!
//! Creates a passive (listen-only) event tap for key-down events.
//! The tap callback classifies each key's CGKeyCode into KeyCategory
//! and calls record_categorized_keystroke() on the shared collector.
//!
//! Runs on a dedicated std::thread. Uses CFRunLoop for event delivery.
//! The thread exits when the `running` AtomicBool is set to false.
//!
//! Requires Accessibility permission on macOS (System Settings >
//! Privacy & Security > Accessibility). If permission is denied,
//! CGEventTapCreate returns null and the function exits gracefully.

use super::classify::classify_keycode;
use crate::input_activity::InputActivityCollector;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

/// Run the CGEventTap key observer. Blocks until `running` becomes false.
///
/// This function MUST be called from a dedicated std::thread (not from
/// a tokio task) because CFRunLoop::run_current() blocks the thread.
///
/// `run_loop` is a shared slot populated by this function with the observer
/// thread's `CFRunLoop` before it blocks in `run_current()`. `KeyHook::stop()`
/// calls `CFRunLoopStop` through this slot to wake the loop on idle sessions
/// where no key event ever fires the tap callback (the only other path that
/// observes `running == false`). `CFRunLoopStop` is thread-safe.
pub fn run_event_tap(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    run_loop: Arc<Mutex<Option<CFRunLoop>>>,
) {
    // Use the lower-level CGEventTap::new_unchecked path (rather than the
    // higher-level `with_enabled`) so the tap handle is available *inside* the
    // callback. macOS disables an event tap and delivers a one-shot
    // TapDisabledByTimeout / TapDisabledByUserInput event whenever the callback
    // is too slow or the user generates input faster than it can be drained;
    // the tap must be explicitly re-enabled or key-category collection is lost
    // permanently. `with_enabled` does not expose the handle to the callback,
    // so it cannot re-enable the tap (#17).
    //
    // The tap is stored in a single-thread shared cell so the callback can read
    // it back and call `enable()`. This is sound because the tap, its run loop,
    // and its callback all live on *this* dedicated thread: the callback is only
    // ever invoked from this thread's run loop, and the tap is dropped before
    // this function returns. We therefore use a non-`Send` `Rc<RefCell<..>>`
    // rather than an `Arc<Mutex<..>>`.
    //
    // CGEventTap with ListenOnly creates a passive observer that does not modify
    // or block key events.
    let tap_cell: Rc<RefCell<Option<CGEventTap>>> = Rc::new(RefCell::new(None));

    // SAFETY: We bypass the 'static restriction of CGEventTap::new because the
    // tap is dropped before this function returns (the callback therefore cannot
    // outlive its captured state), and the tap is only installed on the current
    // thread's run loop, so the callback is only ever called from this thread.
    let tap_result = unsafe {
        CGEventTap::new_unchecked(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![
                CGEventType::KeyDown,
                // Receive tap-disabled events so we can re-enable the tap.
                CGEventType::TapDisabledByTimeout,
                CGEventType::TapDisabledByUserInput,
            ],
            {
                let running = running.clone();
                let collector = collector.clone();
                let tap_cell = tap_cell.clone();
                move |_proxy, event_type, event| {
                    if !running.load(Ordering::Relaxed) {
                        // Signal CFRunLoop to stop
                        CFRunLoop::get_current().stop();
                        return CallbackResult::Keep;
                    }

                    // Re-enable the tap when macOS disables it. Without this the
                    // tap stays dead and all subsequent key events are lost.
                    if matches!(
                        event_type,
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                    ) {
                        if let Ok(guard) = tap_cell.try_borrow() {
                            if let Some(tap) = guard.as_ref() {
                                tap.enable();
                                warn!(
                                    ?event_type,
                                    "CGEventTap was disabled by the OS -- re-enabled"
                                );
                            } else {
                                // Should not happen: the cell is populated before
                                // the run loop starts delivering events.
                                warn!(
                                    ?event_type,
                                    "CGEventTap disabled but handle unavailable -- \
                                     key collection may be lost"
                                );
                            }
                        } else {
                            warn!(
                                ?event_type,
                                "CGEventTap disabled but handle busy -- \
                                 key collection may be lost"
                            );
                        }
                        return CallbackResult::Keep;
                    }

                    if !matches!(event_type, CGEventType::KeyDown) {
                        return CallbackResult::Keep;
                    }

                    // Extract the virtual keycode from the event
                    let keycode =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;

                    let category = classify_keycode(keycode);

                    // Check if modifier flags indicate a shortcut (Command or Control held)
                    let flags = event.get_flags();
                    let is_shortcut = flags.contains(CGEventFlags::CGEventFlagCommand)
                        || flags.contains(CGEventFlags::CGEventFlagControl);

                    // Backspace is classified as correction automatically inside
                    // record_categorized_keystroke; is_correction=false here.
                    collector.record_categorized_keystroke(category, is_shortcut, false);

                    CallbackResult::Keep // listen-only tap must return the event unmodified
                }
            },
        )
    };

    let tap = match tap_result {
        Ok(tap) => tap,
        Err(()) => {
            warn!(
                "CGEventTapCreate failed -- grant Accessibility permission in \
                 System Settings > Privacy & Security > Accessibility"
            );
            return;
        }
    };

    // Wire the tap's mach port into the current thread's run loop, mirroring what
    // CGEventTap::with_enabled does internally.
    let loop_source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            warn!("CGEventTap run loop source creation failed -- key observer not started");
            return;
        }
    };
    let current_loop = CFRunLoop::get_current();
    current_loop.add_source(&loop_source, unsafe { kCFRunLoopCommonModes });

    // Publish the tap so the callback can re-enable it on TapDisabled events.
    *tap_cell.borrow_mut() = Some(tap);
    let tap_borrow = tap_cell.borrow();
    let Some(tap) = tap_borrow.as_ref() else {
        panic!("tap was just stored");
    };
    tap.enable();

    info!("CGEventTap active -- passive key observer running");

    // Publish this thread's run loop so KeyHook::stop() can wake it via
    // CFRunLoopStop on idle sessions. The tap source is already added to the
    // current run loop, so get_current() here is the loop that run_current()
    // will block on.
    //
    // Startup-race guard: stop() sets `running = false` BEFORE locking this
    // slot, while we check `running` AFTER publishing the loop, all under the
    // same lock. So either stop() locks first (we then observe running == false
    // and skip run_current()), or we lock and publish first (stop() then sees
    // Some(..) and calls CFRunLoopStop). Either ordering avoids a permanent
    // block on an idle session.
    let should_run = if let Ok(mut guard) = run_loop.lock() {
        *guard = Some(current_loop);
        running.load(Ordering::SeqCst)
    } else {
        // Poisoned slot: fall back to the flag so we never block here.
        running.load(Ordering::SeqCst)
    };
    if should_run {
        // CFRunLoop::run_current() blocks until stop() is called (from the
        // callback when `running` becomes false, or from KeyHook::stop() via
        // the published run loop reference above).
        CFRunLoop::run_current();
    }
    // Clear the slot so stop() does not act on a stale run loop after the loop
    // has already returned.
    if let Ok(mut guard) = run_loop.lock() {
        *guard = None;
    }

    // Break the Rc cycle before returning. The callback (owned by the tap) holds
    // a clone of `tap_cell`, and `tap_cell` holds the tap — so merely dropping
    // the local `tap_cell` at end of scope would leave the strong count at 1 and
    // the CGEventTap (and its mach port) would leak forever. The run loop has
    // stopped, so the callback can no longer fire; explicitly take the tap out of
    // the cell and drop it here. That severs the cycle, runs CGEventTap's Drop
    // (invalidating the mach port), and lets the remaining `tap_cell` Rc free at
    // end of scope — satisfying the new_unchecked SAFETY contract that the tap
    // not outlive this thread/run loop. (#17 follow-up: fixes the Rc-cycle leak.)
    let dropped_tap = tap_cell.borrow_mut().take();
    drop(dropped_tap);

    debug!("CGEventTap run loop exited");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_context_size_is_reasonable() {
        // Ensure we are not accidentally capturing a large closure
        assert!(std::mem::size_of::<Arc<InputActivityCollector>>() <= 16);
    }

    /// Test that creating and immediately stopping the hook does not panic.
    /// This test may fail in CI without Accessibility permission; that is
    /// expected. The test is primarily for local development.
    #[test]
    fn start_stop_does_not_panic() {
        let collector = Arc::new(InputActivityCollector::new());
        let running = Arc::new(AtomicBool::new(true));
        let run_loop: Arc<Mutex<Option<CFRunLoop>>> = Arc::new(Mutex::new(None));

        // Immediately signal stop before the run loop starts
        running.store(false, Ordering::SeqCst);

        // run_event_tap should exit quickly because running is false
        let running_clone = running.clone();
        let collector_clone = collector.clone();
        let run_loop_clone = run_loop.clone();
        let handle = std::thread::spawn(move || {
            run_event_tap(collector_clone, running_clone, run_loop_clone);
        });

        // Give it a moment then join
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = handle.join();
    }
}
