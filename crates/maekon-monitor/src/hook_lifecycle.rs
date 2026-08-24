//! Generic lifecycle skeleton shared by `key_hook` and `mouse_hook` (#7727).
//!
//! Both hooks spawn a dedicated OS thread that blocks in a platform-specific
//! event loop (a macOS `CFRunLoop`, a Windows message pump, or a Linux
//! `xinput` subprocess reader) until told to stop. Before this module
//! existed, the running-flag + thread-handle bookkeeping AND all three
//! platform-specific "how do I wake a thread blocked in an OS call" fixes
//! were duplicated verbatim between `key_hook/mod.rs` and `mouse_hook/mod.rs`
//! (~85% identical), with comments already starting to drift between the two
//! copies. `HookLifecycle` now owns that skeleton exactly once; `KeyHook` and
//! `MouseHook` are thin wrappers that supply only their OS-specific event
//! loop body (`key_hook::macos::run_event_tap`, `mouse_hook::linux::
//! run_x11_mouse_hook`, etc.) as a closure.
//!
//! # The three hang fixes (each lived in two places; now lives in one)
//!
//! A hook thread blocks in a call that does NOT observe the `running`
//! `AtomicBool` on its own -- flipping the flag to `false` is necessary but
//! not sufficient to unblock it. `stop()` additionally "wakes" the blocked
//! call through a platform-specific slot (the [`PlatformWaker`]) that the
//! thread body publishes itself into just before it blocks:
//!
//! - **macOS**: the thread blocks in `CFRunLoop::run_current()`. On an idle
//!   session with no key/mouse event, nothing ever fires the tap callback
//!   that would observe `running == false`, so the loop never returns on its
//!   own. `stop()` calls the thread-safe `CFRunLoopStop` through the
//!   published `CFRunLoop` handle to wake it directly.
//! - **Windows**: the thread blocks in `GetMessageW()`. On an idle session
//!   with no input message, `GetMessageW` never returns on its own either.
//!   `stop()` posts a `WM_STOP_HOOK` thread message via `PostThreadMessageW`
//!   to the published thread id, which `GetMessageW` retrieves and the
//!   message-pump loop then breaks on.
//! - **Linux**: the thread blocks in a `BufReader::lines()` iterator reading
//!   the `xinput test-xi2` child process's stdout. On an idle session with no
//!   keystroke/mouse activity, the child never writes a line and `lines()`
//!   never returns `None` on its own. `stop()` kills the published child
//!   process, which closes its stdout and delivers EOF, unblocking the
//!   reader.
//!
//! Without the relevant wake step, `stop()`/`Drop` hangs indefinitely on an
//! idle session -- this is exactly the class of bug this module exists to
//! guard against just once instead of per-hook (see the hang-regression
//! tests at the bottom of this file).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::debug;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::Mutex;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicU32;
#[cfg(target_os = "windows")]
use windows_sys::Win32::Foundation::GetLastError;
#[cfg(target_os = "windows")]
use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_USER};

/// The platform-specific "wake a blocked thread" slot passed to a hook's
/// thread body and read back by [`HookLifecycle::stop`]. Each hook's thread
/// body publishes its own live handle into this slot immediately before it
/// blocks (see the module doc for the exact mechanism per platform).
#[cfg(target_os = "linux")]
pub type PlatformWaker = Arc<Mutex<Option<std::process::Child>>>;

/// macOS variant: a shared slot for the observer thread's `CFRunLoop`, so
/// `stop()` can call `CFRunLoopStop` to wake a loop blocked in
/// `CFRunLoopRun()` with no event ever firing the tap callback.
#[cfg(target_os = "macos")]
pub type PlatformWaker = Arc<Mutex<Option<core_foundation::runloop::CFRunLoop>>>;

/// Maximum time a macOS hook run loop may remain blocked if `stop()` races
/// with the small interval between checking `running` and entering
/// `CFRunLoopRunInMode`.
///
/// `CFRunLoopStop` only terminates a currently running activation. The normal
/// path still wakes immediately through [`PlatformWaker`]; this bounded wait
/// is the fail-safe that makes an earlier stop request persistent through the
/// atomic `running` flag instead of relying on Core Foundation to remember it.
#[cfg(target_os = "macos")]
const MACOS_RUN_LOOP_STOP_FALLBACK: Duration = Duration::from_secs(1);

