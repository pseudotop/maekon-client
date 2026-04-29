use chrono::Timelike;
use oneshim_api_contracts::reports::{
    AppStat, DailyStat, HourlyActivity, ProductivityMetrics, ReportResponse,
};
use oneshim_core::models::storage_records::FrameRecord;
use oneshim_core::models::system::SystemMetrics;

pub(crate) struct ReportResponseInput {
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

pub(crate) fn assemble_report_response(input: ReportResponseInput) -> ReportResponse {
    ReportResponse {
        title: input.title,
        from_date: input.from_date,
        to_date: input.to_date,
        days: input.days,
        total_active_secs: input.total_active_secs,
        total_idle_secs: input.total_idle_secs,
        total_captures: input.total_captures,
        total_events: input.total_events,
        avg_cpu: input.avg_cpu,
        avg_memory: input.avg_memory,
        daily_stats: input.daily_stats,
        app_stats: input.app_stats,
        hourly_activity: input.hourly_activity,
        productivity: input.productivity,
    }
}

pub(crate) fn assemble_daily_stat(date: String) -> DailyStat {
    DailyStat {
        date,
        active_secs: 0,
        idle_secs: 0,
        captures: 0,
        events: 0,
        cpu_avg: 0.0,
        memory_avg: 0.0,
    }
}

pub(crate) fn assemble_hourly_activity(hour: usize, activity: u64) -> HourlyActivity {
    HourlyActivity {
        hour: hour as u8,
        activity,
    }
}

pub(crate) fn assemble_productivity_metrics(
    score: f64,
    active_ratio: f64,
    peak_hour: u8,
    top_app: String,
    trend: f64,
) -> ProductivityMetrics {
    ProductivityMetrics {
        score,
        active_ratio,
        peak_hour,
        top_app,
        trend,
    }
}

pub(crate) fn memory_pct(metric: &SystemMetrics) -> f64 {
    if metric.memory_total > 0 {
        (metric.memory_used as f64 / metric.memory_total as f64) * 100.0
    } else {
        0.0
    }
}

pub(crate) fn frame_hour(frame: &FrameRecord) -> Option<usize> {
    chrono::DateTime::parse_from_rfc3339(&frame.timestamp)
        .ok()
        .map(|timestamp| timestamp.hour() as usize)
}
