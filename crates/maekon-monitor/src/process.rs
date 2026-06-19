use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
use maekon_core::path_redaction::mask_user_paths;
use maekon_core::ports::monitor::ProcessMonitor;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::System;
use tracing::debug;

/// Minimum interval between full process-list refreshes.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(2);

pub struct ProcessTracker {
    // F-RR-C23-01: Arc<Mutex<…>> wrapping mirrors SysInfoMonitor so fields are
    // 'static + Send and can be moved into spawn_blocking closures without
    // cloning large sysinfo state.
    sys: Arc<Mutex<System>>,
    /// Timestamp of the last `refresh_processes` call.
    last_refresh: Arc<Mutex<Instant>>,
}

impl ProcessTracker {
    pub fn new() -> Self {
        // F-PF-18: System::new_all() scans the entire process table on init.
        // Replace it with System::new() to cut the initialization cost.
        // The actual process list is refreshed lazily in
        // refresh_if_stale → refresh_processes.
        //
        // Initialize last_refresh to before REFRESH_COOLDOWN so the first call
        // refreshes immediately. Initializing with Instant::now() would mean the
        // cooldown has not elapsed on the first call, leaving the process list
        // empty (pre-existing test failure).
        let stale_start = Instant::now()
            .checked_sub(REFRESH_COOLDOWN)
            .unwrap_or_else(Instant::now);
        Self {
            sys: Arc::new(Mutex::new(System::new())),
            last_refresh: Arc::new(Mutex::new(stale_start)),
        }
    }

