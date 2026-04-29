use chrono::{Duration, NaiveDate, Utc};
use oneshim_api_contracts::stats::{AppUsageEntry, DateQuery};
use oneshim_core::models::event::Event;
use oneshim_core::models::system::SystemMetrics;
use std::collections::HashMap;

use crate::error::ApiError;
use crate::services::stats_assembler::assemble_app_usage_entry;
use crate::services::web_contexts::StorageWebContext;

pub(crate) fn resolve_day_range(
    params: &DateQuery,
) -> Result<(String, chrono::DateTime<Utc>, chrono::DateTime<Utc>), ApiError> {
    let date = params
        .date
        .clone()
        .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string());
    let from = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .map_err(|_| ApiError::BadRequest(format!("Invalid date format: {date}")))?
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::Internal("failed to construct midnight time".into()))?
        .and_utc();
    Ok((date, from, from + Duration::days(1)))
}

pub(crate) fn average_metric_usage(metrics: &[SystemMetrics]) -> (f64, f64) {
    let (cpu_sum, memory_sum, count) = metrics.iter().fold(
        (0.0f64, 0.0f64, 0u64),
        |(cpu_acc, memory_acc, count), metric| {
            let memory_percent = if metric.memory_total > 0 {
                (metric.memory_used as f64 / metric.memory_total as f64) * 100.0
            } else {
                0.0
            };
            (
                cpu_acc + metric.cpu_usage as f64,
                memory_acc + memory_percent,
                count + 1,
            )
        },
    );

    if count == 0 {
        (0.0, 0.0)
    } else {
        (cpu_sum / count as f64, memory_sum / count as f64)
    }
}

pub(crate) fn build_activity_counts(
    events: &[oneshim_core::models::event::Event],
    frames: &[oneshim_core::models::storage_records::FrameRecord],
) -> HashMap<String, (u64, u64)> {
    let mut app_stats: HashMap<String, (u64, u64)> = HashMap::new();

    for event in events {
        if let Some(app_name) = match event {
            Event::User(value) => Some(value.app_name.clone()),
            Event::Context(value) => Some(value.app_name.clone()),
            _ => None,
        } {
            let entry = app_stats.entry(app_name).or_insert((0, 0));
            entry.0 += 1;
        }
    }

    for frame in frames {
        let entry = app_stats.entry(frame.app_name.clone()).or_insert((0, 0));
        entry.1 += 1;
    }

    app_stats
}

pub(crate) fn app_durations_for_range(
    ctx: &StorageWebContext,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
) -> HashMap<String, i64> {
    let from_rfc = from.to_rfc3339();
    let to_rfc = to.to_rfc3339();
    match ctx.storage.get_app_durations_by_date(&from_rfc, &to_rfc) {
        Ok(durations) => durations.into_iter().collect(),
        Err(_) => HashMap::new(),
    }
}

pub(crate) fn build_app_usage_entries(
    app_stats: &mut HashMap<String, (u64, u64)>,
    session_app_durations: &HashMap<String, i64>,
) -> Vec<AppUsageEntry> {
    app_stats
        .drain()
        .map(|(name, (event_count, frame_count))| {
            let duration_secs = session_app_durations
                .get(&name)
                .map(|&duration| duration as u64)
                .unwrap_or(event_count * 5);
            assemble_app_usage_entry(name, duration_secs, event_count, frame_count)
        })
        .collect()
}

pub(crate) fn total_active_secs_for_range(
    ctx: &StorageWebContext,
    from: chrono::DateTime<Utc>,
    to: chrono::DateTime<Utc>,
    fallback_events_logged: u64,
) -> u64 {
    let Ok(window) = oneshim_core::types::TimeWindow::new(from, to) else {
        return fallback_events_logged * 5;
    };
    match ctx.storage.get_daily_active_secs(&window) {
        Ok(daily) if !daily.is_empty() => daily.iter().map(|(_, seconds)| *seconds as u64).sum(),
        _ => fallback_events_logged * 5,
    }
}

pub(crate) fn increment_heatmap_cell(grid: &mut [[u32; 24]; 7], day: u32, hour: u32) {
    let day = day as usize;
    let hour = hour as usize;
    if day < 7 && hour < 24 {
        grid[day][hour] += 1;
    }
}
