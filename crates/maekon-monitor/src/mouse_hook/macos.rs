//! macOS mouse event observer using CGEventTap.
//!
//! Creates a passive (listen-only) event tap for mouse button, scroll, and
//! move events. The tap callback maps each event to the corresponding
//! `InputActivityCollector` recorder call.
//!
//! Runs on a dedicated std::thread. Uses CFRunLoop for event delivery.
//! The thread exits when the `running` AtomicBool is set to false.
//!
//! Requires Accessibility permission on macOS (System Settings >
//! Privacy & Security > Accessibility). If permission is denied,
//! CGEventTapCreate returns null and the function exits gracefully.
//!
//! This is a SEPARATE CGEventTap from `crate::key_hook::macos` (its own
//! thread + run loop), not an extension of the keyboard tap: the two hooks
//! have independent lifecycles (`KeyHook`/`MouseHook` each own/stop their own
//! tap), and macOS supports multiple simultaneous listen-only taps without
//! issue. This also keeps the already-tested keyboard tap code untouched.

use crate::input_activity::InputActivityCollector;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

/// macOS click-state field value indicating a double click (or higher --
/// triple clicks are counted as a double click here since `MouseActivity`
/// has no triple-click counter).
const DOUBLE_CLICK_STATE_THRESHOLD: i64 = 2;

/// Run the CGEventTap mouse observer. Blocks until `running` becomes false.
///
/// This function MUST be called from a dedicated std::thread (not from
/// a tokio task) because CFRunLoop::run_current() blocks the thread.
///
/// `run_loop` is a shared slot populated by this function with the observer
/// thread's `CFRunLoop` before it blocks in `run_current()`. `MouseHook::stop()`
/// calls `CFRunLoopStop` through this slot to wake the loop on idle sessions
/// where no mouse event ever fires the tap callback. `CFRunLoopStop` is
/// thread-safe.
pub fn run_event_tap(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    run_loop: Arc<Mutex<Option<CFRunLoop>>>,
) {
    // Same lower-level CGEventTap::new_unchecked rationale as key_hook::macos:
    // the tap handle must be reachable from inside the callback so a
    // TapDisabledByTimeout/TapDisabledByUserInput event can re-enable it.
    let tap_cell: Rc<RefCell<Option<CGEventTap>>> = Rc::new(RefCell::new(None));

    // SAFETY: identical contract to key_hook::macos::run_event_tap -- the tap
    // is dropped before this function returns and is only installed on this
    // thread's run loop, so the callback never outlives this thread.
    let tap_result = unsafe {
        CGEventTap::new_unchecked(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            vec![
                CGEventType::LeftMouseDown,
                CGEventType::RightMouseDown,
                CGEventType::OtherMouseDown,
                CGEventType::ScrollWheel,
                CGEventType::MouseMoved,
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
                        CFRunLoop::get_current().stop();
                        return CallbackResult::Keep;
                    }

                    // Re-enable the tap when macOS disables it. Without this the
                    // tap stays dead and all subsequent mouse events are lost.
                    if matches!(
                        event_type,
                        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
                    ) {
                        if let Ok(guard) = tap_cell.try_borrow() {
                            if let Some(tap) = guard.as_ref() {
                                tap.enable();
                                warn!(
                                    ?event_type,
                                    "CGEventTap (mouse) was disabled by the OS -- re-enabled"
                                );
                            } else {
                                warn!(
                                    ?event_type,
                                    "CGEventTap (mouse) disabled but handle unavailable -- \
                                     mouse collection may be lost"
                                );
                            }
                        } else {
                            warn!(
                                ?event_type,
                                "CGEventTap (mouse) disabled but handle busy -- \
                                 mouse collection may be lost"
                            );
                        }
                        return CallbackResult::Keep;
                    }

                    handle_mouse_event(&collector, event_type, event);

                    CallbackResult::Keep // listen-only tap must return the event unmodified
                }
            },
        )
    };

    let tap = match tap_result {
        Ok(tap) => tap,
        Err(()) => {
            warn!(
                "CGEventTapCreate (mouse) failed -- grant Accessibility permission in \
                 System Settings > Privacy & Security > Accessibility"
            );
            return;
        }
    };

    let loop_source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            warn!(
                "CGEventTap (mouse) run loop source creation failed -- mouse observer not started"
            );
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

    info!("CGEventTap (mouse) active -- passive mouse observer running");

    // Startup-race guard: identical to key_hook::macos -- stop() sets
    // `running = false` BEFORE locking this slot, while we check `running`
    // AFTER publishing the loop, all under the same lock.
    let should_run = if let Ok(mut guard) = run_loop.lock() {
        *guard = Some(current_loop);
        running.load(Ordering::SeqCst)
    } else {
        running.load(Ordering::SeqCst)
    };
    if should_run {
        CFRunLoop::run_current();
    }
    if let Ok(mut guard) = run_loop.lock() {
        *guard = None;
    }

    // Break the Rc cycle before returning (see key_hook::macos for the full
    // rationale) -- otherwise the CGEventTap (and its mach port) leaks.
    let dropped_tap = tap_cell.borrow_mut().take();
    drop(dropped_tap);

    debug!("CGEventTap (mouse) run loop exited");
}

