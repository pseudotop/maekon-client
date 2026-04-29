//! Linux key event observer using X11 XRecord extension.
//!
//! Uses the XRecord extension to register a key event listener.
//! This is X11-only. On pure Wayland sessions (without XWayland),
//! the hook fails gracefully and KeyHook::start() returns None.
//!
//! Best-effort implementation: if XRecord is unavailable (e.g.,
//! missing xinput, restricted server), log a warning and exit.
//!
//! Runs on a dedicated std::thread. Spawns `xinput test-xi2 --root`
//! as a child process and parses its stdout for key press events.

use super::classify::classify_keycode;
use crate::input_activity::InputActivityCollector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Run the X11 XRecord key observer. Blocks until `running` becomes false.
///
/// This is a best-effort implementation. If X11 or XRecord is unavailable,
/// it logs a warning and returns immediately.
pub fn run_x11_record_hook(collector: Arc<InputActivityCollector>, running: Arc<AtomicBool>) {
    // Check if we can connect to X11
    let display_env = std::env::var("DISPLAY").unwrap_or_default();
    if display_env.is_empty() {
        warn!("No DISPLAY set -- X11 key hook unavailable (Wayland-only?)");
        return;
    }

    // The XRecord approach requires the `x11` crate or raw FFI to libX11
    // and libXtst. For Phase 1.5, we use a subprocess approach via
    // `xinput test-xi2 --root` as a pragmatic fallback that requires no
    // additional Rust crate dependencies.
    //
    // The `xinput` tool outputs key press/release events in a parseable
    // format. We spawn it, read stdout line by line, and classify keycodes.
    //
    // If `xinput` is not installed, we fall back to a no-op with a warning.
    info!("starting X11 key observer via xinput test-xi2");

    let mut child = match std::process::Command::new("xinput")
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
                     for key-category tracking on Linux"
                );
            } else {
                warn!("failed to spawn xinput: {e}");
            }
            return;
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            warn!("failed to capture xinput stdout");
            if let Err(e) = child.kill() {
                debug!("process kill failed: {e}");
            }
            return;
        }
    };

    use std::io::BufRead;
    let reader = std::io::BufReader::new(stdout);

    // xinput test-xi2 output format:
    //   EVENT type 2 (KeyPress)
    //       detail: 36
    //       ...
    // We parse "EVENT type 2" for key press and "detail: <keycode>" for
    // the X11 keycode, then map to approximate keysym.

    let mut in_key_press = false;

    for line in reader.lines() {
        if !running.load(Ordering::Relaxed) {
            break;
        }

        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        let trimmed = line.trim();

        if trimmed.contains("EVENT type 2") || trimmed.contains("KeyPress") {
            in_key_press = true;
            continue;
        }

        if trimmed.contains("EVENT type 3") || trimmed.contains("KeyRelease") {
            in_key_press = false;
            continue;
        }

        if in_key_press {
            if let Some(detail) = trimmed.strip_prefix("detail:") {
                if let Ok(keycode) = detail.trim().parse::<u32>() {
                    // Convert X11 hardware keycode to approximate keysym
                    let keysym = x11_keycode_to_keysym_approx(keycode);
                    let category = classify_keycode(keysym);
                    collector.record_categorized_keystroke(category, false, false);
                    in_key_press = false;
                }
            }
        }
    }

    // Clean up the child process
    if let Err(e) = child.kill() {
        debug!("process kill failed: {e}");
    }
    if let Err(e) = child.wait() {
        debug!("process wait failed: {e}");
    }

    debug!("X11 key observer exited");
}

/// Approximate mapping from X11 hardware keycode to keysym.
///
/// X11 hardware keycodes vary by keyboard model, but the standard
/// evdev mapping (keycode - 8 = evdev code) is common on modern
/// Linux systems. This maps the most common keys; unmapped codes
/// default to a "regular" keysym range.
fn x11_keycode_to_keysym_approx(keycode: u32) -> u32 {
    // Standard evdev-based mapping (common on modern Linux)
    match keycode {
        9 => 0xFF1B,         // Escape
        22 => 0xFF08,        // BackSpace
        23 => 0xFF09,        // Tab
        36 => 0xFF0D,        // Return
        104 => 0xFF8D,       // KP_Enter
        111 => 0xFF52,       // Up
        113 => 0xFF51,       // Left
        114 => 0xFF53,       // Right
        116 => 0xFF54,       // Down
        110 => 0xFF50,       // Home
        115 => 0xFF57,       // End
        112 => 0xFF55,       // Page_Up
        117 => 0xFF56,       // Page_Down
        119 => 0xFFFF,       // Delete
        67 => 0xFFBE,        // F1
        68 => 0xFFBF,        // F2
        69 => 0xFFC0,        // F3
        70 => 0xFFC1,        // F4
        71 => 0xFFC2,        // F5
        72 => 0xFFC3,        // F6
        73 => 0xFFC4,        // F7
        74 => 0xFFC5,        // F8
        75 => 0xFFC6,        // F9
        76 => 0xFFC7,        // F10
        95 => 0xFFC8,        // F11
        96 => 0xFFC9,        // F12
        50 | 62 => 0xFFE1,   // Shift_L, Shift_R
        37 | 105 => 0xFFE3,  // Control_L, Control_R
        64 | 108 => 0xFFE9,  // Alt_L, Alt_R
        133 | 134 => 0xFFEB, // Super_L, Super_R
        66 => 0xFFE5,        // Caps_Lock
        _ => 0x0061,         // Default to 'a' (Regular)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_keycode_return_maps_correctly() {
        assert_eq!(x11_keycode_to_keysym_approx(36), 0xFF0D);
    }

    #[test]
    fn x11_keycode_escape_maps_correctly() {
        assert_eq!(x11_keycode_to_keysym_approx(9), 0xFF1B);
    }

    #[test]
    fn x11_keycode_arrows_map_correctly() {
        assert_eq!(x11_keycode_to_keysym_approx(111), 0xFF52); // Up
        assert_eq!(x11_keycode_to_keysym_approx(113), 0xFF51); // Left
        assert_eq!(x11_keycode_to_keysym_approx(114), 0xFF53); // Right
        assert_eq!(x11_keycode_to_keysym_approx(116), 0xFF54); // Down
    }

    #[test]
    fn x11_keycode_unknown_is_regular_keysym() {
        // Unknown keycode maps to 'a' keysym -> Regular
        assert_eq!(x11_keycode_to_keysym_approx(999), 0x0061);
    }
}
