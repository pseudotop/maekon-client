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
    GetRawInputData, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RIDEV_INPUTSINK, RIDEV_REMOVE, RID_INPUT, RIM_TYPEKEYBOARD,
};
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    PostThreadMessageW, RegisterClassW, HWND_MESSAGE, MSG, WM_INPUT, WM_USER, WNDCLASSW,
};

/// Custom message ID used to signal the message loop to exit.
#[cfg(target_os = "windows")]
const WM_STOP_HOOK: u32 = WM_USER + 1;

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

/// Run the Raw Input keyboard hook. Blocks until `running` becomes false.
///
/// Creates a hidden message-only window, registers a Raw Input keyboard
/// device with `RIDEV_INPUTSINK`, and processes `WM_INPUT` messages for
/// `RIM_TYPEKEYBOARD` events.
///
/// Each key-down event is classified via `classify_keycode()` and forwarded
/// to `InputActivityCollector::record_categorized_keystroke()`.
///
/// `hook_thread_id` is published with this thread's id so `KeyHook::stop()`
/// can `PostThreadMessageW(WM_STOP_HOOK)` to wake the loop blocked in
/// `GetMessageW()` on idle sessions where no input message arrives. The id is
/// published ONLY after the full setup sequence (RegisterClassW +
/// CreateWindowExW + RegisterRawInputDevices) succeeds, immediately before
/// entering the message pump. This guarantees that whenever stop() observes a
/// non-zero id, a live message queue exists to receive WM_STOP_HOOK, and it
/// closes the window where an early-return on a setup failure would otherwise
/// leave a stale id targeting a dead/recycled thread. The id is cleared back to
/// `0` before returning so stop() does not post to a recycled id.
#[cfg(target_os = "windows")]
pub fn run_raw_input_hook(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    hook_thread_id: Arc<AtomicU32>,
) {
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
        let class_name: Vec<u16> = "MaekonRawInputKeyHook\0".encode_utf16().collect();

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

        // Publish this thread's id now that the window + Raw Input device are
        // fully set up and we are about to enter the message pump. Publishing
        // here (rather than at function entry) ensures stop() never observes an
        // id for a thread that subsequently early-returned on a setup failure,
        // and that a live message queue exists to receive WM_STOP_HOOK.
        hook_thread_id.store(GetCurrentThreadId(), Ordering::SeqCst);

        // Message pump. Exits on WM_QUIT or when `running` is false.
        let mut msg: MSG = std::mem::zeroed();
        loop {
            // Check running flag before blocking on GetMessageW.
            if !running.load(Ordering::Relaxed) {
                debug!("running flag is false — exiting Raw Input message loop");
                break;
            }

            // hWnd MUST be NULL (not `hwnd`) so the pump also receives THREAD
            // messages (whose MSG.hwnd is NULL) posted by PostThreadMessageW.
            // With a non-NULL window filter, GetMessageW retrieves only messages
            // for that window and the WM_STOP_HOOK wakeup would be silently
            // dropped, leaving the loop blocked forever on idle. NULL retrieves
            // messages for any window owned by this thread (our message-only
            // window) PLUS thread messages, so WM_INPUT still arrives.
            let ret = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if ret == 0 || ret == -1 {
                // WM_QUIT (ret==0) or error (ret==-1)
                break;
            }

            // Check for our custom stop message (posted via PostThreadMessageW
            // from KeyHook::stop()).
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

    // Clear the published thread id so a late stop() does not post WM_STOP_HOOK
    // to a recycled thread id after this thread has exited.
    hook_thread_id.store(0, Ordering::SeqCst);

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

        // Allocate an 8-byte-aligned buffer and retrieve the raw input data.
        //
        // Vec<u8> provides only 1-byte alignment, but RAWINPUTHEADER begins
        // with a HANDLE (pointer-sized, 8-byte aligned on x86-64 Windows).
        // Casting a 1-aligned pointer to *const RAWINPUT is undefined
        // behaviour in Rust's memory model.  We use Vec<u64> instead: u64
        // has align_of 8, satisfying RAWINPUT's requirement on all supported
        // Windows targets (x86-64 and ARM64).
        //
        // Word count: ceil(size / 8) — the last word may be partially used;
        // GetRawInputData only writes `size` bytes so the tail is harmless.
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

        // SAFETY: `aligned_buf` is a Vec<u64> whose allocation has align_of
        // 8 bytes.  GetRawInputData wrote `copied` bytes starting at
        // aligned_buf.as_mut_ptr() (reinterpreted as *mut c_void above, which is
        // valid for byte-granularity writes into any allocation).  RAWINPUT
        // requires 8-byte alignment on Windows x86-64/ARM64; this is now
        // satisfied.  `copied` <= `size` <= `word_count * 8`, so all accessed
        // bytes are within the allocation.  The reference does not outlive
        // `aligned_buf`.
        let raw = &*(aligned_buf.as_ptr() as *const RAWINPUT);

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

/// Wake the hook thread's `GetMessageW` by posting `WM_STOP_HOOK` to its queue.
///
/// Called from `KeyHook::stop()` after the `running` flag is cleared. A
/// posted thread message is the only thing that unblocks `GetMessageW()` on an
/// idle machine where no input arrives, so without this the join in stop()
/// hangs indefinitely. A `thread_id` of `0` means the hook thread has not
/// published its id yet (or has already exited); we skip in that case.
#[cfg(target_os = "windows")]
pub fn wake_hook_thread(thread_id: u32) {
    if thread_id == 0 {
        return;
    }
    // SAFETY: PostThreadMessageW is thread-safe and takes a plain thread id with
    // no pointer arguments (wparam/lparam are 0). It returns FALSE if the target
    // thread has no message queue or already exited, which we treat as benign.
    let posted = unsafe { PostThreadMessageW(thread_id, WM_STOP_HOOK, 0, 0) };
    if posted == 0 {
        debug!(
            "PostThreadMessageW(WM_STOP_HOOK) failed (error={}); hook thread may have already exited",
            unsafe { GetLastError() }
        );
    }
}

/// Stub fallback for non-Windows platforms. Only compiled when the module is
/// included on a non-Windows target (should not normally happen due to
/// `#[cfg(target_os = "windows")]` gating in `mod.rs`, but kept for safety).
#[cfg(not(target_os = "windows"))]
pub fn run_raw_input_hook(
    collector: Arc<InputActivityCollector>,
    running: Arc<AtomicBool>,
    hook_thread_id: Arc<AtomicU32>,
) {
    let _ = (collector, running, hook_thread_id);
    warn!("Windows Raw Input key hook not yet implemented -- platform hook not active");
}

/// Stub fallback for non-Windows platforms; mirrors `wake_hook_thread`.
#[cfg(not(target_os = "windows"))]
pub fn wake_hook_thread(thread_id: u32) {
    let _ = thread_id;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_does_not_panic() {
        let collector = Arc::new(InputActivityCollector::new());
        let running = Arc::new(AtomicBool::new(false));
        let hook_thread_id = Arc::new(AtomicU32::new(0));
        // Should return immediately without panic
        run_raw_input_hook(collector, running, hook_thread_id);
    }

    /// Regression guard for `win-keyhook-threadid`: after `run_raw_input_hook`
    /// returns, the published thread id must always be `0`, so a concurrent or
    /// late `KeyHook::stop()` never posts `WM_STOP_HOOK` to a dead/recycled
    /// thread id.
    ///
    /// On non-Windows hosts (CI on macOS/Linux) this exercises the stub, which
    /// never publishes an id. On Windows, `running` starts `false`, so the hook
    /// either exits the pump immediately (clearing the id at the tail) or
    /// early-returns on a setup failure (the id was never published). Either
    /// way the post-condition is `0`.
    #[test]
    fn hook_thread_id_is_zero_after_return() {
        let collector = Arc::new(InputActivityCollector::new());
        let running = Arc::new(AtomicBool::new(false));
        let hook_thread_id = Arc::new(AtomicU32::new(0));
        run_raw_input_hook(collector, running, hook_thread_id.clone());
        assert_eq!(
            hook_thread_id.load(Ordering::SeqCst),
            0,
            "published hook_thread_id must be cleared/unpublished after return"
        );
    }

    /// `wake_hook_thread(0)` must be a no-op. This is the cross-platform safety
    /// net the ordering fix relies on: while the hook thread is still setting up
    /// (before it publishes its id) the stored value is `0`, and stop() must not
    /// post to thread id `0`.
    #[test]
    fn wake_hook_thread_zero_is_noop() {
        // Must not panic and must not attempt any thread-message post.
        wake_hook_thread(0);
    }

    /// Verifies that the aligned-buffer sizing arithmetic is correct: the
    /// allocated Vec<u64> always covers at least `size` bytes, and its base
    /// pointer carries at least 8-byte alignment (guaranteed by the allocator
    /// for u64).  This test exercises pure Rust with no Win32 calls, so it
    /// runs on all host platforms including macOS/Linux CI.
    #[test]
    fn aligned_buf_covers_requested_byte_size() {
        for size in [0usize, 1, 7, 8, 9, 15, 16, 17, 48, 49] {
            let word_count = size.div_ceil(8);
            let buf = vec![0u64; word_count];
            // Allocation in bytes is always >= requested size.
            assert!(
                buf.len() * 8 >= size,
                "word_count={word_count} covers only {} bytes, need {size}",
                buf.len() * 8,
            );
            // u64 is guaranteed align_of 8; verify the pointer satisfies it.
            assert_eq!(
                buf.as_ptr() as usize % 8,
                0,
                "Vec<u64> base pointer is not 8-byte aligned for size={size}"
            );
        }
    }
}
