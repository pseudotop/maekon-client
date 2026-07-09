//! Criterion benchmarks for maekon-monitor.
//!
//! `SysInfoMonitor` is IO-bound (reads `/proc`, sysctl, etc.) but measuring
//! its latency is valuable for understanding scheduler loop budgets.

// Benchmark harness, not shipped code — freely uses unwrap/expect on setup
// invariants (#7719 workspace `unwrap_used`/`expect_used` policy).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{criterion_group, criterion_main, Criterion};
use maekon_core::ports::monitor::SystemMonitor;
use maekon_monitor::system::SysInfoMonitor;

fn bench_sysinfo_monitor_new(c: &mut Criterion) {
    c.bench_function("SysInfoMonitor::new()", |b| {
        b.iter(|| {
            let _monitor = SysInfoMonitor::new();
        })
    });
}

fn bench_collect_metrics(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let monitor = SysInfoMonitor::new();

    c.bench_function("SysInfoMonitor::collect_metrics()", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _ = monitor.collect_metrics().await;
            })
        })
    });
}

criterion_group!(benches, bench_sysinfo_monitor_new, bench_collect_metrics,);
criterion_main!(benches);
