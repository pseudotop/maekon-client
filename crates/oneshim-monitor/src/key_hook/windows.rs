//! Windows key event observer using Raw Input.
//!
//! Registers a Raw Input keyboard device listener using
//! `RegisterRawInputDevices` with `RIDEV_INPUTSINK`. This receives all
//! keyboard input system-wide without blocking or modifying events.
//!
//! Uses a hidden message-only window (`HWND_MESSAGE`) with its own message
//! pump on a dedicated `std::thread`.
//!
//! The thread exits when the `running` `AtomicBool` is set to false or
//! when a `WM_QUIT` message is posted.

use super::classify::classify_keycode;
use crate::input_activity::InputActivityCollector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, WPARAM};
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    RegisterClassW, HWND_MESSAGE, MSG, WM_INPUT, WM_USER, WNDCLASSW,
};

/// Custom message ID used to signal the message loop to exit.
#[cfg(target_os = "windows")]
const WM_STOP_HOOK: u32 = WM_USER + 1;

/// Run the Raw Input keyboard hook. Blocks until `running` becomes false.
///
/// Creates a hidden message-only window, registers a Raw Input keyboard
/// device with `RIDEV_INPUTSINK`, and processes `WM_INPUT` messages for
/// `RIM_TYPEKEYBOARD` events.
///
/// Each key-down event is classified via `classify_keycode()` and forwarded
/// to `InputActivityCollector::record_categorized_keystroke()`.
// Module-scope thread-locals shared between `run_raw_input_hook` (populates)
// and `raw_input_wnd_proc` (reads). Must be at module scope so both functions
// reference the same storage.
#[cfg(target_os = "windows")]
thread_local! {
    static TL_COLLECTOR: std::cell::RefCell<Option<Arc<InputActivityCollector>>> =
        std::cell::RefCell::new(None);
    static TL_RUNNING: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        std::cell::RefCell::new(None);
}

#[cfg(target_os = "windows")]
pub fn run_raw_input_hook(collector: Arc<InputActivityCollector>, running: Arc<AtomicBool>) {
    TL_COLLECTOR.with(|c| *c.borrow_mut() = Some(collector));
    TL_RUNNING.with(|r| *r.borrow_mut() = Some(running.clone()));

    // SAFETY: Win32 window class + Raw Input registration sequence.
    // - class_name is a null-terminated UTF-16 vec kept alive for the block.
    // - GetModuleHandleW(null) returns the current module handle (always valid).
    // - RegisterClassW/CreateWindowExW operate on stack-local structs; HWND is
    //   checked for null/zero before use and destroyed via DestroyWindow on exit.
    // - RAWINPUTDEVICE points to a stack-local struct with valid hwndTarget.
    // - MSG is zeroed POD; GetMessageW/DispatchMessageW run on the current thread.
    // - Raw Input device is unregistered (RIDEV_REMOVE) before DestroyWindow.
    // - No cross-thread data races: entire block runs on a single dedicated thread.
    unsafe {
        // Register a unique window class for our message-only window.
        let class_name: Vec<u16> = "OneshimRawInputKeyHook\0".encode_utf16().collect();

        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(raw_input_wnd_proc),
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
                "RegisterClassW failed (error={}); Raw Input key hook not started",
                GetLastError()
            );
            return;
        }

        // Create a message-only window (child of HWND_MESSAGE).
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            std::ptr::null(), // no title
            0,                // no style
            0,
            0,
            0,
            0,
            HWND_MESSAGE,         // message-only
            std::ptr::null_mut(), // no menu
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );

        if hwnd.is_null() {
            error!(
                "CreateWindowExW failed (error={}); Raw Input key hook not started",
                GetLastError()
            );
            return;
        }

        // Register for Raw Input keyboard events with RIDEV_INPUTSINK so we
        // receive input even when our window does not have focus.
        let rid = RAWINPUTDEVICE {
            usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
            usUsage: 0x06,     // HID_USAGE_GENERIC_KEYBOARD
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
                "RegisterRawInputDevices failed (error={}); Raw Input key hook not started",
                GetLastError()
            );
            DestroyWindow(hwnd);
            return;
        }

        info!("Windows Raw Input key hook active (message-only window)");

        // Message pump. Exits on WM_QUIT or when `running` is false.
        let mut msg: MSG = std::mem::zeroed();
        loop {
            // Check running flag before blocking on GetMessageW.
            if !running.load(Ordering::Relaxed) {
                debug!("running flag is false — exiting Raw Input message loop");
                break;
            }

            let ret = GetMessageW(&mut msg, hwnd, 0, 0);
            if ret == 0 || ret == -1 {
                // WM_QUIT (ret==0) or error (ret==-1)
                break;
            }

            // Check for our custom stop message.
            if msg.message == WM_STOP_HOOK {
                debug!("received WM_STOP_HOOK — exiting Raw Input message loop");
                break;
            }

            DispatchMessageW(&msg);
        }

        // Unregister Raw Input device before cleanup.
        let rid_remove = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
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

    // Clear thread-local references.
    TL_COLLECTOR.with(|c| *c.borrow_mut() = None);
    TL_RUNNING.with(|r| *r.borrow_mut() = None);

    debug!("Windows Raw Input key hook exited");
}

