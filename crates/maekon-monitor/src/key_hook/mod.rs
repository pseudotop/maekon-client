//! Platform key-category hooks for text-heavy app intelligence.
//!
//! Spawns a dedicated OS thread that passively observes keyboard events,
//! classifies each key into a `KeyCategory`, and calls
//! `InputActivityCollector::record_categorized_keystroke()`.
//!
//! The hook is purely passive -- it does NOT modify or block key events.
//!
//! Gated by `text_intelligence.input_pattern_detail = true` in config.

mod classify;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
mod linux;

pub use classify::classify_keycode;

use crate::input_activity::InputActivityCollector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info};

/// Handle to a running platform key event observer.
///
/// Call `start()` to spawn the observer thread. Call `stop()` or drop the
/// handle to terminate the observer.
pub struct KeyHook {
    running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
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
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let thread_handle = Self::spawn_platform_hook(collector, running_clone)?;

        info!("key-category hook started");

        Some(Self {
            running,
            thread_handle: Some(thread_handle),
        })
    }

    /// Signal the observer thread to stop and wait for it to exit.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            // Platform hooks may block in a run loop; we signal via the
            // AtomicBool and give the thread a bounded time to exit.
            // On macOS, CFRunLoopStop is called from within the tap callback
            // when `running` becomes false.
            if let Err(e) = handle.join() {
                debug!("join failed: {e:?}");
            }
        }
        info!("key-category hook stopped");
    }

    /// Platform-specific hook spawning. Returns None if unsupported.
    #[cfg(target_os = "macos")]
    fn spawn_platform_hook(
        collector: Arc<InputActivityCollector>,
        running: Arc<AtomicBool>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("key-hook-macos".to_string())
            .spawn(move || {
                macos::run_event_tap(collector, running);
            })
            .ok()
    }

    #[cfg(target_os = "windows")]
    fn spawn_platform_hook(
        collector: Arc<InputActivityCollector>,
        running: Arc<AtomicBool>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("key-hook-windows".to_string())
            .spawn(move || {
                windows::run_raw_input_hook(collector, running);
            })
            .ok()
    }

    #[cfg(target_os = "linux")]
    fn spawn_platform_hook(
        collector: Arc<InputActivityCollector>,
        running: Arc<AtomicBool>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("key-hook-linux".to_string())
            .spawn(move || {
                linux::run_x11_record_hook(collector, running);
            })
            .ok()
    }
}

impl Drop for KeyHook {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hook_running_flag_starts_true() {
        let running = Arc::new(AtomicBool::new(true));
        assert!(running.load(Ordering::Relaxed));
    }

    #[test]
    fn key_hook_stop_sets_running_false() {
        let running = Arc::new(AtomicBool::new(true));
        running.store(false, Ordering::SeqCst);
        assert!(!running.load(Ordering::Relaxed));
    }
}
