use std::collections::HashMap;

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use oneshim_api_contracts::reports::{AppStat, DailyStat, ReportPeriod, ReportQuery};
use oneshim_core::models::event::Event;
use oneshim_core::types::TimeWindow;

use crate::error::ApiError;
use crate::services::reports_assembler::{
    assemble_daily_stat, assemble_hourly_activity, assemble_productivity_metrics, frame_hour,
    memory_pct,
};
use crate::services::web_contexts::StorageWebContext;

pub(crate) fn resolve_report_window(
    params: &ReportQuery,
    now: DateTime<Utc>,
) -> Result<(TimeWindow, String), ApiError> {
    match params.period {
        ReportPeriod::Week => {
            let to = now;
            let from = to - Duration::days(7);
            let window = TimeWindow::new(from, to).expect("now - 7d <= now");
            Ok((window, "주간 Activity Report".to_string()))
        }
        ReportPeriod::Month => {
            let to = now;
            let from = to - Duration::days(30);
            let window = TimeWindow::new(from, to).expect("now - 30d <= now");
            Ok((window, "월간 Activity Report".to_string()))
        }
        ReportPeriod::Custom => {
            // ReportQuery is date-only (%Y-%m-%d) per spec — NaiveDate parsing
            // preserved (NOT RFC3339). TimeWindow construction follows once both
            // dates are converted to UTC datetimes.
            let from_str = params
                .from
                .as_ref()
                .ok_or_else(|| ApiError::BadRequest("from date is required".to_string()))?;
            let to_str = params
                .to
                .as_ref()
                .ok_or_else(|| ApiError::BadRequest("to date is required".to_string()))?;

            let from_date = NaiveDate::parse_from_str(from_str, "%Y-%m-%d")
                .map_err(|_| ApiError::BadRequest(format!("Invalid from date: {from_str}")))?;
            let to_date = NaiveDate::parse_from_str(to_str, "%Y-%m-%d")
                .map_err(|_| ApiError::BadRequest(format!("Invalid to date: {to_str}")))?;

            let from = from_date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| ApiError::Internal("Time conversion failed: 00:00:00".to_string()))?
                .and_utc();
            let to = to_date
                .and_hms_opt(23, 59, 59)
                .ok_or_else(|| ApiError::Internal("Time conversion failed: 23:59:59".to_string()))?
                .and_utc();

            let window =
                TimeWindow::new(from, to).map_err(|e| ApiError::BadRequest(e.to_string()))?;
            Ok((window, format!("Activity Report ({from_str} ~ {to_str})")))
        }
    }
}

pub(crate) struct DailyStatsInput<'a> {
    pub(crate) ctx: &'a StorageWebContext,
    pub(crate) from: DateTime<Utc>,
    pub(crate) to: DateTime<Utc>,
    pub(crate) metrics: &'a [oneshim_core::models::system::SystemMetrics],
    pub(crate) events: &'a [Event],
    pub(crate) frames: &'a [oneshim_core::models::storage_records::FrameRecord],
    pub(crate) idle_periods: &'a [oneshim_core::models::activity::IdlePeriod],
}

pub(crate) fn build_daily_stats(input: DailyStatsInput<'_>) -> Vec<DailyStat> {
    let mut daily_map = HashMap::new();
    let mut current = input.from;
    while current < input.to {
        let date = current.format("%Y-%m-%d").to_string();
        daily_map.insert(date.clone(), assemble_daily_stat(date));
        current += Duration::days(1);
    }

    for event in input.events {
        let date = event_timestamp(event).format("%Y-%m-%d").to_string();
        if let Some(stat) = daily_map.get_mut(&date) {
            stat.events += 1;
        }
    }

    if let Ok(window) = oneshim_core::types::TimeWindow::new(input.from, input.to) {
        if let Ok(daily_active) = input.ctx.storage.get_daily_active_secs(&window) {
            for (day, secs) in &daily_active {
                if let Some(stat) = daily_map.get_mut(day) {
                    stat.active_secs = *secs as u64;
                }
            }
        }
    }

    for stat in daily_map.values_mut() {
        if stat.active_secs == 0 && stat.events > 0 {
            stat.active_secs = stat.events * 5;
        }
    }

    for frame in input.frames {
        if let Ok(timestamp) = DateTime::parse_from_rfc3339(&frame.timestamp) {
            let date = timestamp.format("%Y-%m-%d").to_string();
            if let Some(stat) = daily_map.get_mut(&date) {
                stat.captures += 1;
            }
        }
    }

    for idle_period in input.idle_periods {
        let date = idle_period.start_time.format("%Y-%m-%d").to_string();
        if let Some(stat) = daily_map.get_mut(&date) {
            stat.idle_secs += idle_period.duration_secs.unwrap_or(0);
        }
    }

    let mut daily_metrics: HashMap<String, (f64, f64, u64)> = HashMap::new();
    for metric in input.metrics {
        let date = metric.timestamp.format("%Y-%m-%d").to_string();
        let entry = daily_metrics.entry(date).or_insert((0.0, 0.0, 0));
        entry.0 += metric.cpu_usage as f64;
        entry.1 += memory_pct(metric);
        entry.2 += 1;
    }

    for (date, (cpu, mem, count)) in daily_metrics {
        if let Some(stat) = daily_map.get_mut(&date) {
            if count > 0 {
                stat.cpu_avg = cpu / count as f64;
                stat.memory_avg = mem / count as f64;
            }
        }
    }

    let mut daily_stats: Vec<DailyStat> = daily_map.into_values().collect();
    if daily_stats.is_empty() {
        daily_stats.push(assemble_daily_stat(
            input.from.format("%Y-%m-%d").to_string(),
        ));
    }
    daily_stats
}

