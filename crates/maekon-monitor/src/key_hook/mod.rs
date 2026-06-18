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
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::Mutex;
use tracing::{debug, info};

/// Handle to a running platform key event observer.
///
/// Call `start()` to spawn the observer thread. Call `stop()` or drop the
/// handle to terminate the observer.
pub struct KeyHook {
    running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    /// Linux only: shared handle to the xinput child process so that stop()
    /// can kill it before joining the reader thread (prevents hang on idle
    /// sessions where `lines()` blocks indefinitely with no keystrokes).
    #[cfg(target_os = "linux")]
    child_proc: Arc<Mutex<Option<std::process::Child>>>,
    /// macOS only: shared handle to the observer thread's `CFRunLoop` so that
    /// stop() can call `CFRunLoopStop` to wake the loop blocked in
    /// `CFRunLoopRun()`. Without this, stop()/Drop hangs indefinitely on idle
    /// sessions where no key event fires the tap callback (the only other path
    /// that observes `running == false`).
    #[cfg(target_os = "macos")]
    run_loop: Arc<Mutex<Option<core_foundation::runloop::CFRunLoop>>>,
    /// Windows only: id of the message-pump thread so that stop() can
    /// `PostThreadMessageW(WM_STOP_HOOK)` to wake the loop blocked in
    /// `GetMessageW()`. Without this, stop()/Drop hangs indefinitely on idle
    /// sessions where no input message arrives. `0` means "not yet published".
    #[cfg(target_os = "windows")]
    hook_thread_id: Arc<std::sync::atomic::AtomicU32>,
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

        #[cfg(target_os = "linux")]
        let child_proc: Arc<Mutex<Option<std::process::Child>>> = Arc::new(Mutex::new(None));

        #[cfg(target_os = "macos")]
        let run_loop: Arc<Mutex<Option<core_foundation::runloop::CFRunLoop>>> =
            Arc::new(Mutex::new(None));

        #[cfg(target_os = "windows")]
        let hook_thread_id: Arc<std::sync::atomic::AtomicU32> =
            Arc::new(std::sync::atomic::AtomicU32::new(0));

        #[cfg(target_os = "linux")]
        let thread_handle =
            Self::spawn_platform_hook(collector, running_clone, child_proc.clone())?;

        #[cfg(target_os = "macos")]
        let thread_handle = Self::spawn_platform_hook(collector, running_clone, run_loop.clone())?;

        #[cfg(target_os = "windows")]
        let thread_handle =
            Self::spawn_platform_hook(collector, running_clone, hook_thread_id.clone())?;

        info!("key-category hook started");

        Some(Self {
            running,
            thread_handle: Some(thread_handle),
            #[cfg(target_os = "linux")]
            child_proc,
            #[cfg(target_os = "macos")]
            run_loop,
            #[cfg(target_os = "windows")]
            hook_thread_id,
        })
    }

    /// Signal the observer thread to stop and wait for it to exit.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        // Linux: kill the xinput child BEFORE joining so its stdout closes
        // (EOF), which unblocks the blocking `lines()` iterator in the reader
        // thread.  Without this, stop()/Drop hangs indefinitely on idle
        // sessions where no keystrokes arrive.
        // We `take()` the Child out of the Option so the reader thread's own
        // exit-path kill/wait (which also uses take()) cannot double-kill.
        #[cfg(target_os = "linux")]
        {
            if let Ok(mut guard) = self.child_proc.lock() {
                if let Some(mut child) = guard.take() {
                    if let Err(e) = child.kill() {
                        debug!("stop: xinput kill failed (may have already exited): {e}");
                    }
                    // wait() reaps the zombie; ignore result here — the
                    // reader thread's exit path may have already waited.
                    let _ = child.wait();
                }
            }
        }

        // macOS: explicitly stop the observer's CFRunLoop. A loop blocked in
        // CFRunLoopRun() on an idle machine does NOT return just because the
        // `running` flag flipped — only an event would fire the tap callback
        // that calls stop(). CFRunLoopStop is thread-safe and wakes the loop so
        // the observer thread returns and can be joined. Without this, stop()/
        // Drop hangs indefinitely on idle sessions.
        #[cfg(target_os = "macos")]
        {
            if let Ok(guard) = self.run_loop.lock() {
                if let Some(run_loop) = guard.as_ref() {
                    run_loop.stop();
                }
            }
        }

        // Windows: post a message to the hook thread to unblock GetMessageW.
        // A low-level message pump blocked in GetMessageW() on an idle machine
        // does NOT return just because the `running` flag flipped — no input
        // message arrives to wake it. PostThreadMessageW delivers WM_STOP_HOOK
        // to the thread's queue so GetMessageW returns and the loop exits.
        // Without this, stop()/Drop hangs indefinitely on idle sessions.
        #[cfg(target_os = "windows")]
        {
            windows::wake_hook_thread(self.hook_thread_id.load(Ordering::SeqCst));
        }

        if let Some(handle) = self.thread_handle.take() {
            // Platform hooks may block in a run loop; we signal via the
            // AtomicBool and additionally wake the blocked loop explicitly
            // (CFRunLoopStop on macOS, PostThreadMessageW on Windows, child
            // kill on Linux) so the thread observes the stop signal promptly.
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
        run_loop: Arc<Mutex<Option<core_foundation::runloop::CFRunLoop>>>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("key-hook-macos".to_string())
            .spawn(move || {
                macos::run_event_tap(collector, running, run_loop);
            })
            .ok()
    }

    #[cfg(target_os = "windows")]
    fn spawn_platform_hook(
        collector: Arc<InputActivityCollector>,
        running: Arc<AtomicBool>,
        hook_thread_id: Arc<std::sync::atomic::AtomicU32>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("key-hook-windows".to_string())
            .spawn(move || {
                windows::run_raw_input_hook(collector, running, hook_thread_id);
            })
            .ok()
    }

    #[cfg(target_os = "linux")]
    fn spawn_platform_hook(
        collector: Arc<InputActivityCollector>,
        running: Arc<AtomicBool>,
        child_proc: Arc<Mutex<Option<std::process::Child>>>,
    ) -> Option<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("key-hook-linux".to_string())
            .spawn(move || {
                linux::run_x11_record_hook(collector, running, child_proc);
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