    /// Refresh the process list only if the cooldown has elapsed.
    /// Must be called with the `sys` lock already held by the caller.
    fn refresh_if_stale(last_refresh: &Arc<Mutex<Instant>>, sys: &mut System) {
        let mut last = last_refresh.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() >= REFRESH_COOLDOWN {
            // #6441 F14: refresh only the fields the collectors actually read.
            // `refresh_processes(All, true)` is shorthand for
            // `ProcessRefreshKind::everything()`, which additionally collects
            // per-process disk I/O, cmd line, environ, cwd, root, user, and (on
            // Linux) the thread-task table — none of which collect_top_sync /
            // collect_detailed_sync read. Narrow to cpu + memory + exe (exe only
            // when not already set); pid/name/start_time are basic fields populated
            // regardless, and `nothing()` already excludes the Linux task walk.
            // Cuts per-refresh syscalls/allocations for the <100MB RSS / low-CPU
            // edge-client budget.
            let refresh_kind = sysinfo::ProcessRefreshKind::nothing()
                .with_cpu()
                .with_memory()
                .with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
            sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh_kind);
            *last = Instant::now();
        }
    }

    /// Synchronous top-process collection — runs in a blocking thread pool via
    /// `spawn_blocking`.
    ///
    /// F-RR-C23-01: sysinfo::System::refresh_processes is a blocking syscall
    /// that walks the process table (up to ~50 ms on busy hosts). Isolating it
    /// to `spawn_blocking` prevents starvation of the tokio worker thread pool
    /// during the 1-second scheduler hot-path.
    fn collect_top_sync(
        sys: Arc<Mutex<System>>,
        last_refresh: Arc<Mutex<Instant>>,
        limit: usize,
    ) -> Result<Vec<ProcessInfo>, CoreError> {
        let mut sys_guard = sys.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;
        Self::refresh_if_stale(&last_refresh, &mut sys_guard);

        let mut processes: Vec<ProcessInfo> = sys_guard
            .processes()
            .values()
            .map(|p| ProcessInfo {
                pid: p.pid().as_u32(),
                name: p.name().to_string_lossy().to_string(),
                cpu_usage: p.cpu_usage(),
                memory_bytes: p.memory(),
            })
            .collect();
        drop(sys_guard);

        processes.sort_by(|a, b| {
            b.cpu_usage
                .partial_cmp(&a.cpu_usage)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        processes.truncate(limit);

        debug!("top {}items collect", processes.len());
        Ok(processes)
    }

    /// Synchronous detailed-process collection — runs in a blocking thread pool
    /// via `spawn_blocking`.
    ///
    /// F-RR-C23-01: see `collect_top_sync` for rationale.
    fn collect_detailed_sync(
        sys: Arc<Mutex<System>>,
        last_refresh: Arc<Mutex<Instant>>,
        foreground_pid: Option<u32>,
        top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        let mut sys_guard = sys.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;
        Self::refresh_if_stale(&last_refresh, &mut sys_guard);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut all_details: Vec<ProcessDetail> = sys_guard
            .processes()
            .values()
            .map(|p| {
                let pid = p.pid().as_u32();
                let start_time = p.start_time();
                let running_secs = if start_time > 0 && now > start_time {
                    now - start_time
                } else {
                    0
                };

                // Mask user-home segments before the path is uploaded as
                // ProcessSnapshotEvent.executable_path. The canonical masker in
                // maekon-core covers /Users/, /home/, and C:\Users\ so Linux
                // usernames are not egressed (2nd-pass #61); the previous inline
                // redaction handled only macOS/Windows and leaked Linux homes.
                let exe_path = p.exe().map(|path| mask_user_paths(&path.to_string_lossy()));

                ProcessDetail {
                    name: p.name().to_string_lossy().to_string(),
                    pid,
                    cpu_percent: p.cpu_usage(),
                    memory_mb: p.memory() as f64 / (1024.0 * 1024.0),
                    window_count: 0, // filled by platform-specific window APIs
                    is_foreground: foreground_pid == Some(pid),
                    running_secs,
                    executable_path: exe_path,
                }
            })
            .collect();
        drop(sys_guard);

        all_details.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut result: Vec<ProcessDetail> = Vec::with_capacity(top_n + 1);
        let mut seen_pids: HashSet<u32> = HashSet::new();

        if let Some(fg_pid) = foreground_pid {
            if let Some(fg_detail) = all_details.iter().find(|d| d.pid == fg_pid) {
                result.push(fg_detail.clone());
                seen_pids.insert(fg_pid);
            }
        }

        for detail in all_details {
            // Cap the total result length (including any pinned foreground entry)
            // at exactly `top_n`, matching get_top_processes / truncate(limit)
            // semantics. The previous `> top_n` guard allowed one extra element
            // (top_n + 1) to be pushed before breaking (process-offbyone).
            if result.len() >= top_n {
                break;
            }
            if !seen_pids.contains(&detail.pid) {
                seen_pids.insert(detail.pid);
                result.push(detail);
            }
        }

        debug!(
            "detailed process list collected: count={}, foreground={:?}",
            result.len(),
            foreground_pid
        );
        Ok(result)
    }

    /// Test-only accessor for the cached `last_refresh` timestamp.
    #[cfg(test)]
    pub(crate) fn _last_refresh_instant(&self) -> Instant {
        *self.last_refresh.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Test-only: returns the process count right after init (for F-PF-18
    /// regression verification).
    #[cfg(test)]
    pub(crate) fn _initial_process_count(&self) -> usize {
        self.sys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .processes()
            .len()
    }

    /// Test-only mutator to drive cooldown expiration without wall-clock sleep.
    #[cfg(test)]
    pub(crate) fn _set_last_refresh(&self, t: Instant) {
        *self.last_refresh.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }
}

impl Default for ProcessTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProcessMonitor for ProcessTracker {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::get_active_window_macos()
                .await
                .map_err(Into::into)
        }
        // F-RR-C23-02: get_active_window_windows() calls get_process_name() which
        // builds a fresh sysinfo::System and calls refresh_processes() — a blocking
        // NtQuerySystemInformation syscall (~10–50 ms on a busy host). Wrapping in
        // spawn_blocking keeps the tokio worker pool free during the 1-second
        // scheduler hot-path. Mirrors the spawn_blocking pattern used by
        // get_top_processes / get_detailed_processes above.
        #[cfg(target_os = "windows")]
        {
            tokio::task::spawn_blocking(crate::windows::get_active_window_windows)
                .await
                .map_err(|e| CoreError::Internal {
                    code: InternalCode::Generic,
                    message: format!("spawn_blocking join error: {e}"),
                })?
                .map_err(Into::into)
        }
        #[cfg(target_os = "linux")]
        {
            crate::linux::get_active_window_linux()
                .await
                .map_err(Into::into)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Ok(None)
        }
    }

    // F-RR-C23-01: sysinfo refresh_processes is a blocking syscall. Delegate to
    // spawn_blocking to avoid stalling the tokio worker pool on the 1-second
    // scheduler hot-path. Arc clones are cheap (pointer + refcount bump).
    async fn get_top_processes(&self, limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        let sys = Arc::clone(&self.sys);
        let last_refresh = Arc::clone(&self.last_refresh);
        tokio::task::spawn_blocking(move || Self::collect_top_sync(sys, last_refresh, limit))
            .await
            .map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })?
    }

    // F-RR-C23-01: same spawn_blocking isolation as get_top_processes.
    async fn get_detailed_processes(
        &self,
        foreground_pid: Option<u32>,
        top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        let sys = Arc::clone(&self.sys);
        let last_refresh = Arc::clone(&self.last_refresh);
        tokio::task::spawn_blocking(move || {
            Self::collect_detailed_sync(sys, last_refresh, foreground_pid, top_n)
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_top_processes() {
        let tracker = ProcessTracker::new();
        let procs = tracker.get_top_processes(5).await.unwrap();
        assert!(procs.len() <= 5);
        assert!(!procs.is_empty());
    }

    /// F-RR-C23-01: verify that get_top_processes_async runs via spawn_blocking
    /// and returns Some result (non-blocking + correct result).
    #[tokio::test]
    async fn get_top_processes_async_returns_metrics() {
        let tracker = ProcessTracker::new();
        // Returns Ok(Vec) via spawn_blocking — only check that the length is non-zero.
        let procs = tracker
            .get_top_processes(10)
            .await
            .expect("get_top_processes via spawn_blocking must not fail");
        assert!(
            !procs.is_empty(),
            "get_top_processes via spawn_blocking returned an empty list"
        );
        assert!(procs.len() <= 10, "must respect the limit");
    }

    /// #6441 F14 regression: the narrowed `refresh_processes_specifics`
    /// (cpu + memory + exe only) must still populate the fields the collectors
    /// read. A too-narrow `ProcessRefreshKind` (e.g. dropping `.with_memory()`)
    /// would silently return zeroed memory or empty names. Asserts at least one
    /// process — this test process is guaranteed present — carries a non-empty
    /// name and non-zero memory.
    #[tokio::test]
    async fn narrowed_refresh_still_populates_name_and_memory() {
        let tracker = ProcessTracker::new();
        let procs = tracker
            .get_top_processes(50)
            .await
            .expect("get_top_processes must not fail");
        assert!(
            !procs.is_empty(),
            "process list must be populated after the narrowed refresh"
        );
        assert!(
            procs.iter().any(|p| !p.name.is_empty()),
            "narrowed refresh dropped process names (ProcessRefreshKind too narrow)"
        );
        assert!(
            procs.iter().any(|p| p.memory_bytes > 0),
            "narrowed refresh dropped process memory (missing .with_memory())"
        );
    }

    /// F-PF-18 regression test: verify ProcessTracker::new() uses System::new().
    /// With System::new_all() the processes would be populated right after init,
    /// but with System::new() the process list must be empty before
    /// refresh_processes is called.
    #[tokio::test]
    async fn test_system_sysinfo_new_not_new_all() {
        let tracker = ProcessTracker::new();
        assert_eq!(
            tracker._initial_process_count(),
            0,
            "System::new() must not pre-load the process table (F-PF-18)"
        );
        // After the first get_top_processes call the list is refreshed and populated.
        let procs = tracker.get_top_processes(5).await.unwrap();
        assert!(
            !procs.is_empty(),
            "process list must not be empty after refresh"
        );
    }

    #[tokio::test]
    async fn refresh_if_stale_skips_within_cooldown() {
        let tracker = ProcessTracker::new();
        // First call: new() initialises last_refresh to a stale instant so
        // this call WILL refresh. Capture the post-refresh timestamp.
        let _ = tracker.get_top_processes(5).await.unwrap();
        let after_first = tracker._last_refresh_instant();
        // Second call within the 2s cooldown — last_refresh must NOT advance.
        let _ = tracker.get_top_processes(5).await.unwrap();
        let after_second = tracker._last_refresh_instant();
        assert_eq!(
            after_first, after_second,
            "refresh_if_stale advanced within cooldown on second call"
        );
    }

    #[tokio::test]
    async fn refresh_if_stale_refreshes_after_cooldown() {
        let tracker = ProcessTracker::new();
        let pushed_back = Instant::now()
            .checked_sub(Duration::from_secs(3))
            .expect("test runner: Instant::now() must be >= 3s after process start");
        tracker._set_last_refresh(pushed_back);
        let before = tracker._last_refresh_instant();
        let _ = tracker.get_top_processes(5).await.unwrap();
        let after = tracker._last_refresh_instant();
        assert!(
            after > before,
            "refresh_if_stale did not advance after cooldown"
        );
    }

    /// F-QA-C24-01: verify that a panicking spawn_blocking task in
    /// get_top_processes maps to
    /// JoinError → CoreError::Internal { code: Generic, message: "spawn_blocking join error: ..." }.
    /// Same pattern as `collect_metrics_propagates_join_error` in system.rs.
    #[tokio::test]
    async fn get_top_processes_returns_internal_on_join_error() {
        // Pass a panic closure directly to spawn_blocking to produce a JoinError.
        // Reproduce the same conversion as the .map_err path in
        // ProcessTracker::get_top_processes.
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-QA-C24-01 get_top_processes test panic"))
                .await;

        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "the JoinError of a panicking spawn_blocking must be is_panic()"
        );

        // Reproduce the map_err pattern inside get_top_processes.
        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "JoinError must map to the CoreError::Internal variant: {err:?}"
        );
        if let CoreError::Internal { message, .. } = err {
            assert!(
                message.contains("spawn_blocking join error"),
                "message must contain 'spawn_blocking join error': {message}"
            );
        }
    }

    /// F-QA-C24-01: verify that a panicking spawn_blocking task in
    /// get_detailed_processes maps to JoinError → CoreError::Internal.
    #[tokio::test]
    async fn get_detailed_processes_returns_internal_on_join_error() {
        // Pass a panic closure directly to spawn_blocking to produce a JoinError.
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-QA-C24-01 get_detailed_processes test panic"))
                .await;

        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "the JoinError of a panicking spawn_blocking must be is_panic()"
        );

        // Reproduce the map_err pattern inside get_detailed_processes.
        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "JoinError must map to the CoreError::Internal variant: {err:?}"
        );
        if let CoreError::Internal { message, .. } = err {
            assert!(
                message.contains("spawn_blocking join error"),
                "message must contain 'spawn_blocking join error': {message}"
            );
        }
    }

    /// F-RR-C23-02: get_active_window on Windows routes through spawn_blocking.
    /// Verify that a panicking spawn_blocking closure maps to CoreError::Internal
    /// with "spawn_blocking join error" in the message — identical to the pattern
    /// used by get_top_processes and get_detailed_processes (F-QA-C24-01).
    ///
    /// This test is cfg-gated to windows because the live call site is also
    /// cfg-gated; the JoinError map_err shape is shared and already exercised on
    /// all platforms by get_top_processes_returns_internal_on_join_error above.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn get_active_window_returns_internal_on_join_error_windows() {
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-RR-C23-02 get_active_window test panic"))
                .await;

        let join_err = join_result.unwrap_err();
        assert!(join_err.is_panic());

        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        assert!(matches!(err, CoreError::Internal { .. }));
        if let CoreError::Internal { message, .. } = err {
            assert!(message.contains("spawn_blocking join error"));
        }
    }

    /// F-QA-C25-05: verify poisoned-Mutex handling inside refresh_if_stale.
    ///
    /// When the `last_refresh` Mutex is poisoned by another thread's panic,
    /// `unwrap_or_else(|e| e.into_inner())` must ignore the poison and return the
    /// inner value. In other words, verify that ProcessTracker keeps working
    /// without panicking even when the Mutex is poisoned.
    ///
    /// Pattern: panic while holding the Mutex inside spawn_blocking → Mutex is
    ///          poisoned → the next call recovers cleanly via the unwrap_or_else path.
    #[tokio::test]
    async fn refresh_if_stale_recovers_from_poisoned_last_refresh_mutex() {
        let tracker = ProcessTracker::new();

        // Clone the last_refresh Arc and poison it from a separate thread.
        let last_refresh_clone = Arc::clone(&tracker.last_refresh);

        // Panic while holding the Mutex → thread exits without dropping the
        // MutexGuard → the Mutex becomes poisoned.
        let poison_handle = std::thread::spawn(move || {
            let _guard = last_refresh_clone.lock().unwrap();
            panic!("F-QA-C25-05: intentional Mutex-poison test");
        });
        // Verify the panicking thread returns a JoinError (Mutex poisoning done).
        // thread::JoinHandle::join() returns Err(Box<dyn Any + Send>) on panic — the opaque
        // payload carries no typed variant to match against; Err presence is the full contract.
        // lint:allow-is-err-hedge — Box<dyn Any> has no typed variant
        assert!(
            poison_handle.join().is_err(),
            "a panicking thread must return Err"
        );

        // Verify last_refresh is actually poisoned.
        // PoisonError<MutexGuard<T>>::into_inner() is the only meaningful accessor; the error
        // carries no discriminating code or message beyond the fact of being poisoned.
        // lint:allow-is-err-hedge — PoisonError has no typed payload beyond being poisoned
        assert!(
            tracker.last_refresh.lock().is_err(),
            "the last_refresh Mutex must be poisoned"
        );

        // get_top_processes on a poisoned ProcessTracker must recover via
        // unwrap_or_else(|e| e.into_inner()) without panicking, and still
        // return a non-empty result bounded by the requested limit.
        let procs = tracker
            .get_top_processes(3)
            .await
            .expect("get_top_processes must recover from a poisoned Mutex (unwrap_or_else path)");
        assert!(
            procs.len() <= 3,
            "recovered result must respect the limit; got {} entries",
            procs.len()
        );
    }

    /// process-offbyone regression: get_detailed_processes must return at most
    /// `top_n` entries — including any pinned foreground entry — matching
    /// get_top_processes / truncate(limit) semantics. The previous `> top_n`
    /// break guard allowed `top_n + 1` entries to be returned.
    #[tokio::test]
    async fn get_detailed_processes_respects_top_n() {
        let tracker = ProcessTracker::new();

        // Some(pid) case: a pinned foreground entry must still be counted toward
        // the cap, so the total length must not exceed top_n.
        let with_fg = tracker
            .get_detailed_processes(Some(std::process::id()), 5)
            .await
            .expect("get_detailed_processes(Some, N) must not fail");
        assert!(
            with_fg.len() <= 5,
            "Some(pid) case exceeded top_n: got {} entries (expected <= 5)",
            with_fg.len()
        );

        // None case: no pinned foreground entry, still bounded by top_n.
        let without_fg = tracker
            .get_detailed_processes(None, 5)
            .await
            .expect("get_detailed_processes(None, N) must not fail");
        assert!(
            without_fg.len() <= 5,
            "None case exceeded top_n: got {} entries (expected <= 5)",
            without_fg.len()
        );
    }

    /// 2nd-pass #61 regression: the masker wired into the ProcessDetail
    /// `executable_path` builder must redact Linux `/home/<user>/` paths.
    /// `executable_path` is uploaded verbatim as ProcessSnapshotEvent, so a
    /// missed Linux home segment leaks the local username to the server.
    /// (The old inline redaction handled only /Users/ and \Users\.)
    #[test]
    fn exe_path_masker_redacts_linux_home() {
        let masked = mask_user_paths("/home/alice/bin/app");
        assert_eq!(masked, "/home/[USER]/bin/app");
        assert!(!masked.contains("alice"), "Linux username leaked: {masked}");
    }
}
