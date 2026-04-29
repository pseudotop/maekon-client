use oneshim_api_contracts::focus::{
    FocusMetricsDto, FocusMetricsResponse, InterruptionDto, LocalSuggestionDto, WorkSessionDto,
};
use oneshim_core::models::storage_records::{
    FocusInterruptionRecord, FocusWorkSessionRecord, LocalSuggestionRecord,
};
use oneshim_core::models::work_session::FocusMetrics;

pub(crate) fn assemble_focus_metrics(date: String, metrics: FocusMetrics) -> FocusMetricsDto {
    FocusMetricsDto {
        date,
        total_active_secs: metrics.total_active_secs,
        deep_work_secs: metrics.deep_work_secs,
        communication_secs: metrics.communication_secs,
        context_switches: metrics.context_switches,
        interruption_count: metrics.interruption_count,
        avg_focus_duration_secs: metrics.avg_focus_duration_secs,
        max_focus_duration_secs: metrics.max_focus_duration_secs,
        focus_score: metrics.focus_score,
    }
}

pub(crate) fn assemble_focus_metrics_response(
    today: FocusMetricsDto,
    history: Vec<FocusMetricsDto>,
) -> FocusMetricsResponse {
    FocusMetricsResponse { today, history }
}

pub(crate) fn assemble_work_session(row: FocusWorkSessionRecord) -> WorkSessionDto {
    WorkSessionDto {
        id: row.id,
        started_at: row.started_at,
        ended_at: row.ended_at,
        primary_app: row.primary_app,
        category: row.category,
        state: row.state,
        interruption_count: row.interruption_count,
        deep_work_secs: row.deep_work_secs,
        duration_secs: row.duration_secs,
    }
}

pub(crate) fn assemble_interruption(row: FocusInterruptionRecord) -> InterruptionDto {
    InterruptionDto {
        id: row.id,
        interrupted_at: row.interrupted_at,
        from_app: row.from_app,
        from_category: row.from_category,
        to_app: row.to_app,
        to_category: row.to_category,
        resumed_at: row.resumed_at,
        resumed_to_app: row.resumed_to_app,
        duration_secs: row.duration_secs,
    }
}

pub(crate) fn assemble_local_suggestion(row: LocalSuggestionRecord) -> LocalSuggestionDto {
    LocalSuggestionDto {
        id: row.id,
        suggestion_type: row.suggestion_type,
        payload: row.payload,
        created_at: row.created_at,
        shown_at: row.shown_at,
        dismissed_at: row.dismissed_at,
        acted_at: row.acted_at,
    }
}
