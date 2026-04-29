use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::handlers;
use crate::AppState;

pub fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/metrics", get(handlers::metrics::get_metrics))
        .route(
            "/metrics/hourly",
            get(handlers::metrics::get_hourly_metrics),
        )
        .route("/processes", get(handlers::processes::get_processes))
        .route("/idle", get(handlers::idle::get_idle_periods))
        .route("/sessions", get(handlers::sessions::list_sessions))
        .route("/sessions/{id}", get(handlers::sessions::get_session))
        .route("/frames", get(handlers::frames::get_frames))
        .route("/frames/{id}/image", get(handlers::frames::get_frame_image))
        .route(
            "/frames/{frame_id}/annotations",
            get(handlers::annotations::list_annotations)
                .post(handlers::annotations::create_annotation),
        )
        .route(
            "/frames/{frame_id}/annotations/{annotation_id}",
            delete(handlers::annotations::delete_annotation),
        )
        .route("/events", get(handlers::events::get_events))
        .route("/stats/summary", get(handlers::stats::get_summary))
        .route("/stats/apps", get(handlers::stats::get_app_usage))
        .route("/stats/heatmap", get(handlers::stats::get_heatmap))
        .route("/stats/gui-heatmap", get(handlers::stats::get_gui_heatmap))
        .route("/reports", get(handlers::reports::generate_report))
        .route("/settings", get(handlers::settings::get_settings))
        .route("/settings", post(handlers::settings::update_settings))
        .route(
            "/ai/provider-surfaces",
            get(handlers::ai_provider_surfaces::list_provider_surfaces),
        )
        .route(
            "/ai/providers/models",
            post(handlers::ai_models::discover_provider_models),
        )
        .route(
            "/ai/sessions",
            post(handlers::ai_session::create_session).get(handlers::ai_session::list_sessions),
        )
        .route(
            "/ai/sessions/{id}",
            get(handlers::ai_session::get_session).delete(handlers::ai_session::delete_session),
        )
        .route(
            "/ai/sessions/{id}/messages",
            post(handlers::ai_session::send_message),
        )
        .route(
            "/integration/status",
            get(handlers::integration::get_status),
        )
        .route("/integration/audit", get(handlers::integration::get_audit))
        .route(
            "/integration/auth/status",
            get(handlers::integration::get_auth_status),
        )
        .route(
            "/integration/auth/device/start",
            post(handlers::integration::start_device_authorization),
        )
        .route(
            "/integration/auth/device/poll",
            post(handlers::integration::poll_device_authorization),
        )
        .route(
            "/integration/auth/device/cancel",
            post(handlers::integration::cancel_device_authorization),
        )
        .route(
            "/integration/auth/reset",
            post(handlers::integration::reset_auth_state),
        )
        .route("/integration/inbox", get(handlers::integration::list_inbox))
        .route(
            "/integration/inbox/refresh",
            post(handlers::integration::refresh_inbox),
        )
        .route(
            "/integration/inbox/{prompt_id}/ack",
            post(handlers::integration::acknowledge_inbox_prompt),
        )
        .route(
            "/integration/inbox/{prompt_id}/dismiss",
            post(handlers::integration::dismiss_inbox_prompt),
        )
        .route("/storage/stats", get(handlers::settings::get_storage_stats))
        .route("/data/range", delete(handlers::data::delete_data_range))
        .route("/data/all", delete(handlers::data::delete_all_data))
        .route("/search", get(handlers::search::search))
        .route("/stream", get(handlers::stream::event_stream))
        .route("/export/metrics", get(handlers::export::export_metrics))
        .route("/export/events", get(handlers::export::export_events))
        .route("/export/frames", get(handlers::export::export_frames))
        .route("/export/ical", get(handlers::export::export_ical))
        .route("/export/toggl", get(handlers::export::export_toggl))
        .route("/backup", get(handlers::backup::create_backup))
        .route("/backup/restore", post(handlers::backup::restore_backup))
        .route("/tags", get(handlers::tags::list_tags))
        .route("/tags", post(handlers::tags::create_tag))
        .route("/tags/{id}", get(handlers::tags::get_tag))
        .route("/tags/{id}", put(handlers::tags::update_tag))
        .route("/tags/{id}", delete(handlers::tags::delete_tag))
        .route(
            "/frames/{frame_id}/tags",
            get(handlers::tags::get_frame_tags),
        )
        .route("/timeline", get(handlers::timeline::get_timeline))
        .route("/frames/batch-tags", post(handlers::tags::batch_add_tag))
        .route(
            "/frames/{frame_id}/tags/{tag_id}",
            post(handlers::tags::add_tag_to_frame),
        )
        .route(
            "/frames/{frame_id}/tags/{tag_id}",
            delete(handlers::tags::remove_tag_from_frame),
        )
        .route("/suggestions", get(handlers::suggestions::list_suggestions))
        .route(
            "/suggestions/{id}/dismiss",
            post(handlers::suggestions::dismiss_suggestion),
        )
        .route("/focus/metrics", get(handlers::focus::get_focus_metrics))
        .route("/focus/sessions", get(handlers::focus::get_work_sessions))
        .route(
            "/focus/interruptions",
            get(handlers::focus::get_interruptions),
        )
        .route("/focus/suggestions", get(handlers::focus::get_suggestions))
        .route(
            "/focus/suggestions/{id}/feedback",
            post(handlers::focus::submit_suggestion_feedback),
        )
        .route(
            "/automation/status",
            get(handlers::automation::get_automation_status),
        )
        .route(
            "/automation/contracts",
            get(handlers::automation::get_contract_versions),
        )
        .route(
            "/automation/audit",
            get(handlers::automation::get_audit_logs),
        )
        .route(
            "/automation/policy-events",
            get(handlers::automation::get_policy_events),
        )
        .route(
            "/automation/policies",
            get(handlers::automation::get_policies),
        )
        .route(
            "/automation/execution-policies",
            get(handlers::automation::list_execution_policies)
                .post(handlers::automation::create_execution_policy),
        )
        .route(
            "/automation/execution-policies/{id}",
            put(handlers::automation::update_execution_policy)
                .delete(handlers::automation::delete_execution_policy),
        )
        .route(
            "/automation/stats",
            get(handlers::automation::get_automation_stats),
        )
        .route(
            "/automation/presets",
            get(handlers::automation::list_presets),
        )
        .route(
            "/automation/presets",
            post(handlers::automation::create_preset),
        )
        .route(
            "/automation/presets/{id}",
            put(handlers::automation::update_preset),
        )
        .route(
            "/automation/presets/{id}",
            delete(handlers::automation::delete_preset),
        )
        .route(
            "/automation/presets/{id}/run",
            post(handlers::automation::run_preset),
        )
        .route(
            "/automation/execute-hint",
            post(handlers::automation::execute_intent_hint),
        )
        .route(
            "/automation/execute-scene-action",
            post(handlers::automation::execute_scene_action),
        )
        .route(
            "/automation/scene",
            get(handlers::automation::get_automation_scene),
        )
        .route(
            "/automation/scene/calibration",
            get(handlers::automation::get_automation_scene_calibration),
        )
        .route(
            "/automation/gui/sessions",
            post(handlers::automation_gui::create_gui_session),
        )
        .route(
            "/automation/gui/sessions/{id}",
            get(handlers::automation_gui::get_gui_session)
                .delete(handlers::automation_gui::delete_gui_session),
        )
        .route(
            "/automation/gui/sessions/{id}/highlight",
            post(handlers::automation_gui::highlight_gui_session),
        )
        .route(
            "/automation/gui/sessions/{id}/confirm",
            post(handlers::automation_gui::confirm_gui_session),
        )
        .route(
            "/automation/gui/sessions/{id}/execute",
            post(handlers::automation_gui::execute_gui_session),
        )
        .route(
            "/automation/gui/sessions/{id}/events",
            get(handlers::automation_gui::gui_session_event_stream),
        )
        .route(
            "/onboarding/quickstart",
            get(handlers::onboarding::get_quickstart),
        )
        .route(
            "/support/diagnostics",
            get(handlers::support::get_diagnostics),
        )
        .route(
            "/support/bug-report",
            post(handlers::bug_report::create_bug_report_with_params),
        )
        .route(
            "/support/bug-report/latest",
            get(handlers::bug_report::get_latest_bug_report),
        )
        .route("/update/status", get(handlers::update::get_update_status))
        .route("/update/action", post(handlers::update::post_update_action))
        .route("/update/stream", get(handlers::update::get_update_stream))
        .route(
            "/semantic-search",
            get(handlers::semantic_search::semantic_search),
        )
        .route("/digests", get(handlers::digests::list_digests))
        .route("/digests/current", get(handlers::digests::current_digest))
        .route(
            "/digests/daily",
            get(handlers::daily_digest::get_daily_digest),
        )
        .route(
            "/digests/daily/today",
            get(handlers::daily_digest::get_daily_digest_today),
        )
        .route(
            "/digests/daily/export",
            get(handlers::daily_digest::export_daily_digest),
        )
        .route(
            "/dashboard/day",
            get(handlers::dashboard::get_dashboard_day),
        )
        // Recalibration endpoints
        .route(
            "/recalibration/override",
            post(handlers::recalibration::create_override),
        )
        .route(
            "/recalibration/override/{id}",
            delete(handlers::recalibration::delete_override),
        )
        .route(
            "/recalibration/overrides",
            get(handlers::recalibration::list_overrides),
        )
        .route(
            "/recalibration/recluster",
            post(handlers::recalibration::trigger_recluster),
        )
        // Playbook listing endpoints
        .route(
            "/playbooks/coaching",
            get(handlers::playbooks::list_coaching_templates),
        )
        .route("/playbooks/presets", get(handlers::playbooks::list_presets))
        // Coaching endpoints
        .route(
            "/coaching/history",
            get(handlers::coaching::get_coaching_history),
        )
        .route(
            "/coaching/goals",
            get(handlers::coaching::get_goals).put(handlers::coaching::update_goals),
        )
        .route(
            "/coaching/stats/today",
            get(handlers::coaching::get_coaching_stats_today),
        )
        .route("/coaching/habits", get(handlers::coaching::get_habits))
        // Pomodoro timer
        .route("/pomodoro/start", post(handlers::pomodoro::start_pomodoro))
        .route(
            "/pomodoro/current",
            get(handlers::pomodoro::get_current_pomodoro),
        )
        .route(
            "/pomodoro/cancel",
            post(handlers::pomodoro::cancel_pomodoro),
        )
        .route(
            "/pomodoro/complete",
            post(handlers::pomodoro::complete_pomodoro),
        )
        // Tracking-schedule configuration + status (A.15/A.16)
        .route(
            "/tracking-schedule",
            get(handlers::tracking_schedule::get_config)
                .put(handlers::tracking_schedule::put_config),
        )
        .route(
            "/tracking-schedule/status",
            get(handlers::tracking_schedule::get_status),
        )
        // External gRPC live-config introspection (spec §5.11 / D29)
        .route(
            "/external-grpc/live-config",
            get(handlers::external_grpc_live_config::get_live_config),
        )
        // Audit entry export (spec §5.9 / D25 / NV1)
        .route("/audit/export", get(handlers::audit_export::export_audit))
}

pub fn integration_routes() -> Router<AppState> {
    Router::new()
        .route("/status", get(handlers::integration::get_status))
        .route("/audit", get(handlers::integration::get_audit))
        .route(
            "/ai/provider-surfaces",
            get(handlers::ai_provider_surfaces::list_provider_surfaces),
        )
        .route(
            "/ai/providers/models",
            post(handlers::ai_models::discover_provider_models_for_integration),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::broadcast;

    #[test]
    fn routes_compile() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        let state = AppState::with_core(storage, event_tx);
        let _app: Router<()> = api_routes().with_state(state);
    }

    #[test]
    fn integration_routes_compile() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        let state = AppState::with_core(storage, event_tx);
        let _app: Router<()> = integration_routes().with_state(state);
    }
}
