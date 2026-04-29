//! Deterministic `SystemMonitor` mock for D13-v2b streaming tests.
//!
//! `parking_lot::Mutex<SystemMetrics>`-backed (spec IMP-V2-23 — std has no
//! `AtomicF32`). Tests flip CPU/memory via `set_cpu` / `set_mem` between
//! polls to drive `LoadPolicy` classification branches deterministically.

use std::sync::Arc;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::system::SystemMetrics;
use oneshim_core::ports::monitor::SystemMonitor;
use parking_lot::Mutex;

pub struct MockSystemMonitor {
    metrics: Mutex<SystemMetrics>,
}

impl MockSystemMonitor {
    pub fn new(cpu_pct: f32, used_mb: u32, total_mb: u32) -> Arc<Self> {
        Arc::new(Self {
            metrics: Mutex::new(SystemMetrics {
                timestamp: chrono::Utc::now(),
                cpu_usage: cpu_pct,
                memory_used: u64::from(used_mb) * 1_048_576,
                memory_total: u64::from(total_mb) * 1_048_576,
                disk_used: 0,
                disk_total: 0,
                network: None,
                typing_wpm: 0.0,
            }),
        })
    }

    pub fn set_cpu(&self, pct: f32) {
        self.metrics.lock().cpu_usage = pct;
    }

    pub fn set_mem(&self, used_mb: u32) {
        self.metrics.lock().memory_used = u64::from(used_mb) * 1_048_576;
    }
}

#[async_trait]
impl SystemMonitor for MockSystemMonitor {
    async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError> {
        let mut m = self.metrics.lock().clone();
        m.timestamp = chrono::Utc::now();
        Ok(m)
    }
}
