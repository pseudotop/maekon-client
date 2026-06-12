//! Idle state tracking and transition helpers.

use std::sync::Arc;

use chrono::Utc;
use maekon_api_contracts::stream::{IdleUpdate, RealtimeEvent};
use maekon_core::models::activity::IdleState;
use maekon_monitor::idle::IdleTracker;
use maekon_monitor::input_activity::InputActivityCollector;
use tokio::sync::broadcast;
use tracing::{debug, warn};

use crate::notification_manager::NotificationManager;
use crate::scheduler::config::SchedulerStorage;

/// Process idle state transitions: start/end idle periods in storage,
/// reset notifications on resume, and check idle notification thresholds.
/// Returns the updated `prev_idle_secs` value for the caller to persist.
pub(crate) async fn handle_idle_tick(
    idle_tracker: &mut IdleTracker,
    sqlite: &Arc<dyn SchedulerStorage>,
    notif: &Option<Arc<NotificationManager>>,
    input_collector: &InputActivityCollector,
    prev_idle_secs: u64,
    focus_mode_active: bool,
    event_tx: &Option<broadcast::Sender<RealtimeEvent>>,
) -> u64 {
    // Capture previous state BEFORE check_idle() updates it, so edge detection
    // (`prev_state == Active && current == Idle`) works correctly.
    let prev_state = idle_tracker.previous_state();
    let idle_info = idle_tracker.check_idle().await;

    if prev_state == IdleState::Active && idle_info.state == IdleState::Idle {
        // Storage FIRST (spec §U2 I2 ordering). Log-and-continue on failure.
        match sqlite.start_idle_period(Utc::now()).await {
            Ok(id) => {
                idle_tracker.set_idle_period_id(Some(id));
                debug!("idle period started: id={}", id);
            }
            Err(e) => warn!("idle period started record failure: {e}"),
        }
        // Emit AFTER storage (success or failure — subscribers observe the edge).
        if let Some(tx) = event_tx.as_ref() {
            let ev = RealtimeEvent::Idle(IdleUpdate {
                is_idle: true,
                idle_secs: idle_info.idle_secs,
            });
            if let Err(e) = tx.send(ev) {
                debug!("idle event channel send failed (active->idle): {e}");
            }
        }
    } else if prev_state == IdleState::Idle && idle_info.state == IdleState::Active {
        if let Some(id) = idle_tracker.idle_period_id() {
            if let Err(e) = sqlite.end_idle_period(id, Utc::now()).await {
                warn!("idle period ended record failure: {e}");
            }
            idle_tracker.set_idle_period_id(None);
        }
        if let Some(ref notif) = notif {
            notif.reset_session().await;
        }
        // Emit AFTER storage + notif-reset (success or failure — subscribers observe the edge).
        // idle_period_id may be None on cold-start (user was idle before process
        // started); emission proceeds regardless so subscribers observe the resume.
        if let Some(tx) = event_tx.as_ref() {
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
        if let Some(ref notif) = notif {
            notif.check_idle(idle_info.idle_secs).await;
        }
    }

    input_collector.estimate_from_idle_change(prev_idle_secs, idle_info.idle_secs);
    idle_info.idle_secs
}
