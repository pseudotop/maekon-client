//! Windows mouse event observer using Raw Input.
//!
//! Registers a Raw Input mouse device listener using
//! `RegisterRawInputDevices` with `RIDEV_INPUTSINK`. This receives all
//! mouse input system-wide without blocking or modifying events. Mirrors
//! `crate::key_hook::windows`'s hidden message-only window + message pump
//! pattern, but registers `RIM_TYPEMOUSE` (HID usage 0x02) instead of
//! `RIM_TYPEKEYBOARD` -- this is a SEPARATE message-only window/thread from
//! the key hook's own, not a shared one.
//!
//! The thread exits when the `running` `AtomicBool` is set to false or
//! when a `WM_QUIT` message is posted.

use crate::input_activity::InputActivityCollector;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use tracing::debug;
#[cfg(not(target_os = "windows"))]
use tracing::warn;
#[cfg(target_os = "windows")]
use tracing::{error, info};

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE,
    RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEMOUSE,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, RegisterClassW,
    HWND_MESSAGE, MSG, WM_INPUT, WNDCLASSW,
};

// The stop-message id (`WM_STOP_HOOK`) this message pump listens for is
// defined once in `crate::hook_lifecycle` -- shared with `key_hook::windows`'s
// separate message-only window/thread (scoped to its own message queue, so
// reusing the same numeric value causes no collision) and with the
// `PostThreadMessageW` poster in `HookLifecycle::stop()` (#7727).
#[cfg(target_os = "windows")]
use crate::hook_lifecycle::WM_STOP_HOOK;

#[cfg(target_os = "windows")]
thread_local! {
    static TL_COLLECTOR: std::cell::RefCell<Option<Arc<InputActivityCollector>>> =
        const { std::cell::RefCell::new(None) };
    static TL_RUNNING: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

/// RI_MOUSE_* button-down flag bits (`usButtonFlags` of `RAWMOUSE`). Defined
/// as plain local constants (rather than importing the windows-sys values)
/// so `mouse_button_actions` below needs no `RAWMOUSE`/union access to unit
/// test -- mirrors `key_hook::linux::x11_keycode_to_keysym_approx`'s use of
/// hardcoded protocol constants for the same decoupling reason. Values per
/// the Win32 RAWMOUSE contract (winuser.h `RI_MOUSE_*`). This whole module
/// only compiles on the `windows` target (gated in `mod.rs`), so these tests
/// run on Windows CI, not on this macOS development host (#7652 note).
const RI_MOUSE_LEFT_BUTTON_DOWN: u32 = 0x0001;
const RI_MOUSE_RIGHT_BUTTON_DOWN: u32 = 0x0004;
const RI_MOUSE_MIDDLE_BUTTON_DOWN: u32 = 0x0010;
const RI_MOUSE_WHEEL: u32 = 0x0400;

/// Discrete button actions decoded from a RAWMOUSE `usButtonFlags` bitmask.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MouseButtonActions {
    left_down: bool,
    right_down: bool,
    middle_down: bool,
    wheel: bool,
}

/// Decode a RAWMOUSE `usButtonFlags` bitmask into discrete actions.
///
/// Pure and decoupled from the RAWMOUSE union, mirroring how
/// `key_hook::windows`'s `aligned_buf_covers_requested_byte_size` test
/// exercises pure Rust logic independent of live Win32 calls.
fn mouse_button_actions(button_flags: u32) -> MouseButtonActions {
    MouseButtonActions {
        left_down: button_flags & RI_MOUSE_LEFT_BUTTON_DOWN != 0,
        right_down: button_flags & RI_MOUSE_RIGHT_BUTTON_DOWN != 0,
        middle_down: button_flags & RI_MOUSE_MIDDLE_BUTTON_DOWN != 0,
        wheel: button_flags & RI_MOUSE_WHEEL != 0,
    }
}

/// Compute the move distance from a RAWMOUSE relative delta.
///
/// Returns `None` when the device reports absolute coordinates
/// (`is_absolute`) -- `lLastX`/`lLastY` are only a meaningful delta in the
/// default relative mode; RDP/tablet devices that report absolute
/// coordinates are intentionally skipped rather than mis-accumulated as one
/// huge "jump" distance. Also `None` for a zero delta (no-op move).
///
/// Pure and decoupled from the RAWMOUSE union for direct unit testing.
fn compute_move_distance(last_x: i32, last_y: i32, is_absolute: bool) -> Option<f64> {
    if is_absolute || (last_x == 0 && last_y == 0) {
        return None;
    }
    Some(f64::from(last_x).hypot(f64::from(last_y)))
}

