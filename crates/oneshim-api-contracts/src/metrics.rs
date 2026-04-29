use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct MetricsResponse {
    pub timestamp: String,
    pub cpu_usage: f64,
    pub memory_used: u64,
    pub memory_total: u64,
    pub memory_percent: f64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub network_upload: u64,
    pub network_download: u64,
}

#[derive(Debug, Deserialize)]
pub struct HourlyQuery {
    pub hours: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct HourlyMetricsResponse {
    pub hour: String,
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub memory_avg: u64,
    pub memory_max: u64,
    pub sample_count: u64,
}
