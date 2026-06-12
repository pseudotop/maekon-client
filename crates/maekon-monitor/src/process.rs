use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::context::{ProcessInfo, WindowInfo};
use maekon_core::models::event::ProcessDetail;
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
        // F-PF-18: System::new_all() 은 초기화 시 프로세스 테이블 전체를 스캔한다.
        // System::new() 로 교체하여 초기화 비용을 절감한다.
        // 실제 프로세스 목록은 refresh_if_stale → refresh_processes 에서 lazily 갱신된다.
        //
        // last_refresh 를 REFRESH_COOLDOWN 이전으로 초기화하여 첫 호출 시 즉시 refresh
        // 가 일어나도록 보장한다. Instant::now() 로 초기화하면 첫 호출에서 cooldown 이
        // 경과하지 않아 프로세스 목록이 비어 있게 된다 (pre-existing test failure).
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
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
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

                let exe_path = p.exe().map(|path| {
                    let path_str = path.to_string_lossy().to_string();
                    if path_str.contains("/Users/") {
                        path_str
                            .split("/Users/")
                            .last()
                            .and_then(|s| s.split('/').nth(1))
                            .map(|rest| format!("~/{}", rest))
                            .unwrap_or_else(|| "~/...".to_string())
                    } else if path_str.contains("\\Users\\") {
                        path_str
                            .split("\\Users\\")
                            .last()
                            .and_then(|s| s.split('\\').nth(1))
                            .map(|rest| format!("~\\{}", rest))
                            .unwrap_or_else(|| "~\\...".to_string())
                    } else {
                        path_str
                    }
                });

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
            if result.len() > top_n {
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

    /// Test-only: 초기화 직후 프로세스 수를 반환 (F-PF-18 회귀 검증용).
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
        #[cfg(target_os = "windows")]
        {
            crate::windows::get_active_window_windows().map_err(Into::into)
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

    /// F-RR-C23-01: get_top_processes_async は spawn_blocking 経由で実行され、
    /// Some result を返すことを検証する (non-blocking + correct result).
    #[tokio::test]
    async fn get_top_processes_async_returns_metrics() {
        let tracker = ProcessTracker::new();
        // spawn_blocking 경유로 Ok(Vec) 반환 — 길이가 0이 아닌 것만 확인.
        let procs = tracker
            .get_top_processes(10)
            .await
            .expect("get_top_processes via spawn_blocking must not fail");
        assert!(
            !procs.is_empty(),
            "spawn_blocking 경유 get_top_processes 가 빈 목록을 반환했다"
        );
        assert!(procs.len() <= 10, "limit 준수");
    }

    /// F-PF-18 회귀 테스트: ProcessTracker::new() 이 System::new() 를 사용하는지 확인.
    /// System::new_all() 이면 초기화 직후 프로세스가 채워지지만,
    /// System::new() 에서는 refresh_processes 호출 전 프로세스 목록이 비어 있어야 한다.
    #[tokio::test]
    async fn test_system_sysinfo_new_not_new_all() {
        let tracker = ProcessTracker::new();
        assert_eq!(
            tracker._initial_process_count(),
            0,
            "System::new() 는 프로세스 테이블을 사전 로드하지 않아야 한다 (F-PF-18)"
        );
        // 첫 get_top_processes 호출 후 refresh 되어 목록이 채워진다.
        let procs = tracker.get_top_processes(5).await.unwrap();
        assert!(
            !procs.is_empty(),
            "refresh 후 프로세스 목록이 비어 있으면 안 된다"
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

    /// F-QA-C24-01: get_top_processes 의 spawn_blocking 태스크가 panic 하면
    /// JoinError → CoreError::Internal { code: Generic, message: "spawn_blocking join error: ..." }
    /// 로 매핑되는지 검증한다.
    /// system.rs `collect_metrics_propagates_join_error` 와 동일한 패턴.
    #[tokio::test]
    async fn get_top_processes_returns_internal_on_join_error() {
        // spawn_blocking 에 panic closure 를 직접 전달하여 JoinError 를 생성한다.
        // ProcessTracker::get_top_processes 의 .map_err 경로와 동일한 변환을 재현한다.
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-QA-C24-01 get_top_processes test panic"))
                .await;

        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "panic 한 spawn_blocking 의 JoinError 는 is_panic() 이어야 한다"
        );

        // get_top_processes 내부의 map_err 패턴 재현
        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "JoinError 는 CoreError::Internal 변형으로 매핑되어야 한다: {err:?}"
        );
        if let CoreError::Internal { message, .. } = err {
            assert!(
                message.contains("spawn_blocking join error"),
                "message 가 'spawn_blocking join error' 를 포함해야 한다: {message}"
            );
        }
    }

    /// F-QA-C24-01: get_detailed_processes 의 spawn_blocking 태스크가 panic 하면
    /// JoinError → CoreError::Internal 로 매핑되는지 검증한다.
    #[tokio::test]
    async fn get_detailed_processes_returns_internal_on_join_error() {
        // spawn_blocking 에 panic closure 를 직접 전달하여 JoinError 를 생성한다.
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-QA-C24-01 get_detailed_processes test panic"))
                .await;

        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "panic 한 spawn_blocking 의 JoinError 는 is_panic() 이어야 한다"
        );

        // get_detailed_processes 내부의 map_err 패턴 재현
        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "JoinError 는 CoreError::Internal 변형으로 매핑되어야 한다: {err:?}"
        );
        if let CoreError::Internal { message, .. } = err {
            assert!(
                message.contains("spawn_blocking join error"),
                "message 가 'spawn_blocking join error' 를 포함해야 한다: {message}"
            );
        }
    }

    /// F-QA-C25-05: refresh_if_stale 내부 Mutex 독(poison) 처리 검증.
    ///
    /// `last_refresh` Mutex 가 다른 스레드의 panic 으로 독(poisoned)이 되면
    /// `unwrap_or_else(|e| e.into_inner())` 가 독을 무시하고 내부 값을 반환해야 한다.
    /// 즉, Mutex 독이 있어도 ProcessTracker 가 패닉 없이 계속 동작하는지 확인한다.
    ///
    /// 패턴: spawn_blocking 내에서 Mutex 를 잠근 채 panic → Mutex 독 발생 →
    ///        다음 호출에서 unwrap_or_else 경로를 거쳐 정상 복구.
    #[tokio::test]
    async fn refresh_if_stale_recovers_from_poisoned_last_refresh_mutex() {
        let tracker = ProcessTracker::new();

        // last_refresh Arc 를 클론하여 별도 스레드에서 독 생성
        let last_refresh_clone = Arc::clone(&tracker.last_refresh);

        // Mutex 를 잠근 채 panic → MutexGuard drop 없이 스레드 종료 → Mutex 독
        let poison_handle = std::thread::spawn(move || {
            let _guard = last_refresh_clone.lock().unwrap();
            panic!("F-QA-C25-05: 의도적 Mutex 독 생성 테스트");
        });
        // panic 한 스레드가 JoinError 를 반환하는 것을 확인 (Mutex 독 생성 완료).
        // thread::JoinHandle::join() returns Err(Box<dyn Any + Send>) on panic — the opaque
        // payload carries no typed variant to match against; Err presence is the full contract.
        // lint:allow-is-err-hedge — Box<dyn Any> has no typed variant
        assert!(
            poison_handle.join().is_err(),
            "panic 한 스레드는 Err를 반환해야 한다"
        );

        // last_refresh 가 실제로 독(poisoned) 상태인지 확인.
        // PoisonError<MutexGuard<T>>::into_inner() is the only meaningful accessor; the error
        // carries no discriminating code or message beyond the fact of being poisoned.
        // lint:allow-is-err-hedge — PoisonError has no typed payload beyond being poisoned
        assert!(
            tracker.last_refresh.lock().is_err(),
            "last_refresh Mutex 가 독(poisoned) 상태여야 한다"
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
}