/// Run the Raw Input mouse hook. Blocks until `running` becomes false.
///
/// Creates a hidden message-only window, registers a Raw Input mouse device
/// with `RIDEV_INPUTSINK`, and processes `WM_INPUT` messages for
/// `RIM_TYPEMOUSE` events. See `key_hook::windows::run_raw_input_hook` for
/// the full rationale behind the message-only window + `hook_thread_id`
/// wakeup design (mirrored here 1:1, just for mouse instead of keyboard).
#[cfg(target_os = "windows")]
pub fn run_raw_input_mouse_hook(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    hook_thread_id: Arc<AtomicU32>,
) {
    TL_COLLECTOR.with(|c| *c.borrow_mut() = Some(collector));
    TL_RUNNING.with(|r| *r.borrow_mut() = Some(running.clone()));

    // SAFETY: identical Win32 window class + Raw Input registration sequence
    // and safety contract as `key_hook::windows::run_raw_input_hook` -- see
    // that function's SAFETY comment for the full rationale. The only
    // difference is the HID usage (mouse instead of keyboard) and window
    // class name (must be unique per RegisterClassW call within the
    // process, since this hook owns its own message-only window distinct
    // from the key hook's).
    unsafe {
        let class_name: Vec<u16> = "MaekonRawInputMouseHook\0".encode_utf16().collect();

        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(raw_input_mouse_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };

        let atom = RegisterClassW(&wc);
        if atom == 0 {
            error!(
                "RegisterClassW failed (error={}); Raw Input mouse hook not started",
                GetLastError()
            );
            return;
        }

        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );

        if hwnd.is_null() {
            error!(
                "CreateWindowExW failed (error={}); Raw Input mouse hook not started",
                GetLastError()
            );
            return;
        }

        // HID_USAGE_PAGE_GENERIC / HID_USAGE_GENERIC_MOUSE.
        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };

        let registered = RegisterRawInputDevices(
            &rid as *const RAWINPUTDEVICE,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        );

        if registered == 0 {
            error!(
                "RegisterRawInputDevices (mouse) failed (error={}); Raw Input mouse hook not started",
                GetLastError()
            );
            DestroyWindow(hwnd);
            return;
        }

        info!("Windows Raw Input mouse hook active (message-only window)");

        hook_thread_id.store(GetCurrentThreadId(), Ordering::SeqCst);

        let mut msg: MSG = std::mem::zeroed();
        loop {
            if !running.load(Ordering::Relaxed) {
                debug!("running flag is false — exiting Raw Input mouse message loop");
                break;
            }

            // hWnd MUST be NULL so the pump also receives THREAD messages
            // (WM_STOP_HOOK) — see key_hook::windows for the full rationale.
            let ret = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if ret == 0 || ret == -1 {
                break;
            }

            if msg.message == WM_STOP_HOOK {
                debug!("received WM_STOP_HOOK — exiting Raw Input mouse message loop");
                break;
            }

            DispatchMessageW(&msg);
        }

        let rid_remove = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_REMOVE,
            hwndTarget: std::ptr::null_mut(),
        };
        RegisterRawInputDevices(
            &rid_remove as *const RAWINPUTDEVICE,
            1,
            std::mem::size_of::<RAWINPUTDEVICE>() as u32,
        );

        DestroyWindow(hwnd);
    }

    TL_COLLECTOR.with(|c| *c.borrow_mut() = None);
    TL_RUNNING.with(|r| *r.borrow_mut() = None);

    hook_thread_id.store(0, Ordering::SeqCst);

    debug!("Windows Raw Input mouse hook exited");
}

