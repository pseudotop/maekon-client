//! Linux mouse event observer using the X11 XInput2 extension.
//!
//! Mirrors `crate::key_hook::linux`'s pragmatic subprocess approach: spawns
//! `xinput test-xi2 --root` (a SEPARATE process from the key hook's own
//! `xinput` child -- the two hooks have independent lifecycles) and parses
//! its stdout for `ButtonPress` and `Motion` events instead of `KeyPress`.
//!
//! Best-effort implementation: if XInput2/xinput is unavailable (e.g.,
//! missing binary, pure Wayland without XWayland), it logs a warning and
//! exits gracefully -- `MouseHook::start()` then returns `None`.
//!
//! Runs on a dedicated std::thread.

use crate::input_activity::InputActivityCollector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

const XINPUT_TOOL: &str = "xinput";

/// Which event header the reader is currently inside, waiting for its
/// `root:` coordinate line (button number for ButtonPress arrives on an
/// earlier `detail:` line; Motion has no `detail:` line of interest).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PendingMouseEvent {
    ButtonPress,
    Motion,
}

/// Run the X11 XInput2 mouse observer. Blocks until `running` becomes false.
///
/// This is a best-effort implementation. If X11 or xinput is unavailable, it
/// logs a warning and returns immediately.
///
/// `child_proc` is a shared slot populated by this function after the xinput
/// process is spawned. `MouseHook::stop()` kills the child through this slot
/// so that the blocking `lines()` iterator receives EOF and unblocks,
/// allowing this thread to observe `running == false` and exit cleanly. Both
/// paths use `Option::take()` to avoid double-kill panics (mirrors
/// `key_hook::linux::run_x11_record_hook`).
pub fn run_x11_mouse_hook(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    child_proc: Arc<Mutex<Option<std::process::Child>>>,
) {
    let display_env = std::env::var("DISPLAY").unwrap_or_default();
    if display_env.is_empty() {
        warn!("No DISPLAY set -- X11 mouse hook unavailable (Wayland-only?)");
        return;
    }

    info!("starting X11 mouse observer via xinput test-xi2");

    // SEC-MON-01: resolve against the trusted-directory allowlist instead of
    // a bare `xinput` spawn (mirrors key_hook::linux -- see that module for
    // the full rationale).
    let Some(xinput_path) = crate::trusted_binary::resolve_trusted_binary(XINPUT_TOOL) else {
        warn!(
            "xinput not found under the trusted directory allowlist -- install with \
             'sudo apt install xinput' for mouse-activity tracking on Linux"
        );
        return;
    };

    let mut child = match std::process::Command::new(xinput_path)
        .args(["test-xi2", "--root"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                warn!(
                    "xinput not found -- install with 'sudo apt install xinput' \
                     for mouse-activity tracking on Linux"
                );
            } else {
                warn!("failed to spawn xinput (mouse observer): {e}");
            }
            return;
        }
    };

    // Publish the Child into the shared slot BEFORE taking stdout (mirrors
    // key_hook::linux's #5968 fix -- see that module for the startup-window
    // rationale).
    let child_slot = child_proc;
    let stdout = {
        let mut guard = match child_slot.lock() {
            Ok(g) => g,
            Err(_) => {
                warn!("mouse-hook child slot mutex poisoned; aborting xinput observer");
                let _ = child.kill();
                return;
            }
        };
        *guard = Some(child);
        let stdout = match guard.as_mut().and_then(|c| c.stdout.take()) {
            Some(s) => s,
            None => {
                warn!("failed to capture xinput stdout (mouse observer)");
                if let Some(mut c) = guard.take() {
                    let _ = c.kill();
                }
                return;
            }
        };
        drop(guard);
        stdout
    };

    use std::io::BufRead;
    let reader = std::io::BufReader::new(stdout);

    // xinput test-xi2 output format for a button press:
    //   EVENT type 4 (ButtonPress)
    //       detail: 1
    //       root: 1234.00/567.00
    //       ...
    // and for pointer motion:
    //   EVENT type 6 (Motion)
    //       detail: 0
    //       root: 1240.00/570.00
    //       ...
    // We track which kind of event header we are inside (`pending`), reset
    // it on EVERY new "EVENT type" header (not just the ones we care about)
    // so a missing/malformed `root:` line can never leak stale state into a
    // later, unrelated event -- mirroring key_hook::linux's KeyPress/
    // KeyRelease toggle discipline.
    let mut pending: Option<PendingMouseEvent> = None;
    let mut pending_button: Option<u32> = None;
    let mut prev_motion_position: Option<(f64, f64)> = None;

    for line in reader.lines() {
        if !running.load(Ordering::Relaxed) {
            break;
        }

        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();

        if trimmed.starts_with("EVENT type") {
            pending_button = None;
            pending = if trimmed.contains("ButtonPress") {
                Some(PendingMouseEvent::ButtonPress)
            } else if trimmed.contains("(Motion)") {
                Some(PendingMouseEvent::Motion)
            } else {
                None
            };
            continue;
        }

        let Some(kind) = pending else {
            continue;
        };

        if kind == PendingMouseEvent::ButtonPress {
            if let Some(detail) = trimmed.strip_prefix("detail:") {
                pending_button = detail.trim().parse::<u32>().ok();
                continue;
            }
        }

        if let Some(root) = trimmed.strip_prefix("root:") {
            let Some((x_str, y_str)) = root.trim().split_once('/') else {
                pending = None;
                continue;
            };
            let parsed = (x_str.trim().parse::<f64>(), y_str.trim().parse::<f64>());
            let (Ok(x), Ok(y)) = parsed else {
                pending = None;
                continue;
            };

            match kind {
                PendingMouseEvent::ButtonPress => {
                    if let Some(button) = pending_button {
                        dispatch_button_press(&collector, button, x as i32, y as i32);
                    }
                }
                PendingMouseEvent::Motion => {
                    if let Some(distance) = motion_move_distance(prev_motion_position, x, y) {
                        collector.record_mouse_move(distance);
                    }
                    prev_motion_position = Some((x, y));
                }
            }
            // Payload consumed -- wait for the next EVENT header.
            pending = None;
        }
    }

    // Clean up the child process (mirrors key_hook::linux's exit-path).
    if let Ok(mut guard) = child_slot.lock() {
        if let Some(mut child) = guard.take() {
            if let Err(e) = child.kill() {
                debug!("exit-path: xinput (mouse) kill failed (may have already exited): {e}");
            }
            if let Err(e) = child.wait() {
                debug!("exit-path: xinput (mouse) wait failed: {e}");
            }
        }
    }

    debug!("X11 mouse observer exited");
}