/// Publish and run the current macOS hook run loop until `running` becomes
/// false.
///
/// Key and mouse hooks share this helper so their stop-before-entry behavior
/// cannot drift. `stop()` normally interrupts the active run immediately via
/// `CFRunLoopStop`; the bounded `run_in_mode` interval closes the remaining
/// race where that call arrives just before the run-loop activation exists.
#[cfg(target_os = "macos")]
#[mutants::skip]
pub(crate) fn run_macos_loop_until_stopped(
    running: Arc<AtomicBool>,
    run_loop: PlatformWaker,
    current_loop: core_foundation::runloop::CFRunLoop,
) {
    run_macos_loop_until_stopped_impl(running, run_loop, current_loop, || {});
}

#[cfg(target_os = "macos")]
#[mutants::skip]
fn run_macos_loop_until_stopped_impl<F>(
    running: Arc<AtomicBool>,
    run_loop: PlatformWaker,
    current_loop: core_foundation::runloop::CFRunLoop,
    mut before_wait: F,
) where
    F: FnMut(),
{
    if let Ok(mut guard) = run_loop.lock() {
        *guard = Some(current_loop);
    }

    while running.load(Ordering::SeqCst) {
        // Test-only callers use this hook to force stop() into the otherwise
        // microscopic interval immediately before the Core Foundation call.
        before_wait();

        // SAFETY: kCFRunLoopDefaultMode is a process-lifetime Core Foundation
        // constant. The event-tap sources and the hang-regression timer are
        // registered in this mode (or common modes, which include it).
        let result = core_foundation::runloop::CFRunLoop::run_in_mode(
            unsafe { core_foundation::runloop::kCFRunLoopDefaultMode },
            MACOS_RUN_LOOP_STOP_FALLBACK,
            false,
        );
        if matches!(
            result,
            core_foundation::runloop::CFRunLoopRunResult::Finished
        ) {
            break;
        }
    }

    if let Ok(mut guard) = run_loop.lock() {
        *guard = None;
    }
}

/// Windows variant: the id of the message-pump thread, so `stop()` can
/// `PostThreadMessageW(WM_STOP_HOOK)` to wake a loop blocked in
/// `GetMessageW()`. `0` means "not yet published" (or already exited).
#[cfg(target_os = "windows")]
pub type PlatformWaker = Arc<AtomicU32>;

/// Custom thread-message id used to wake a Windows hook thread's
/// `GetMessageW` pump on `stop()`. `key_hook::windows` and
/// `mouse_hook::windows`'s message loops each compare
/// `msg.message == hook_lifecycle::WM_STOP_HOOK` on their own separate
/// message-only windows/threads; the value is defined once here so the two
/// message pumps and this module's poster can never drift out of sync.
#[cfg(target_os = "windows")]
pub(crate) const WM_STOP_HOOK: u32 = WM_USER + 1;

/// Generic handle to a running platform hook observer thread.
///
/// Owns the running-flag + thread-handle + platform waker bookkeeping that
/// `KeyHook` and `MouseHook` used to each duplicate. Call [`start`] to spawn
/// the observer thread with a platform-specific body closure. Call [`stop`]
/// or drop the handle to terminate the observer.
///
/// [`start`]: HookLifecycle::start
/// [`stop`]: HookLifecycle::stop
pub struct HookLifecycle {
    running: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
    waker: PlatformWaker,
}

impl HookLifecycle {
    /// Spawn `body` on a dedicated, named OS thread.
    ///
    /// `body` receives a clone of the `running` flag (it must poll this and
    /// return once it observes `false`) and a clone of a fresh
    /// [`PlatformWaker`] slot (it must publish its own live blocking-call
    /// handle into this slot immediately before it blocks, so `stop()` can
    /// wake it -- see the module doc). Returns `None` only if the OS thread
    /// could not be spawned at all.
    pub fn start<F>(thread_name: impl Into<String>, body: F) -> Option<Self>
    where
        F: FnOnce(Arc<AtomicBool>, PlatformWaker) + Send + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let waker = PlatformWaker::default();

        let running_for_thread = running.clone();
        let waker_for_thread = waker.clone();
        let thread_handle = std::thread::Builder::new()
            .name(thread_name.into())
            .spawn(move || body(running_for_thread, waker_for_thread))
            .ok()?;

