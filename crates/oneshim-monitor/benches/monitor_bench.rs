//! Criterion benchmarks for oneshim-monitor.
//!
//! `SysInfoMonitor` is IO-bound (reads `/proc`, sysctl, etc.) but measuring
//! its latency is valuable for understanding scheduler loop budgets.

use criterion::{criterion_group, criterion_main, Criterion};
use oneshim_core::ports::monitor::SystemMonitor;
use oneshim_monitor::system::SysInfoMonitor;

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