/// Map an X11 pointer button number (the classic X11/XInput2 convention) to
/// an `InputActivityCollector` call.
///
/// Button 1/2/3 are left/middle/right per the X11 core protocol. Buttons
/// 4-7 are the legacy scroll-wheel emulation (up/down/left/right) that X
/// servers have synthesized from wheel/smooth-scroll input for decades --
/// the same best-effort convention `xinput test-xi2` reports for both
/// physical wheels and most touchpad drivers.
fn dispatch_button_press(collector: &InputActivityCollector, button: u32, x: i32, y: i32) {
    match button {
        1 => collector.record_click_at(x, y),
        2 => collector.record_click(),
        3 => collector.record_right_click(),
        4..=7 => collector.record_scroll(),
        _ => {}
    }
}

/// Compute the Euclidean move distance from the previous Motion sample to
/// `(x, y)`. Returns `None` for the first sample in a session (no previous
/// position to diff against) or a zero-distance sample (no-op move).
///
/// Pure and free of any subprocess/OS dependency, so it is directly
/// unit-tested without a live xinput observer, mirroring how
/// `x11_keycode_to_keysym_approx` is tested independent of a live hook.
fn motion_move_distance(prev: Option<(f64, f64)>, x: f64, y: f64) -> Option<f64> {
    let (prev_x, prev_y) = prev?;
    let distance = (x - prev_x).hypot(y - prev_y);
    (distance > 0.0).then_some(distance)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_left_button_records_click_at_position() {
        let collector = InputActivityCollector::new();
        dispatch_button_press(&collector, 1, 100, 200);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 1);
        assert_eq!(snapshot.mouse.last_position, Some((100.0, 200.0)));
    }

    #[test]
    fn dispatch_middle_button_records_generic_click() {
        let collector = InputActivityCollector::new();
        dispatch_button_press(&collector, 2, 0, 0);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 1);
        assert_eq!(snapshot.mouse.right_click_count, 0);
    }

    #[test]
    fn dispatch_right_button_records_right_click() {
        let collector = InputActivityCollector::new();
        dispatch_button_press(&collector, 3, 0, 0);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.right_click_count, 1);
        assert_eq!(snapshot.mouse.click_count, 0);
    }

    #[test]
    fn dispatch_wheel_buttons_record_scroll() {
        for button in 4..=7u32 {
            let collector = InputActivityCollector::new();
            dispatch_button_press(&collector, button, 0, 0);
            let snapshot = collector.take_snapshot();
            assert_eq!(
                snapshot.mouse.scroll_count, 1,
                "button {button} must scroll"
            );
        }
    }

    #[test]
    fn dispatch_unknown_button_is_ignored() {
        let collector = InputActivityCollector::new();
        dispatch_button_press(&collector, 42, 0, 0);

        let snapshot = collector.take_snapshot();
        assert_eq!(snapshot.mouse.click_count, 0);
        assert_eq!(snapshot.mouse.scroll_count, 0);
        assert_eq!(snapshot.mouse.right_click_count, 0);
    }

    #[test]
    fn motion_distance_first_sample_has_no_previous_position() {
        // The very first Motion event in a session has no prior sample to
        // diff against -- matches `prev_motion_position` starting as `None`
        // in `run_x11_mouse_hook`.
        assert_eq!(motion_move_distance(None, 10.0, 10.0), None);
    }

    #[test]
    fn motion_distance_computes_euclidean_delta() {
        // 3-4-5 triangle -> distance 5.0
        let distance = motion_move_distance(Some((0.0, 0.0)), 3.0, 4.0);
        assert!((distance.expect("must compute a distance") - 5.0).abs() < 1e-9);
    }

    #[test]
    fn motion_distance_zero_delta_yields_none() {
        assert_eq!(motion_move_distance(Some((5.0, 5.0)), 5.0, 5.0), None);
    }
}
