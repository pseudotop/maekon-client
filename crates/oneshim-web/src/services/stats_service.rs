use chrono::{Datelike, Duration, Timelike, Utc};
use oneshim_api_contracts::stats::{
    AppUsageResponse, DailySummaryResponse, DateQuery, HeatmapResponse,
};
use oneshim_core::models::event::Event;

use crate::error::ApiError;
use crate::services::stats_assembler::{
    assemble_app_usage_response, assemble_daily_summary, assemble_heatmap_cell,
    assemble_heatmap_response, DailySummaryInput,
};
use crate::services::stats_query_support::{
    app_durations_for_range, average_metric_usage, build_activity_counts, build_app_usage_entries,
    increment_heatmap_cell, resolve_day_range, total_active_secs_for_range,
};
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct StatsQueryService {
    ctx: StorageWebContext,
}

impl StatsQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_summary(&self, params: &DateQuery) -> Result<DailySummaryResponse, ApiError> {
        let (date, from, to) = resolve_day_range(params)?;
        let metrics = self.ctx.storage.get_metrics(from, to, 10000).await?;

        let (cpu_avg, memory_avg_percent) = average_metric_usage(&metrics);

        let idle_periods = self.ctx.storage.get_idle_periods(from, to).await?;
        let total_idle_secs: u64 = idle_periods
            .iter()
            .filter_map(|period| period.duration_secs)
            .sum();

        let events = self.ctx.storage.get_events(from, to, 100000).await?;
        let frames = self.ctx.storage.get_frames(from, to, 100000)?;
        let events_logged = events.len() as u64;
        let frames_captured = frames.len() as u64;

        let mut app_stats = build_activity_counts(&events, &frames);
        let session_app_durations = app_durations_for_range(&self.ctx, from, to);
        let mut top_apps = build_app_usage_entries(&mut app_stats, &session_app_durations);
        top_apps.sort_by_key(|a| std::cmp::Reverse(a.duration_secs));
        top_apps.truncate(10);

        let total_active_secs = total_active_secs_for_range(&self.ctx, from, to, events_logged);

        Ok(assemble_daily_summary(DailySummaryInput {
            date,
            total_active_secs,
            total_idle_secs,
            top_apps,
            cpu_avg,
            memory_avg_percent,
            frames_captured,
            events_logged,
        }))
    }

    pub async fn get_app_usage(&self, params: &DateQuery) -> Result<AppUsageResponse, ApiError> {
        let (date, from, to) = resolve_day_range(params)?;
        let events = self.ctx.storage.get_events(from, to, 100000).await?;
        let frames = self.ctx.storage.get_frames(from, to, 100000)?;

        let mut app_stats = build_activity_counts(&events, &frames);
        let session_app_durations = app_durations_for_range(&self.ctx, from, to);
        let mut apps = build_app_usage_entries(&mut app_stats, &session_app_durations);
        apps.sort_by_key(|a| std::cmp::Reverse(a.duration_secs));

        Ok(assemble_app_usage_response(date, apps))
    }

    pub async fn get_heatmap(&self, days: Option<u32>) -> Result<HeatmapResponse, ApiError> {
        let days = days.unwrap_or(7).min(30) as i64;
        let to = Utc::now();
        let from = to - Duration::days(days);

        let events = self.ctx.storage.get_events(from, to, 100000).await?;
        let frames = self.ctx.storage.get_frames(from, to, 100000)?;

        let mut grid: [[u32; 24]; 7] = [[0; 24]; 7];

        for event in &events {
            let timestamp = match event {
                Event::User(value) => value.timestamp,
                Event::Context(value) => value.timestamp,
                Event::System(value) => value.timestamp,
                Event::Input(value) => value.timestamp,
                Event::Process(value) => value.timestamp,
                Event::Window(value) => value.timestamp,
                Event::Clipboard(value) => value.timestamp,
                Event::FileAccess(value) => value.timestamp,
            };
            increment_heatmap_cell(
                &mut grid,
                timestamp.weekday().num_days_from_monday(),
                timestamp.hour(),
            );
        }

        for frame in &frames {
            if let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(&frame.timestamp) {
                let timestamp = timestamp.with_timezone(&Utc);
                increment_heatmap_cell(
                    &mut grid,
                    timestamp.weekday().num_days_from_monday(),
                    timestamp.hour(),
                );
            }
        }

        let mut cells = Vec::with_capacity(7 * 24);
        let mut max_value = 0u32;
        for (day, hours) in grid.iter().enumerate() {
            for (hour, &value) in hours.iter().enumerate() {
                cells.push(assemble_heatmap_cell(day as u8, hour as u8, value));
                max_value = max_value.max(value);
            }
        }

        Ok(assemble_heatmap_response(
            from.format("%Y-%m-%d").to_string(),
            to.format("%Y-%m-%d").to_string(),
            cells,
            max_value,
        ))
    }
}
