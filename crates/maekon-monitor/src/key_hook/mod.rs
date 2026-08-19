//! Platform key-category hooks for text-heavy app intelligence.
//!
//! Spawns a dedicated OS thread that passively observes keyboard events,
//! classifies each key into a `KeyCategory`, and calls
//! `InputActivityCollector::record_categorized_keystroke()`.
//!
//! The hook is purely passive -- it does NOT modify or block key events.
//!
//! Gated by `text_intelligence.input_pattern_detail = true` in config.
//!
//! The running-flag + thread-handle + platform-wake lifecycle is shared with
//! `crate::mouse_hook` via `crate::hook_lifecycle::HookLifecycle` (#7727);
//! this module supplies only the OS-specific event loop body per platform.

mod classify;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

pub use classify::classify_keycode;

use crate::hook_lifecycle::HookLifecycle;
use crate::input_activity::InputActivityCollector;
use std::sync::Arc;
use tracing::info;

/// Handle to a running platform key event observer.
///
/// Call `start()` to spawn the observer thread. Call `stop()` or drop the
/// handle to terminate the observer.
pub struct KeyHook {
    lifecycle: HookLifecycle,
}

impl KeyHook {
    /// Spawn the platform-specific key event observer on a dedicated thread.
    ///
    /// The observer calls `collector.record_categorized_keystroke()` for each
    /// key-down event. Key-up events are ignored (we only count presses).
    ///
    /// Returns `None` if the platform does not support passive key observation
    /// (e.g., Linux Wayland without X11 fallback).
    pub fn start(collector: Arc<InputActivityCollector>) -> Option<Self> {
        let thread_name = format!("key-hook-{}", std::env::consts::OS);
        let lifecycle = HookLifecycle::start(thread_name, move |running, waker| {
            #[cfg(target_os = "macos")]
            macos::run_event_tap(collector, running, waker);
            #[cfg(target_os = "windows")]
            windows::run_raw_input_hook(collector, running, waker);
            #[cfg(target_os = "linux")]
            linux::run_x11_record_hook(collector, running, waker);
        })?;

        info!("key-category hook started");

        Some(Self { lifecycle })
    }

    /// Signal the observer thread to stop and wait for it to exit.
    pub fn stop(&mut self) {
        self.lifecycle.stop();
        info!("key-category hook stopped");
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        if self.lifecycle.is_running() {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `events.rs::reconcile_key_hook()` moves a `KeyHook` into
    /// `tokio::task::spawn_blocking` to call `stop()` off the async runtime
    /// thread (#7702 S1) -- `KeyHook` must stay `Send` for that call site to
    /// keep compiling. This is a compile-time-only check (no OS-level hook
    /// is ever installed), kept alongside the type it protects.
    ///
    /// `HookLifecycle`'s own generic mechanics -- the running-flag/join round
    /// trip formerly duplicated here as `key_hook_running_flag_starts_true`/
    /// `key_hook_stop_sets_running_false`, plus the three platform
    /// hang-regression guards -- are now covered exactly once in
    /// `crate::hook_lifecycle::tests` (#7727).
    #[test]
    fn key_hook_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<KeyHook>();
    }
}
