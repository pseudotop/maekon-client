//! Platform mouse-activity hooks feeding `InputActivityCollector`.
//!
//! Spawns a dedicated OS thread that passively observes mouse events (button
//! down, scroll, move) and forwards them to `InputActivityCollector`'s
//! `record_click*`/`record_scroll`/`record_mouse_move` counters. Structurally
//! mirrors `crate::key_hook` (see that module for the keyboard analog) --
//! same lifecycle shape (start/stop, `AtomicBool` running flag, per-OS
//! observer thread), but mouse events need no keycode classification step,
//! so there is no `classify` sibling module here.
//!
//! The hook is purely passive -- it does NOT modify or block mouse events.
//!
//! Gated by the `input_activity` consent permission (#7652 HIGH-1): mouse
//! capture is user-monitoring, so it is spawned under the exact same
//! `consent.input_activity` gate the emitted `InputActivityEvent.mouse`
//! snapshot already uses (see `monitor_input_snapshot` in
//! `src-tauri/src/scheduler/loops/monitor.rs`).
//!
//! The running-flag + thread-handle + platform-wake lifecycle is shared with
//! `crate::key_hook` via `crate::hook_lifecycle::HookLifecycle` (#7727);
//! this module supplies only the OS-specific event loop body per platform.

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

use crate::hook_lifecycle::HookLifecycle;
use crate::input_activity::InputActivityCollector;
use std::sync::Arc;
use tracing::info;

/// Handle to a running platform mouse event observer.
///
/// Call `start()` to spawn the observer thread. Call `stop()` or drop the
/// handle to terminate the observer.
pub struct MouseHook {
    lifecycle: HookLifecycle,
}

impl MouseHook {
    /// Spawn the platform-specific mouse event observer on a dedicated thread.
    ///
    /// The observer calls `collector.record_click_at()` / `record_click()` /
    /// `record_right_click()` / `record_scroll()` / `record_mouse_move()` for
    /// each observed mouse event.
    ///
    /// Returns `None` if the platform does not support passive mouse
    /// observation (e.g., Linux Wayland without X11 fallback), or if the
    /// observer failed to install (e.g., missing Accessibility permission on
    /// macOS).
    pub fn start(collector: Arc<InputActivityCollector>) -> Option<Self> {
        let thread_name = format!("mouse-hook-{}", std::env::consts::OS);
        let lifecycle = HookLifecycle::start(thread_name, move |running, waker| {
            #[cfg(target_os = "macos")]
            macos::run_event_tap(collector, running, waker);
            #[cfg(target_os = "windows")]
            windows::run_raw_input_mouse_hook(collector, running, waker);
            #[cfg(target_os = "linux")]
            linux::run_x11_mouse_hook(collector, running, waker);
        })?;

        info!("mouse-activity hook started");

        Some(Self { lifecycle })
    }

    /// Signal the observer thread to stop and wait for it to exit.
    pub fn stop(&mut self) {
        self.lifecycle.stop();
        info!("mouse-activity hook stopped");
    }
}

impl Drop for MouseHook {
    fn drop(&mut self) {
        if self.lifecycle.is_running() {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `events.rs::reconcile_mouse_hook()` moves a `MouseHook` into
    /// `tokio::task::spawn_blocking` to call `stop()` off the async runtime
    /// thread (#7702 S1) -- `MouseHook` must stay `Send` for that call site
    /// to keep compiling. This is a compile-time-only check (no OS-level
    /// hook is ever installed), kept alongside the type it protects.
    ///
    /// `HookLifecycle`'s own generic mechanics -- the running-flag/join round
    /// trip formerly duplicated here as `mouse_hook_running_flag_starts_true`/
    /// `mouse_hook_stop_sets_running_false`, plus the three platform
    /// hang-regression guards -- are now covered exactly once in
    /// `crate::hook_lifecycle::tests` (#7727).
    #[test]
    fn mouse_hook_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MouseHook>();
    }
}