/// Window procedure that handles `WM_INPUT` messages for `RIM_TYPEMOUSE`.
///
/// Decodes button state via `mouse_button_actions` and movement via
/// `compute_move_distance`, then forwards to `InputActivityCollector`.
/// Click position is resolved on-demand via `GetCursorPos`
/// (`crate::windows::get_mouse_position_windows`) at the moment of a
/// left-button-down event -- NOT polled every tick (that per-tick polling
/// path was removed as dead/wasteful, see `crate::activity`) -- so this is a
/// legitimate, bounded, event-triggered use of the same API.
#[cfg(target_os = "windows")]
unsafe extern "system" fn raw_input_mouse_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        let mut size: u32 = 0;
        let header_size = std::mem::size_of::<RAWINPUTHEADER>() as u32;

        GetRawInputData(
            lparam as _,
            RID_INPUT,
            std::ptr::null_mut(),
            &mut size,
            header_size,
        );

        if size == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }

        // 8-byte-aligned buffer -- see key_hook::windows::raw_input_wnd_proc
        // for the full alignment SAFETY rationale (identical here).
        let word_count = (size as usize).div_ceil(8);
        let mut aligned_buf = vec![0u64; word_count];
        let copied = GetRawInputData(
            lparam as _,
            RID_INPUT,
            aligned_buf.as_mut_ptr() as *mut std::ffi::c_void,
            &mut size,
            header_size,
        );

        if copied == u32::MAX {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }

        // SAFETY: identical alignment/bounds contract as
        // key_hook::windows::raw_input_wnd_proc (Vec<u64> guarantees 8-byte
        // alignment; `copied` <= `size` <= `word_count * 8`).
        let raw = &*(aligned_buf.as_ptr() as *const RAWINPUT);

        if raw.header.dwType == RIM_TYPEMOUSE {
            let mouse = &raw.data.mouse;
            let button_flags = u32::from(mouse.Anonymous.Anonymous.usButtonFlags);
            let actions = mouse_button_actions(button_flags);
            let is_absolute = mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0;
            let move_distance = compute_move_distance(mouse.lLastX, mouse.lLastY, is_absolute);

            TL_COLLECTOR.with(|c| {
                if let Some(ref collector) = *c.borrow() {
                    if actions.left_down {
                        match crate::windows::get_mouse_position_windows() {
                            Some(pos) => collector.record_click_at(pos.x, pos.y),
                            None => collector.record_click(),
                        }
                    }
                    if actions.right_down {
                        collector.record_right_click();
                    }
                    if actions.middle_down {
                        // No dedicated MouseActivity counter for the middle
                        // button -- count as a generic click (mirrors the
                        // macOS OtherMouseDown / Linux button-2 mapping).
                        collector.record_click();
                    }
                    if actions.wheel {
                        collector.record_scroll();
                    }
                    if let Some(distance) = move_distance {
                        collector.record_mouse_move(distance);
                    }
                }
            });
        }
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Stub fallback for non-Windows platforms. Only compiled when the module is
/// included on a non-Windows target (should not normally happen due to
/// `#[cfg(target_os = "windows")]` gating in `mod.rs`, but kept for safety --
/// mirrors `key_hook::windows`'s identical belt-and-braces stub).
#[cfg(not(target_os = "windows"))]
pub fn run_raw_input_mouse_hook(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    hook_thread_id: Arc<AtomicU32>,
) {
    let _ = (collector, running, hook_thread_id);
    warn!("Windows Raw Input mouse hook not yet implemented -- platform hook not active");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_does_not_panic() {
        let collector = Arc::new(InputActivityCollector::new());
        let running = Arc::new(AtomicBool::new(false));
        let hook_thread_id = Arc::new(AtomicU32::new(0));
        run_raw_input_mouse_hook(collector, running, hook_thread_id);
    }

    /// Mirrors `key_hook::windows::hook_thread_id_is_zero_after_return`: the
    /// published thread id must always return to `0` so a late
    /// `HookLifecycle::stop()` never posts `WM_STOP_HOOK` to a dead/recycled
    /// thread id.
    #[test]
    fn hook_thread_id_is_zero_after_return() {
        let collector = Arc::new(InputActivityCollector::new());
        let running = Arc::new(AtomicBool::new(false));
        let hook_thread_id = Arc::new(AtomicU32::new(0));
        run_raw_input_mouse_hook(collector, running, hook_thread_id.clone());
        assert_eq!(
            hook_thread_id.load(Ordering::SeqCst),
            0,
            "published hook_thread_id must be cleared/unpublished after return"
        );
    }

    // `wake_hook_thread(0)` no-op coverage moved to
    // `crate::hook_lifecycle::tests::stop_is_a_noop_wake_when_waker_thread_id_is_still_zero`
    // now that the wake step lives in `HookLifecycle::stop()` rather than a
    // per-hook helper function (#7727).

    #[test]
    fn mouse_button_actions_decodes_left_down() {
        let actions = mouse_button_actions(RI_MOUSE_LEFT_BUTTON_DOWN);
        assert!(actions.left_down);
        assert!(!actions.right_down);
        assert!(!actions.middle_down);
        assert!(!actions.wheel);
    }

    #[test]
    fn mouse_button_actions_decodes_right_down() {
        let actions = mouse_button_actions(RI_MOUSE_RIGHT_BUTTON_DOWN);
        assert!(actions.right_down);
        assert!(!actions.left_down);
    }

    #[test]
    fn mouse_button_actions_decodes_middle_down() {
        let actions = mouse_button_actions(RI_MOUSE_MIDDLE_BUTTON_DOWN);
        assert!(actions.middle_down);
    }

    #[test]
    fn mouse_button_actions_decodes_wheel() {
        let actions = mouse_button_actions(RI_MOUSE_WHEEL);
        assert!(actions.wheel);
    }

    #[test]
    fn mouse_button_actions_decodes_combined_flags() {
        // A single RAWINPUT message can carry multiple simultaneous button
        // transitions in one bitmask.
        let actions = mouse_button_actions(RI_MOUSE_LEFT_BUTTON_DOWN | RI_MOUSE_WHEEL);
        assert!(actions.left_down);
        assert!(actions.wheel);
        assert!(!actions.right_down);
        assert!(!actions.middle_down);
    }

    #[test]
    fn mouse_button_actions_zero_flags_is_all_false() {
        assert_eq!(mouse_button_actions(0), MouseButtonActions::default());
    }

    #[test]
    fn compute_move_distance_relative_delta() {
        // 3-4-5 triangle -> distance 5.0
        let distance = compute_move_distance(3, 4, false);
        assert!((distance.expect("must compute a distance") - 5.0).abs() < 1e-9);
    }

    #[test]
    fn compute_move_distance_zero_delta_is_none() {
        assert_eq!(compute_move_distance(0, 0, false), None);
    }

    #[test]
    fn compute_move_distance_absolute_mode_is_none() {
        // Absolute-coordinate devices (RDP/tablet) are intentionally skipped
        // rather than mis-accumulated as one huge "jump" distance.
        assert_eq!(compute_move_distance(500, 500, true), None);
    }
}
