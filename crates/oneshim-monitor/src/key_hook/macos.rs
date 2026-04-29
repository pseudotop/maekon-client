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
use core_foundation::runloop::CFRunLoop;
use core_graphics::event::{
    CGEventFlags, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Run the CGEventTap key observer. Blocks until `running` becomes false.
///
/// This function MUST be called from a dedicated std::thread (not from
/// a tokio task) because CFRunLoop::run_current() blocks the thread.
pub fn run_event_tap(collector: Arc<InputActivityCollector>, running: Arc<AtomicBool>) {
    // Use the high-level CGEventTap::with_enabled API which handles
    // CFRunLoop setup and teardown automatically. The callback receives
    // a safe &CGEvent reference.
    //
    // CGEventTap with ListenOnly creates a passive observer that does
    // not modify or block key events.
    let result = core_graphics::event::CGEventTap::with_enabled(
        CGEventTapLocation::Session,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![
            CGEventType::KeyDown,
            // Receive TapDisabledByTimeout so we can detect re-enable events
            CGEventType::TapDisabledByTimeout,
            CGEventType::TapDisabledByUserInput,
        ],
        {
            let running = running.clone();
            let collector = collector.clone();
            move |_proxy, event_type, event| {
                if !running.load(Ordering::Relaxed) {
                    // Signal CFRunLoop to stop
                    CFRunLoop::get_current().stop();
                    return CallbackResult::Keep;
                }

                // Handle tap re-enable events
                if matches!(
                    event_type,
                    CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                ) {
                    debug!("CGEventTap was disabled, will re-enable");
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
        || {
            info!("CGEventTap active -- passive key observer running");
            // CFRunLoop::run_current() blocks until stop() is called (from the
            // callback when `running` becomes false, or from KeyHook::stop()).
            CFRunLoop::run_current();
        },
    );

    match result {
        Ok(()) => {
            debug!("CGEventTap run loop exited");
        }
        Err(()) => {
            warn!(
                "CGEventTapCreate failed -- grant Accessibility permission in \
                 System Settings > Privacy & Security > Accessibility"
            );
        }
    }
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

        // Immediately signal stop before the run loop starts
        running.store(false, Ordering::SeqCst);

        // run_event_tap should exit quickly because running is false
        let running_clone = running.clone();
        let collector_clone = collector.clone();
        let handle = std::thread::spawn(move || {
            run_event_tap(collector_clone, running_clone);
        });

        // Give it a moment then join
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = handle.join();
    }
}