        Some(Self {
            running,
            thread_handle: Some(thread_handle),
            waker,
        })
    }

    /// `true` while the observer thread handle is still owned here, i.e.
    /// [`stop`](Self::stop) has not yet run to completion. Callers use this
    /// to avoid double-stopping (and double-logging) from both an explicit
    /// `stop()` call and a subsequent `Drop`.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.thread_handle.is_some()
    }

    /// Signal the observer thread to stop, wake it via the platform-specific
    /// mechanism if it is blocked in an OS call that does not observe the
    /// `running` flag on its own, and join it.
    ///
    /// Safe to call more than once: a second call is a no-op (the thread
    /// handle has already been taken, and re-triggering the platform wake
    /// step on an already-stopped/absent waker is harmless).
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        // Linux: kill the hook's child process BEFORE joining so its stdout
        // closes (EOF), which unblocks the blocking `lines()` iterator in the
        // reader thread. Without this, stop()/Drop hangs indefinitely on idle
        // sessions where no keystroke/mouse activity arrives. We `take()` the
        // Child out of the slot so the reader thread's own exit-path
        // kill/wait (which also uses take()) cannot double-kill.
        #[cfg(target_os = "linux")]
        {
            if let Ok(mut guard) = self.waker.lock() {
                if let Some(mut child) = guard.take() {
                    if let Err(e) = child.kill() {
                        debug!("stop: hook child kill failed (may have already exited): {e}");
                    }
                    // wait() reaps the zombie; ignore the result here -- the
                    // reader thread's exit path may have already waited.
                    let _ = child.wait();
                }
            }
        }

        // macOS: explicitly stop the observer's CFRunLoop. A loop blocked in
        // CFRunLoopRun() on an idle machine does NOT return just because the
        // `running` flag flipped -- only an event would fire the tap callback
        // that calls stop(). CFRunLoopStop is thread-safe and wakes the loop
        // so the observer thread returns and can be joined. Without this,
        // stop()/Drop hangs indefinitely on idle sessions.
        #[cfg(target_os = "macos")]
        {
            if let Ok(guard) = self.waker.lock() {
                if let Some(run_loop) = guard.as_ref() {
                    run_loop.stop();
                }
            }
        }

        // Windows: post a message to the hook thread to unblock GetMessageW.
        // A low-level message pump blocked in GetMessageW() on an idle
        // machine does NOT return just because the `running` flag flipped --
        // no input message arrives to wake it. PostThreadMessageW delivers
        // WM_STOP_HOOK to the thread's queue so GetMessageW returns and the
        // loop exits. Without this, stop()/Drop hangs indefinitely on idle
        // sessions. A thread id of `0` means the hook thread has not
        // published its id yet (or has already exited); skip in that case.
        #[cfg(target_os = "windows")]
        {
            let thread_id = self.waker.load(Ordering::SeqCst);
            if thread_id != 0 {
                // SAFETY: PostThreadMessageW is thread-safe and takes a plain
                // thread id with no pointer arguments (wparam/lparam are 0).
                // It returns FALSE if the target thread has no message queue
                // or already exited, which we treat as benign.
                let posted = unsafe { PostThreadMessageW(thread_id, WM_STOP_HOOK, 0, 0) };
                if posted == 0 {
                    debug!(
                        "PostThreadMessageW(WM_STOP_HOOK) failed (error={}); hook thread may \
                         have already exited",
                        unsafe { GetLastError() }
                    );
                }
            }
        }

        if let Some(handle) = self.thread_handle.take() {
            // Platform hooks may block in a run loop; we signal via the
            // AtomicBool and additionally wake the blocked loop explicitly
            // (CFRunLoopStop on macOS, PostThreadMessageW on Windows, child
            // kill on Linux, all above) so the thread observes the stop
            // signal promptly instead of only on its next event.
            if let Err(e) = handle.join() {
                debug!("join failed: {e:?}");
            }
        }
    }
}

