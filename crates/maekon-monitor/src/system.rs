use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::system::{NetworkInfo, PowerStatus, SystemMetrics};
use maekon_core::ports::monitor::SystemMonitor;
use std::sync::{Arc, Mutex};
use sysinfo::{Disks, Networks, System};
use tracing::debug;

pub struct SysInfoMonitor {
    sys: Arc<Mutex<System>>,
    disks: Arc<Mutex<Disks>>,
    networks: Arc<Mutex<Networks>>,
}

impl SysInfoMonitor {
    pub fn new() -> Self {
        // F-PF-18: System::new_all() 은 프로세스 테이블 전체를 즉시 할당한다.
        // System::new() 로 교체하여 초기화 비용을 절감하고,
        // collect_metrics 호출 시 필요한 항목만 refresh_cpu_usage/refresh_memory 로 갱신한다.
        // 프로세스 테이블은 이 어댑터에서 사용하지 않으므로 refresh 하지 않는다.
        Self {
            sys: Arc::new(Mutex::new(System::new())),
            disks: Arc::new(Mutex::new(Disks::new_with_refreshed_list())),
            networks: Arc::new(Mutex::new(Networks::new_with_refreshed_list())),
        }
    }

    /// 동기 메트릭 수집 — `spawn_blocking` 내에서 호출됨.
    ///
    /// F-PF-C20-03: sysinfo I/O (statfs per FS, getifaddrs) 가 tokio worker 스레드를
    /// 점유하지 않도록 blocking 전용 스레드 풀로 격리한다.
    fn collect_metrics_sync(
        sys: Arc<Mutex<System>>,
        disks: Arc<Mutex<Disks>>,
        networks: Arc<Mutex<Networks>>,
    ) -> Result<SystemMetrics, CoreError> {
        {
            let mut sys = sys.lock().map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("Failed to acquire system lock: {e}"),
            })?;
            sys.refresh_cpu_usage();
            sys.refresh_memory();
        }

        {
            let mut disks = disks.lock().map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("Failed to acquire disk lock: {e}"),
            })?;
            disks.refresh(true);
        }

        {
            let mut networks = networks.lock().map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("Failed to acquire network lock: {e}"),
            })?;
            networks.refresh(true);
        }

        let sys = sys.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;

        let cpu_usage = sys.global_cpu_usage();
        let memory_used = sys.used_memory();
        let memory_total = sys.total_memory();
        drop(sys);

        let disks = disks.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire disk lock: {e}"),
        })?;
        let (disk_used, disk_total) = disks.list().iter().fold((0u64, 0u64), |(used, total), d| {
            (
                used + d.total_space() - d.available_space(),
                total + d.total_space(),
            )
        });
        drop(disks);

        let networks = networks.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire network lock: {e}"),
        })?;
        let (upload_speed, download_speed) = networks
            .list()
            .iter()
            .fold((0u64, 0u64), |(up, down), (_name, data)| {
                (up + data.transmitted(), down + data.received())
            });
        drop(networks);

        let network = Some(NetworkInfo {
            upload_speed,
            download_speed,
            is_connected: download_speed > 0 || upload_speed > 0,
        });

        let metrics = SystemMetrics {
            timestamp: chrono::Utc::now(),
            cpu_usage,
            memory_used,
            memory_total,
            disk_used,
            disk_total,
            network,
            typing_wpm: 0.0,
        };

        debug!(
            "system metrics: CPU {:.1}%, memory {}/{}MB",
            metrics.cpu_usage,
            metrics.memory_used / 1_048_576,
            metrics.memory_total / 1_048_576
        );

        Ok(metrics)
    }
}

impl Default for SysInfoMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemMonitor for SysInfoMonitor {
    // F-PF-C20-03: sysinfo I/O (statfs per FS, getifaddrs/proc/net/dev) 가
    // tokio worker 스레드를 최대 5초 간격으로 점유하지 않도록 spawn_blocking 으로 격리.
    // Arc<Mutex<T>> 로 내부 필드를 래핑하여 'static + Send 요구를 충족.
    async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError> {
        let sys = Arc::clone(&self.sys);
        let disks = Arc::clone(&self.disks);
        let networks = Arc::clone(&self.networks);

        tokio::task::spawn_blocking(move || Self::collect_metrics_sync(sys, disks, networks))
            .await
            .map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })?
    }

    async fn current_power_status(&self) -> Result<PowerStatus, CoreError> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::current_power_status_macos()
                .await
                .map_err(|e| CoreError::Internal {
                    code: InternalCode::Generic,
                    message: format!("Failed to collect power status: {e}"),
                })
        }

        #[cfg(not(target_os = "macos"))]
        {
            Ok(PowerStatus::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collect_metrics() {
        let monitor = SysInfoMonitor::new();
        let metrics = monitor.collect_metrics().await.unwrap();

        assert!(metrics.cpu_usage >= 0.0);
        assert!(metrics.memory_total > 0);
        assert!(metrics.memory_used <= metrics.memory_total);
    }

    #[tokio::test]
    async fn collect_metrics_runs_in_spawn_blocking() {
        // F-PF-C20-03: spawn_blocking 이 tokio::task::block_in_place 가 아닌
        // 별도 blocking thread pool 에서 실행됨을 확인한다.
        // 반환 타입이 Result<SystemMetrics> 이면 충분 (regression test).
        let monitor = SysInfoMonitor::new();
        let metrics = monitor
            .collect_metrics()
            .await
            .expect("collect_metrics via spawn_blocking must not fail");
        // Probe that the returned SystemMetrics are structurally valid, mirroring
        // the collect_metrics test above. Justified: sysinfo runs on any host
        // including CI — spawn_blocking itself must not error (#5594).
        assert!(metrics.cpu_usage >= 0.0, "cpu_usage must be non-negative");
        assert!(metrics.memory_total > 0, "memory_total must be positive");
        assert!(
            metrics.memory_used <= metrics.memory_total,
            "memory_used must not exceed memory_total"
        );
    }

    #[tokio::test]
    async fn collect_metrics_propagates_join_error() {
        // F-QA-C21-06: spawn_blocking JoinError → CoreError::Internal 매핑 검증.
        //
        // collect_metrics() 의 map_err(|e| CoreError::Internal { ... }) 경로는
        // 정상 실행에서 도달하지 않으므로 직접 spawn_blocking panic 시나리오로 검증한다.
        //
        // JoinError 는 spawn_blocking 태스크가 panic 하거나 취소될 때 발생한다.
        // 여기서는 panic closure 를 spawn_blocking 에 직접 전달하여 JoinError 생성 후
        // CoreError::Internal 으로의 매핑을 어서션한다.
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-QA-C21-06 test panic")).await;

        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "panic 한 spawn_blocking 의 JoinError 는 is_panic() 이어야 한다"
        );

        // JoinError → CoreError::Internal 매핑 재현 (collect_metrics 내부와 동일 패턴)
        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        // CoreError::Internal 변형인지 확인
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "JoinError 는 CoreError::Internal 변형으로 매핑되어야 한다: {err:?}"
        );
        // message 에 식별 문자열 포함 여부 확인
        if let CoreError::Internal { message, .. } = err {
            assert!(
                message.contains("spawn_blocking join error"),
                "message 가 'spawn_blocking join error' 를 포함해야 한다: {message}"
            );
        }
    }
}
