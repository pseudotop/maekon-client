use chrono::Utc;
use oneshim_api_contracts::reports::{ReportQuery, ReportResponse};

use crate::error::ApiError;
use crate::services::reports_assembler::{
    assemble_report_response, memory_pct, ReportResponseInput,
};
use crate::services::reports_query_support::{
    average_or_zero, build_app_stats, build_daily_stats, build_hourly_activity, build_productivity,
    resolve_report_window, resolve_total_active_secs, DailyStatsInput,
};
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct ReportQueryService {
    ctx: StorageWebContext,
}

impl ReportQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub async fn generate_report(&self, params: &ReportQuery) -> Result<ReportResponse, ApiError> {
        let now = Utc::now();
        let (window, title) = resolve_report_window(params, now)?;
        // Decompose for downstream out-of-plan-scope storage methods + formatters
        // that still take DateTime<Utc>.
        let oneshim_core::types::TimeWindow {
            start: from,
            end: to,
        } = window;
        let days = ((to - from).num_days() as u32).max(1);

        let metrics = self.ctx.storage.get_metrics(from, to, 100000).await?;
        let events = self.ctx.storage.get_events(from, to, 100000).await?;
        let frames = self.ctx.storage.get_frames(from, to, 100000)?;
        let idle_periods = self.ctx.storage.get_idle_periods(from, to).await?;

        let total_captures = frames.len() as u64;
        let total_events = events.len() as u64;
        let total_idle_secs: u64 = idle_periods
            .iter()
            .filter_map(|period| period.duration_secs)
            .sum();

        let (cpu_sum, mem_sum, met_count) =
            metrics.iter().fold((0.0f64, 0.0f64, 0u64), |acc, metric| {
                (
                    acc.0 + metric.cpu_usage as f64,
                    acc.1 + memory_pct(metric),
                    acc.2 + 1,
                )
            });
        let avg_cpu = average_or_zero(cpu_sum, met_count);
        let avg_memory = average_or_zero(mem_sum, met_count);

        let mut daily_stats = build_daily_stats(DailyStatsInput {
            ctx: &self.ctx,
            from,
            to,
            metrics: &metrics,
            events: &events,
            frames: &frames,
            idle_periods: &idle_periods,
        });
        daily_stats.sort_by(|left, right| left.date.cmp(&right.date));

        let app_stats = build_app_stats(&self.ctx, from, to, &events, &frames);
        let hourly_activity = build_hourly_activity(&events, &frames);
        let total_active_secs = resolve_total_active_secs(&daily_stats, total_events);
        let productivity = build_productivity(
            &daily_stats,
            total_active_secs,
            total_idle_secs,
            &app_stats,
            &hourly_activity,
        );

        Ok(assemble_report_response(ReportResponseInput {
            title,
            from_date: from.format("%Y-%m-%d").to_string(),
            to_date: to.format("%Y-%m-%d").to_string(),
            days,
            total_active_secs,
            total_idle_secs,
            total_captures,
            total_events,
            avg_cpu,
            avg_memory,
            daily_stats,
            app_stats,
            hourly_activity,
            productivity,
        }))
    }
}
