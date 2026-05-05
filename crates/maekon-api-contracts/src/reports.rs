use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReportPeriod {
    #[default]
    Week,
    Month,
    Custom,
}

#[derive(Debug, Deserialize)]
pub struct ReportQuery {
    #[serde(default)]
    pub period: ReportPeriod,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct DailyStat {
    pub date: String,
    pub active_secs: u64,
    pub idle_secs: u64,
    pub captures: u64,
    pub events: u64,
    pub cpu_avg: f64,
    pub memory_avg: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct AppStat {
    pub name: String,
    pub duration_secs: u64,
    pub events: u64,
    pub captures: u64,
    pub percentage: f64,
}

#[derive(Debug, Serialize, Clone)]
pub struct HourlyActivity {
    pub hour: u8,
    pub activity: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct ProductivityMetrics {
    pub score: f64,
    pub active_ratio: f64,
    pub peak_hour: u8,
    pub top_app: String,
    pub trend: f64,
}

#[derive(Debug, Serialize)]
pub struct ReportResponse {
    pub title: String,
    pub from_date: String,
    pub to_date: String,
    pub days: u32,
    pub total_active_secs: u64,
    pub total_idle_secs: u64,
    pub total_captures: u64,
    pub total_events: u64,
    pub avg_cpu: f64,
    pub avg_memory: f64,
    pub daily_stats: Vec<DailyStat>,
    pub app_stats: Vec<AppStat>,
    pub hourly_activity: Vec<HourlyActivity>,
    pub productivity: ProductivityMetrics,
}