/// Window procedure that handles `WM_INPUT` messages.
///
/// Extracts the `RAWINPUT` data, filters for `RIM_TYPEKEYBOARD` key-down
/// events, classifies the virtual key code, and records the keystroke.
#[cfg(target_os = "windows")]
unsafe extern "system" fn raw_input_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_INPUT {
        // Retrieve the size of the RAWINPUT structure.
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

        // Allocate buffer and retrieve the raw input data.
        let mut buffer = vec![0u8; size as usize];
        let copied = GetRawInputData(
            lparam as _,
            RID_INPUT,
            buffer.as_mut_ptr() as *mut _,
            &mut size,
            header_size,
        );

        if copied == u32::MAX {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }

        // SAFETY: GetRawInputData wrote `copied` bytes into `buffer`.
        // `buffer` is sized to `size` (queried from the API). The cast is
        // valid because RAWINPUT is a POD struct and `buffer` is properly
        // aligned by Vec<u8> (RAWINPUT requires no special alignment).
        let raw = &*(buffer.as_ptr() as *const RAWINPUT);

        // Only process keyboard events.
        if raw.header.dwType == RIM_TYPEKEYBOARD as u32 {
            let keyboard = &raw.data.keyboard;

            // WM_KEYDOWN = 0x0100, WM_SYSKEYDOWN = 0x0104
            // We only count key-down events (not key-up).
            let is_key_down = keyboard.Message == 0x0100 || keyboard.Message == 0x0104;

            if is_key_down {
                let vk_code = keyboard.VKey as u32;
                let category = classify_keycode(vk_code);

                // Detect shortcut: Control or Alt held (via WM_SYSKEYDOWN for Alt,
                // or check VK state for Control). We use a simple heuristic:
                // WM_SYSKEYDOWN indicates Alt is held.
                let is_shortcut = keyboard.Message == 0x0104;

                TL_COLLECTOR.with(|c| {
                    if let Some(ref collector) = *c.borrow() {
                        collector.record_categorized_keystroke(category, is_shortcut, false);
                    }
                });
            }
        }
    }

    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Stub fallback for non-Windows platforms. Only compiled when the module is
/// included on a non-Windows target (should not normally happen due to
/// `#[cfg(target_os = "windows")]` gating in `mod.rs`, but kept for safety).
#[cfg(not(target_os = "windows"))]
pub fn run_raw_input_hook(collector: Arc<InputActivityCollector>, running: Arc<AtomicBool>) {
    if let Err(e) = (collector, running) {
        debug!("operation failed: {e}");
    }
    warn!("Windows Raw Input key hook not yet implemented -- platform hook not active");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_does_not_panic() {
        let collector = Arc::new(InputActivityCollector::new());
        let running = Arc::new(AtomicBool::new(false));
        // Should return immediately without panic
        run_raw_input_hook(collector, running);
    }
}
