use oneshim_api_contracts::stats::{
    AppUsageEntry, AppUsageResponse, DailySummaryResponse, HeatmapCell, HeatmapResponse,
};

pub(crate) fn assemble_app_usage_entry(
    name: String,
    duration_secs: u64,
    event_count: u64,
    frame_count: u64,
) -> AppUsageEntry {
    AppUsageEntry {
        name,
        duration_secs,
        event_count,
        frame_count,
    }
}

pub(crate) struct DailySummaryInput {
    pub date: String,
    pub total_active_secs: u64,
    pub total_idle_secs: u64,
    pub top_apps: Vec<AppUsageEntry>,
    pub cpu_avg: f64,
    pub memory_avg_percent: f64,
    pub frames_captured: u64,
    pub events_logged: u64,
}

pub(crate) fn assemble_daily_summary(input: DailySummaryInput) -> DailySummaryResponse {
    DailySummaryResponse {
        date: input.date,
        total_active_secs: input.total_active_secs,
        total_idle_secs: input.total_idle_secs,
        top_apps: input.top_apps,
        cpu_avg: input.cpu_avg,
        memory_avg_percent: input.memory_avg_percent,
        frames_captured: input.frames_captured,
        events_logged: input.events_logged,
    }
}

pub(crate) fn assemble_app_usage_response(
    date: String,
    apps: Vec<AppUsageEntry>,
) -> AppUsageResponse {
    AppUsageResponse { date, apps }
}

pub(crate) fn assemble_heatmap_cell(day: u8, hour: u8, value: u32) -> HeatmapCell {
    HeatmapCell { day, hour, value }
}

pub(crate) fn assemble_heatmap_response(
    from_date: String,
    to_date: String,
    cells: Vec<HeatmapCell>,
    max_value: u32,
) -> HeatmapResponse {
    HeatmapResponse {
        from_date,
        to_date,
        cells,
        max_value,
    }
}