impl Drop for HookLifecycle {
    fn drop(&mut self) {
        if self.thread_handle.is_some() {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Ported from `key_hook`/`mouse_hook`'s trivial per-copy tests (kept
    /// exactly once here now that both hooks share this skeleton).
    #[test]
    fn running_flag_starts_true() {
        let running = Arc::new(AtomicBool::new(true));
        assert!(running.load(Ordering::Relaxed));
    }

    /// Ported from `key_hook`/`mouse_hook`'s trivial per-copy tests (kept
    /// exactly once here now that both hooks share this skeleton).
    #[test]
    fn stop_sets_running_false() {
        let running = Arc::new(AtomicBool::new(true));
        running.store(false, Ordering::SeqCst);
        assert!(!running.load(Ordering::Relaxed));
    }

    /// Base skeleton round trip: a body that only polls the `running` flag
    /// (no OS-specific blocking call) must be started and stopped/joined
    /// promptly. Exercises the generic thread-spawn + flag + join mechanics
    /// on every host platform.
    #[test]
    fn start_then_stop_joins_a_flag_polling_body() {
        let mut lifecycle =
            HookLifecycle::start("hook-lifecycle-test-flag-poll", |running, _waker| {
                while running.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .expect("thread spawn must succeed");

        assert!(lifecycle.is_running());
        lifecycle.stop();
        assert!(!lifecycle.is_running());
    }

    /// `Drop` must stop a still-running lifecycle (mirrors `KeyHook`'s/
    /// `MouseHook`'s own `Drop` -> `stop()` delegation, exercised here at the
    /// generic layer).
    #[test]
    fn drop_stops_a_flag_polling_body() {
        let (started_tx, started_rx) = mpsc::channel();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        {
            let _lifecycle =
                HookLifecycle::start("hook-lifecycle-test-drop", move |running, _waker| {
                    let _ = started_tx.send(());
                    while running.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    let _ = stopped_tx.send(());
                })
                .expect("thread spawn must succeed");
            started_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("body must start promptly");
            // `_lifecycle` drops here, at end of scope.
        }
        stopped_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("Drop must stop the body promptly");
    }

    /// No-op `CFRunLoopTimer` callback used only to give the hang-regression
    /// test's run loop something to wait on (see below) -- a run loop with no
    /// source/timer/observer registered returns from `CFRunLoopRun()`
    /// immediately regardless of `CFRunLoopStop`, which would make the test
    /// pass vacuously even with the fix reverted. The real `key_hook`/
    /// `mouse_hook` tap callbacks already provide this "something to wait
    /// on" role via their registered `CGEventTap` source; this timer is the
    /// test-only stand-in for that.
    #[cfg(target_os = "macos")]
    extern "C" fn hang_test_noop_timer_callback(
        _timer: core_foundation::runloop::CFRunLoopTimerRef,
        _info: *mut std::ffi::c_void,
    ) {
    }

    /// Revert-proof hang-regression guard (macOS): a body that registers a
    /// far-future (never-fires-during-the-test) timer so the loop has
    /// something to wait on, publishes its `CFRunLoop` into the waker slot,
    /// and then blocks in `CFRunLoop::run_current()` -- exactly mirroring
    /// `key_hook::macos::run_event_tap` and `mouse_hook::macos::run_event_tap`
    /// on an idle session where no key or mouse event ever fires the tap
    /// callback. The ONLY thing that can unblock it before the timer's fire
    /// date is `stop()`'s `CFRunLoopStop` call through the waker. If that
    /// call were ever removed or reordered after the join, this test times
    /// out instead of passing.
    ///
    /// The `ready` handshake exists only to avoid a test-only startup race
    /// (spawning the stopper thread before the body has published its
    /// `CFRunLoop`); it has no bearing on the production wake mechanism
    /// under test.
    #[cfg(target_os = "macos")]
    #[test]
    fn stop_wakes_a_blocked_cfrunloop_on_an_idle_session() {
        use core_foundation::date::CFDate;
        use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopTimer};

        let (ready_tx, ready_rx) = mpsc::channel();
        let mut lifecycle = HookLifecycle::start(
            "hook-lifecycle-test-macos-hang",
            move |running, run_loop| {
                // Fire an hour from now -- never fires during this test, but
                // gives CFRunLoopRun() a timer to wait on so it genuinely
                // blocks instead of returning immediately (see the callback
                // doc above).
                let fire_date = CFDate::now().abs_time() + 3600.0;
                let timer = CFRunLoopTimer::new(
                    fire_date,
                    0.0,
                    0,
                    0,
                    hang_test_noop_timer_callback,
                    std::ptr::null_mut(),
                );
                let current_loop = CFRunLoop::get_current();
                // SAFETY: `kCFRunLoopDefaultMode` is a read-only extern
                // static CFStringRef owned by CoreFoundation; reading it is
                // the documented way to reference the default mode (mirrors
                // `key_hook::macos`'s identical `unsafe { kCFRunLoopCommonModes }`
                // access).
                current_loop.add_timer(&timer, unsafe { kCFRunLoopDefaultMode });
                let _ = ready_tx.send(());
                run_macos_loop_until_stopped(running, run_loop, current_loop);
            },
        )
        .expect("thread spawn must succeed");

        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("body must publish its CFRunLoop before blocking");

        let (done_tx, done_rx) = mpsc::channel();
        let stopper = std::thread::spawn(move || {
            lifecycle.stop();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(Duration::from_secs(5)).expect(
            "stop() must wake the blocked CFRunLoop via CFRunLoopStop and return promptly \
             -- hang regression",
        );
        stopper.join().expect("stopper thread must not panic");
    }

    /// Deterministically force the stop-before-run-loop-entry ordering that a
    /// plain `CFRunLoopStop` cannot close: the helper has observed
    /// `running == true`, but its Core Foundation activation does not exist
    /// yet. The stop call is therefore intentionally delivered before the
    /// activation and the bounded run interval must still return promptly.
    #[cfg(target_os = "macos")]
    #[test]
    fn stop_before_cfrunloop_entry_cannot_be_lost() {
        use core_foundation::date::CFDate;
        use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopTimer};

        let running = Arc::new(AtomicBool::new(true));
        let run_loop = PlatformWaker::default();
        let thread_running = running.clone();
        let thread_run_loop = run_loop.clone();
        let (before_wait_tx, before_wait_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let worker = std::thread::spawn(move || {
            let fire_date = CFDate::now().abs_time() + 3600.0;
            let timer = CFRunLoopTimer::new(
                fire_date,
                0.0,
                0,
                0,
                hang_test_noop_timer_callback,
                std::ptr::null_mut(),
            );
            let current_loop = CFRunLoop::get_current();
            current_loop.add_timer(&timer, unsafe { kCFRunLoopDefaultMode });

            let mut first_wait = true;
            run_macos_loop_until_stopped_impl(
                thread_running,
                thread_run_loop,
                current_loop,
                || {
                    if first_wait {
                        first_wait = false;
                        before_wait_tx
                            .send(())
                            .expect("test coordinator must still be waiting");
                        continue_rx
                            .recv_timeout(Duration::from_secs(2))
                            .expect("test coordinator must release run-loop entry");
                    }
                },
            );
            let _ = done_tx.send(());
        });

        before_wait_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("worker must pause immediately before run-loop entry");

        // Mirror HookLifecycle::stop ordering while the worker is known not to
        // have entered CFRunLoopRunInMode yet. CFRunLoopStop cannot terminate
        // a future activation, so the running flag plus bounded interval must
        // carry the stop request across this gap.
        running.store(false, Ordering::SeqCst);
        if let Ok(guard) = run_loop.lock() {
            guard
                .as_ref()
                .expect("worker must publish the run loop before waiting")
                .stop();
        }
        continue_tx
            .send(())
            .expect("worker must still be paused before run-loop entry");

        done_rx
            .recv_timeout(Duration::from_secs(3))
            .expect("bounded run-loop fallback must preserve the early stop request");
        worker.join().expect("worker thread must not panic");
    }

    /// Revert-proof hang-regression guard (Linux): a body that spawns a
    /// long-lived child process, publishes it into the waker slot, and then
    /// blocks reading the child's stdout via `BufReader::lines()` -- exactly
    /// mirroring `key_hook::linux::run_x11_record_hook` and
    /// `mouse_hook::linux::run_x11_mouse_hook` on an idle session where the
    /// `xinput` child never writes a line. The child (`sleep 30`) produces no
    /// output and would otherwise block the reader for the full 30 seconds;
    /// the ONLY thing that unblocks it promptly is `stop()` killing the
    /// child (closing its stdout -> EOF). If that kill step were ever
    /// removed or reordered after the join, this test times out well before
    /// the child's natural 30s exit.
    #[cfg(target_os = "linux")]
    #[test]
    fn stop_wakes_a_blocked_child_reader_on_an_idle_session() {
        use std::io::BufRead;

        let (ready_tx, ready_rx) = mpsc::channel();
        let mut lifecycle = HookLifecycle::start(
            "hook-lifecycle-test-linux-hang",
            move |_running, child_proc| {
                let mut child = match std::process::Command::new("sleep")
                    .arg("30")
                    .stdout(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(c) => c,
                    Err(_) => {
                        // `sleep` missing on this host -- signal ready anyway
                        // so the test does not hang waiting for a handshake
                        // that will never arrive.
                        let _ = ready_tx.send(());
                        return;
                    }
                };
                let stdout = child.stdout.take();
                if let Ok(mut guard) = child_proc.lock() {
                    *guard = Some(child);
                }
                let _ = ready_tx.send(());
                if let Some(stdout) = stdout {
                    let reader = std::io::BufReader::new(stdout);
                    // Blocks until EOF -- `sleep 30` writes nothing, so only
                    // stop()'s child.kill() (closing stdout) unblocks this
                    // promptly.
                    for line in reader.lines() {
                        if line.is_err() {
                            break;
                        }
                    }
                }
            },
        )
        .expect("thread spawn must succeed");

        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("body must publish its child process before blocking");

        let (done_tx, done_rx) = mpsc::channel();
        let stopper = std::thread::spawn(move || {
            lifecycle.stop();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(Duration::from_secs(10)).expect(
            "stop() must kill the blocked child reader and return promptly \
             -- hang regression",
        );
        stopper.join().expect("stopper thread must not panic");
    }

    /// Revert-proof hang-regression guard (Windows): a body that publishes
    /// its own thread id into the waker slot and then blocks in
    /// `GetMessageW()` with no window and no input message ever arriving --
    /// exactly mirroring `key_hook::windows::run_raw_input_hook` and
    /// `mouse_hook::windows::run_raw_input_mouse_hook` on an idle session.
    /// `PostThreadMessageW` delivers a thread message purely via the
    /// thread's message queue, so no window is needed to reproduce the real
    /// hang: the ONLY thing that unblocks `GetMessageW()` here is `stop()`'s
    /// `PostThreadMessageW(WM_STOP_HOOK)` call. If that call were ever
    /// removed or reordered after the join, this test hangs.
    #[cfg(target_os = "windows")]
    #[test]
    fn stop_wakes_a_blocked_message_pump_on_an_idle_session() {
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG};

        let (ready_tx, ready_rx) = mpsc::channel();
        let mut lifecycle = HookLifecycle::start(
            "hook-lifecycle-test-windows-hang",
            move |_running, hook_thread_id| {
                // SAFETY: GetCurrentThreadId has no preconditions.
                let tid = unsafe { GetCurrentThreadId() };
                hook_thread_id.store(tid, Ordering::SeqCst);
                let _ = ready_tx.send(());

                // SAFETY: `msg` is a zeroed POD struct; GetMessageW with a
                // null window handle blocks on this thread's message queue
                // only (no window is required to receive a thread message
                // posted via PostThreadMessageW).
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                unsafe {
                    GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                }
            },
        )
        .expect("thread spawn must succeed");

        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("body must publish its thread id before blocking");

        let (done_tx, done_rx) = mpsc::channel();
        let stopper = std::thread::spawn(move || {
            lifecycle.stop();
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(Duration::from_secs(5)).expect(
            "stop() must wake the blocked GetMessageW via PostThreadMessageW and return \
             promptly -- hang regression",
        );
        stopper.join().expect("stopper thread must not panic");
    }

    /// Ported from `key_hook::windows`/`mouse_hook::windows`'s
    /// `wake_hook_thread_zero_is_noop` (kept exactly once here now that the
    /// wake step lives in `stop()` rather than a per-hook helper function).
    /// `stop()` on a lifecycle whose Windows waker never got past `0`
    /// (published id not yet set) must not attempt to post a thread message
    /// and must not panic.
    #[cfg(target_os = "windows")]
    #[test]
    fn stop_is_a_noop_wake_when_waker_thread_id_is_still_zero() {
        let mut lifecycle = HookLifecycle::start(
            "hook-lifecycle-test-windows-zero-id",
            |running, _hook_thread_id| {
                // Never publishes a thread id; exits as soon as `running`
                // flips (no blocking call at all).
                while running.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
            },
        )
        .expect("thread spawn must succeed");
        lifecycle.stop();
        assert!(!lifecycle.is_running());
    }
}
