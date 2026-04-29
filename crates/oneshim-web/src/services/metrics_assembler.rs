use oneshim_api_contracts::metrics::{HourlyMetricsResponse, MetricsResponse};
use oneshim_core::models::storage_records::HourlyMetricsRecord;
use oneshim_core::models::system::SystemMetrics;

pub(crate) fn memory_percent(metric: &SystemMetrics) -> f64 {
    if metric.memory_total > 0 {
        (metric.memory_used as f64 / metric.memory_total as f64) * 100.0
    } else {
        0.0
    }
}

pub(crate) fn assemble_metrics_response(metric: SystemMetrics) -> MetricsResponse {
    let network_upload = metric
        .network
        .as_ref()
        .map(|network| network.upload_speed)
        .unwrap_or(0);
    let network_download = metric
        .network
        .as_ref()
        .map(|network| network.download_speed)
        .unwrap_or(0);

    MetricsResponse {
        timestamp: metric.timestamp.to_rfc3339(),
        cpu_usage: metric.cpu_usage as f64,
        memory_used: metric.memory_used,
        memory_total: metric.memory_total,
        memory_percent: memory_percent(&metric),
        disk_used: metric.disk_used,
        disk_total: metric.disk_total,
        network_upload,
        network_download,
    }
}

pub(crate) fn assemble_hourly_metrics_response(row: HourlyMetricsRecord) -> HourlyMetricsResponse {
    HourlyMetricsResponse {
        hour: row.hour,
        cpu_avg: row.cpu_avg,
        cpu_max: row.cpu_max,
        memory_avg: row.memory_avg,
        memory_max: row.memory_max,
        sample_count: row.sample_count,
    }
}