pub(crate) fn build_app_stats(
    ctx: &StorageWebContext,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    events: &[Event],
    frames: &[oneshim_core::models::storage_records::FrameRecord],
) -> Vec<AppStat> {
    let mut app_map: HashMap<String, (u64, u64)> = HashMap::new();

    for event in events {
        if let Some(app_name) = event_app_name(event) {
            let entry = app_map.entry(app_name).or_insert((0, 0));
            entry.0 += 1;
        }
    }

    for frame in frames {
        let entry = app_map.entry(frame.app_name.clone()).or_insert((0, 0));
        entry.1 += 1;
    }

    let from_rfc = from.to_rfc3339();
    let to_rfc = to.to_rfc3339();
    let session_app_durations: HashMap<String, i64> = ctx
        .storage
        .get_app_durations_by_date(&from_rfc, &to_rfc)
        .map(|durations| durations.into_iter().collect())
        .unwrap_or_default();

    let mut app_stats: Vec<AppStat> = app_map
        .into_iter()
        .map(|(name, (events, captures))| {
            let duration_secs = session_app_durations
                .get(&name)
                .map(|&duration| duration as u64)
                .unwrap_or(events * 5);
            AppStat {
                name,
                duration_secs,
                events,
                captures,
                percentage: 0.0,
            }
        })
        .collect();

    let total_app_duration: u64 = app_stats.iter().map(|stat| stat.duration_secs).sum();
    for stat in &mut app_stats {
        stat.percentage = if total_app_duration > 0 {
            (stat.duration_secs as f64 / total_app_duration as f64) * 100.0
        } else {
            0.0
        };
    }
    app_stats.sort_by_key(|a| std::cmp::Reverse(a.duration_secs));
    app_stats.truncate(10);
    app_stats
}

pub(crate) fn build_hourly_activity(
    events: &[Event],
    frames: &[oneshim_core::models::storage_records::FrameRecord],
) -> Vec<oneshim_api_contracts::reports::HourlyActivity> {
    let mut hourly = [0u64; 24];
    for event in events {
        let hour = event_timestamp(event).hour() as usize;
        hourly[hour] += 1;
    }
    for frame in frames {
        if let Some(hour) = frame_hour(frame) {
            hourly[hour] += 1;
        }
    }

    hourly
        .iter()
        .enumerate()
        .map(|(hour, &activity)| assemble_hourly_activity(hour, activity))
        .collect()
}

pub(crate) fn resolve_total_active_secs(daily_stats: &[DailyStat], total_events: u64) -> u64 {
    let sum: u64 = daily_stats.iter().map(|stat| stat.active_secs).sum();
    if sum > 0 {
        sum
    } else {
        total_events * 5
    }
}

pub(crate) fn build_productivity(
    daily_stats: &[DailyStat],
    total_active_secs: u64,
    total_idle_secs: u64,
    app_stats: &[AppStat],
    hourly_activity: &[oneshim_api_contracts::reports::HourlyActivity],
) -> oneshim_api_contracts::reports::ProductivityMetrics {
    let total_time = total_active_secs + total_idle_secs;
    let active_ratio = if total_time > 0 {
        (total_active_secs as f64 / total_time as f64) * 100.0
    } else {
        0.0
    };

    let peak_hour = hourly_activity
        .iter()
        .max_by_key(|entry| entry.activity)
        .map(|entry| entry.hour)
        .unwrap_or(9);

    let top_app = app_stats
        .first()
        .map(|app| app.name.clone())
        .unwrap_or_default();

    let trend = if daily_stats.len() >= 2 {
        let first_half: u64 = daily_stats
            .iter()
            .take(daily_stats.len() / 2)
            .map(|stat| stat.events)
            .sum();
        let second_half: u64 = daily_stats
            .iter()
            .skip(daily_stats.len() / 2)
            .map(|stat| stat.events)
            .sum();
        if first_half > 0 {
            ((second_half as f64 - first_half as f64) / first_half as f64) * 100.0
        } else {
            0.0
        }
    } else {
        0.0
    };

    let regularity_bonus = if daily_stats.iter().filter(|stat| stat.events > 0).count()
        >= (daily_stats.len() * 7 / 10)
    {
        10.0
    } else {
        0.0
    };
    let score = (active_ratio * 0.9 + regularity_bonus).min(100.0);

    assemble_productivity_metrics(score, active_ratio, peak_hour, top_app, trend)
}

pub(crate) fn average_or_zero(sum: f64, count: u64) -> f64 {
    if count > 0 {
        sum / count as f64
    } else {
        0.0
    }
}

fn event_timestamp(event: &Event) -> DateTime<Utc> {
    match event {
        Event::User(value) => value.timestamp,
        Event::Context(value) => value.timestamp,
        Event::System(value) => value.timestamp,
        Event::Input(value) => value.timestamp,
        Event::Process(value) => value.timestamp,
        Event::Window(value) => value.timestamp,
        Event::Clipboard(value) => value.timestamp,
        Event::FileAccess(value) => value.timestamp,
    }
}

fn event_app_name(event: &Event) -> Option<String> {
    match event {
        Event::User(value) => Some(value.app_name.clone()),
        Event::Context(value) => Some(value.app_name.clone()),
        _ => None,
    }
}
