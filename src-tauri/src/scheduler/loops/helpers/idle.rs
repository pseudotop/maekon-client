//! Idle state tracking and transition helpers.

use std::sync::Arc;

use chrono::Utc;
use maekon_analysis::focus_analyzer::FocusAnalyzer;
use maekon_api_contracts::stream::{IdleUpdate, RealtimeEvent};
use maekon_core::models::activity::IdleState;
use maekon_core::models::suggestion::Suggestion;
use maekon_monitor::idle::IdleTracker;
use maekon_monitor::input_activity::InputActivityCollector;
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::notification_manager::NotificationManager;
use crate::scheduler::SchedulerStorage;

pub(crate) struct IdleTickOutcome {
    pub(crate) idle_secs: u64,
    pub(crate) resume_suggestions: Vec<Suggestion>,
}

pub(crate) struct IdleTickServices<'a> {
    pub(crate) sqlite: &'a Arc<dyn SchedulerStorage>,
    pub(crate) notif: &'a Option<Arc<NotificationManager>>,
    pub(crate) focus: &'a Option<Arc<FocusAnalyzer>>,
    pub(crate) input_collector: &'a InputActivityCollector,
    pub(crate) event_tx: &'a Option<broadcast::Sender<RealtimeEvent>>,
}

pub(crate) async fn handle_idle_resume_edge(
    idle_tracker: &mut IdleTracker,
    sqlite: &Arc<dyn SchedulerStorage>,
    notif: &Option<Arc<NotificationManager>>,
    focus: &Option<Arc<FocusAnalyzer>>,
) -> Vec<Suggestion> {
    if let Some(id) = idle_tracker.idle_period_id() {
        if let Err(e) = sqlite.end_idle_period(id, Utc::now()).await {
            warn!("idle period ended record failure: {e}");
        }
        idle_tracker.set_idle_period_id(None);
    }
    if let Some(ref notif) = notif {
        notif.reset_session().await;
    }
    if let Some(focus) = focus.as_ref() {
        focus.on_idle_resume().await
    } else {
        Vec::new()
    }
}

/// Process idle state transitions: start/end idle periods in storage,
/// reset notifications on resume, and check idle notification thresholds.
/// Returns the updated `prev_idle_secs` value for the caller to persist.
pub(crate) async fn handle_idle_tick(
    idle_tracker: &mut IdleTracker,
    services: IdleTickServices<'_>,
    prev_idle_secs: u64,
    focus_mode_active: bool,
) -> IdleTickOutcome {
    // Capture previous state BEFORE check_idle() updates it, so edge detection
    // (`prev_state == Active && current == Idle`) works correctly.
    let prev_state = idle_tracker.previous_state();
    let idle_info = idle_tracker.check_idle().await;
    let mut resume_suggestions = Vec::new();

    if prev_state == IdleState::Active && idle_info.state == IdleState::Idle {
        // Storage FIRST (spec §U2 I2 ordering). Log-and-continue on failure.
        match services.sqlite.start_idle_period(Utc::now()).await {
            Ok(id) => {
                idle_tracker.set_idle_period_id(Some(id));
                debug!("idle period started: id={}", id);
            }
            Err(e) => warn!("idle period started record failure: {e}"),
        }
        // Emit AFTER storage (success or failure — subscribers observe the edge).
        if let Some(tx) = services.event_tx.as_ref() {
            let ev = RealtimeEvent::Idle(IdleUpdate {
                is_idle: true,
                idle_secs: idle_info.idle_secs,
            });
            if let Err(e) = tx.send(ev) {
                debug!("idle event channel send failed (active->idle): {e}");
            }
        }
    } else if prev_state == IdleState::Idle && idle_info.state == IdleState::Active {
        resume_suggestions = handle_idle_resume_edge(
            idle_tracker,
            services.sqlite,
            services.notif,
            services.focus,
        )
        .await;
        // Emit AFTER storage + notif-reset (success or failure — subscribers observe the edge).
        // idle_period_id may be None on cold-start (user was idle before process
        // started); emission proceeds regardless so subscribers observe the resume.
        if let Some(tx) = services.event_tx.as_ref() {
            let ev = RealtimeEvent::Idle(IdleUpdate {
                is_idle: false,
                idle_secs: idle_info.idle_secs,
            });
            if let Err(e) = tx.send(ev) {
                debug!("idle event channel send failed (idle->active): {e}");
            }
        }
    }

    // A4: Suppress idle notification in focus mode (UNCHANGED)
    if !focus_mode_active {
        if let Some(ref notif) = services.notif {
            notif.check_idle(idle_info.idle_secs).await;
        }
    }

    services
        .input_collector
        .estimate_from_idle_change(prev_idle_secs, idle_info.idle_secs);
    IdleTickOutcome {
        idle_secs: idle_info.idle_secs,
        resume_suggestions,
    }
}