/// Map a single CGEventTap mouse event to `InputActivityCollector` calls.
///
/// Extracted from the tap closure so it is directly unit-testable against
/// synthetic `CGEvent`s (mirrors how `classify_keycode` is unit-tested
/// independent of a real key tap) -- constructing a `CGEvent` via
/// `CGEvent::new_mouse_event`/`CGEvent::new` + `CGEventSource::new` does NOT
/// require Accessibility permission (only creating a live *tap* does), so
/// this path is exercised by a real test on every macOS host/CI runner.
fn handle_mouse_event(
    collector: &InputActivityCollector,
    event_type: CGEventType,
    event: &CGEvent,
) {
    match event_type {
        CGEventType::LeftMouseDown => {
            let loc = event.location();
            collector.record_click_at(loc.x as i32, loc.y as i32);

            let click_state = event.get_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE);
            if click_state >= DOUBLE_CLICK_STATE_THRESHOLD {
                collector.record_double_click();
            }
        }
        CGEventType::RightMouseDown => {
            collector.record_right_click();
        }
        CGEventType::OtherMouseDown => {
            // Middle/extra buttons have no dedicated MouseActivity counter --
            // count them as a generic click (same bucket as a plain
            // record_click(), consistent with the Linux button-2 mapping).
            collector.record_click();
        }
        CGEventType::ScrollWheel => {
            collector.record_scroll();
        }
        CGEventType::MouseMoved => {
            let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as f64;
            let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as f64;
            let distance = dx.hypot(dy);
            if distance > 0.0 {
                collector.record_mouse_move(distance);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::event::CGMouseButton;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::CGPoint;

    #[test]
    fn tap_context_size_is_reasonable() {
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

        running.store(false, Ordering::SeqCst);

        let running_clone = running.clone();
        let collector_clone = collector.clone();
        let run_loop_clone = run_loop.clone();
        let handle = std::thread::spawn(move || {
            run_event_tap(collector_clone, running_clone, run_loop_clone);
        });

        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = handle.join();
    }

    fn synthetic_source() -> CGEventSource {
        CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .expect("CGEventSourceCreate must not require Accessibility permission")
    }

    /// A real (synthetic, not-posted) left-mouse-down CGEvent drives
    /// `handle_mouse_event` into `record_click_at`, proving the mapping path
    /// -- and therefore `MouseActivity.click_count`/`last_position` -- is no
    /// longer dead once a real tap delivers this event type (#7652 HIGH-1).
    #[test]
    fn left_mouse_down_records_click_at_position() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::LeftMouseDown,
            CGPoint { x: 150.0, y: 300.0 },
            CGMouseButton::Left,
        )
        .expect("synthetic mouse event creation must not require Accessibility permission");

        handle_mouse_event(&collector, CGEventType::LeftMouseDown, &event);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 1);
        assert_eq!(snapshot.mouse.last_position, Some((150.0, 300.0)));
        assert_eq!(snapshot.mouse.double_click_count, 0);
    }

    #[test]
    fn left_mouse_down_with_click_state_two_also_records_double_click() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::LeftMouseDown,
            CGPoint { x: 10.0, y: 20.0 },
            CGMouseButton::Left,
        )
        .expect("synthetic mouse event creation must succeed");
        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, 2);

        handle_mouse_event(&collector, CGEventType::LeftMouseDown, &event);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 1);
        assert_eq!(snapshot.mouse.double_click_count, 1);
    }

    #[test]
    fn right_mouse_down_records_right_click() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::RightMouseDown,
            CGPoint { x: 0.0, y: 0.0 },
            CGMouseButton::Right,
        )
        .expect("synthetic mouse event creation must succeed");

        handle_mouse_event(&collector, CGEventType::RightMouseDown, &event);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.right_click_count, 1);
        assert_eq!(snapshot.mouse.click_count, 0);
    }

    #[test]
    fn other_mouse_down_records_generic_click() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::OtherMouseDown,
            CGPoint { x: 0.0, y: 0.0 },
            CGMouseButton::Center,
        )
        .expect("synthetic mouse event creation must succeed");

        handle_mouse_event(&collector, CGEventType::OtherMouseDown, &event);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 1);
        assert_eq!(snapshot.mouse.right_click_count, 0);
    }

    #[test]
    fn scroll_wheel_records_scroll() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        // A blank CGEvent (not a real scroll-wheel event) is enough here --
        // handle_mouse_event branches purely on the `event_type` parameter
        // for ScrollWheel, matching the real tap callback signature where
        // event_type is supplied by the CGEventTap dispatcher independent of
        // the event's own internal fields.
        let event = CGEvent::new(source).expect("blank CGEvent creation must succeed");

        handle_mouse_event(&collector, CGEventType::ScrollWheel, &event);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.scroll_count, 1);
    }

    #[test]
    fn mouse_moved_records_move_distance_from_delta() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        // CGEventSetIntegerValueField only persists mouse-delta fields on an
        // event that is actually mouse-typed -- a blank `CGEvent::new` (type
        // kCGEventNull) silently drops them. Use a real MouseMoved event.
        let event = CGEvent::new_mouse_event(
            source,
            CGEventType::MouseMoved,
            CGPoint { x: 0.0, y: 0.0 },
            CGMouseButton::Left,
        )
        .expect("synthetic mouse event creation must succeed");
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, 3);
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, 4);

        handle_mouse_event(&collector, CGEventType::MouseMoved, &event);

        let snapshot = collector.take_snapshot();
        // 3-4-5 triangle -> distance 5.0
        assert!((snapshot.mouse.move_distance - 5.0).abs() < 1e-9);
    }

    #[test]
    fn mouse_moved_with_zero_delta_does_not_record() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        let event = CGEvent::new(source).expect("blank CGEvent creation must succeed");

        handle_mouse_event(&collector, CGEventType::MouseMoved, &event);

        let snapshot = collector.take_snapshot();
        assert!((snapshot.mouse.move_distance - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn irrelevant_event_type_is_ignored() {
        let collector = InputActivityCollector::new();
        let source = synthetic_source();
        let event = CGEvent::new(source).expect("blank CGEvent creation must succeed");

        handle_mouse_event(&collector, CGEventType::KeyDown, &event);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 0);
        assert_eq!(snapshot.mouse.scroll_count, 0);
        assert!((snapshot.mouse.move_distance - 0.0).abs() < f64::EPSILON);
    }
}
