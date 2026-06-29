use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::magic_overlay::OverlayFullscreenPolicyPayload;
use crate::runtime_state::{AppState, ConfigRuntimeState};
// ADR-026: `state.storage` is the concrete `Arc<SqliteStorage>`, so the inherent
// (synchronous) coaching/habit twins would shadow the async trait methods of the
// same name. Import the traits and dispatch via fully-qualified syntax so these
// async IPC handlers route SQLite work through the `spawn_blocking` funnels
// rather than holding the `parking_lot` guard on the async reactor.
use maekon_core::ports::web_storage::{CoachingQueryStorage, HabitStorage};
use maekon_storage::sqlite::SqliteStorage;
/// Dismiss a coaching overlay message with the given action.
/// If "later", snoozes the profile for 15 minutes.
#[command]
pub async fn dismiss_coaching_message(
    state: tauri::State<'_, AppState>,
    message_id: String,
    action: String,
    profile: String,
) -> Result<(), IpcError> {
    use maekon_core::models::coaching::DismissAction;

    let dismiss_action = match action.as_str() {
        "ok" => DismissAction::Ok,
        "later" => DismissAction::Later,
        "timeout" => DismissAction::Timeout,
        _ => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("Invalid dismiss action: {action}"),
            ));
        }
    };

    if let Some(ref overlay) = state.magic_overlay {
        overlay.dismiss(&message_id, dismiss_action).await;
    }

    // Persist dismiss feedback to SQLite
    let action_str = match dismiss_action {
        DismissAction::Ok => "ok",
        DismissAction::Later => "later",
        DismissAction::Timeout => "timeout",
    };
    let dismissed_at = chrono::Utc::now().to_rfc3339();
    // `update_coaching_event_feedback` has no async twin (it acquires the
    // single-connection `parking_lot` write guard synchronously), so offload it
    // onto `spawn_blocking` to keep the SQLite work off the async reactor. Clone
    // the storage `Arc` and move owned args into the blocking closure.
    let storage = state.storage.clone();
    let event_id = message_id.clone();
    let action_owned = action_str.to_string();
    let dismissed_at_owned = dismissed_at.clone();
    let persist_result = tokio::task::spawn_blocking(move || {
        storage.update_coaching_event_feedback(
            &event_id,
            Some(&action_owned),
            Some(&dismissed_at_owned),
            None,
            None,
        )
    })
    .await;
    match persist_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("coaching dismiss persist failure: {e}"),
        Err(join_err) => {
            tracing::warn!("coaching dismiss persist task join failure: {join_err}")
        }
    }

    // If "Later", snooze the profile for 15 minutes via CoachingPort
    if dismiss_action == DismissAction::Later {
        if let Some(ref engine) = state.coaching_engine {
            engine.snooze_profile(&profile, 900).await;
        }
    }

    // Return overlay to click-through mode after dismissal
    if let Some(ref overlay) = state.magic_overlay {
        overlay.set_interactive(false);
    }

    Ok(())
}

/// Record explicit feedback (thumbs-up/down) for a coaching message.
#[command]
pub async fn submit_coaching_feedback(
    state: tauri::State<'_, AppState>,
    message_id: String,
    positive: bool,
) -> Result<(), IpcError> {
    if let Some(ref engine) = state.coaching_engine {
        engine.record_feedback(&message_id, positive).await;
    }
    Ok(())
}

/// Set the overlay display mode (minimal/rich/adaptive).
#[command]
pub async fn set_overlay_mode(
    state: tauri::State<'_, AppState>,
    mode: String,
) -> Result<(), IpcError> {
    use maekon_core::config::OverlayMode;

    let overlay_mode = match mode.as_str() {
        "minimal" => OverlayMode::Minimal,
        "rich" => OverlayMode::Rich,
        "adaptive" => OverlayMode::Adaptive,
        _ => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("Invalid overlay mode: {mode}"),
            ));
        }
    };

    if let Some(ref overlay) = state.magic_overlay {
        overlay.set_mode(overlay_mode).await;
    }
    Ok(())
}

/// Cycle overlay mode: Minimal → Rich → Adaptive → Minimal.
#[command]
pub async fn toggle_overlay_mode(state: tauri::State<'_, AppState>) -> Result<String, IpcError> {
    if let Some(ref overlay) = state.magic_overlay {
        overlay.toggle_mode().await;
        let mode = overlay.get_mode().await;
        Ok(format!("{:?}", mode))
    } else {
        // Precondition: overlay must be initialized. The frontend should
        // retry after overlay activation or surface this as "feature
        // temporarily unavailable".
        Err(IpcError::new(
            "service.unavailable",
            "overlay not available",
        ))
    }
}

/// Get current overlay state (mode and visibility).
#[command]
pub async fn get_overlay_state(
    state: tauri::State<'_, AppState>,
) -> Result<OverlayStateResponse, IpcError> {
    use maekon_core::config::OverlayMode;

    let (mode, visible) = if let Some(ref overlay) = state.magic_overlay {
        (overlay.get_mode().await, overlay.is_visible().await)
    } else {
        (OverlayMode::Minimal, false)
    };

    Ok(OverlayStateResponse {
        mode: format!("{:?}", mode).to_lowercase(),
        visible,
    })
}

/// Overlay state response for IPC.
#[derive(Serialize)]
pub struct OverlayStateResponse {
    pub mode: String,
    pub visible: bool,
}

/// Fullscreen policy response for overlay IPC diagnostics.
#[derive(Serialize)]
pub struct OverlayFullscreenPolicyResponse {
    pub ok: bool,
    pub state: Option<OverlayFullscreenPolicyPayload>,
}

/// Toggle overlay between click-through and interactive modes.
#[command]
pub async fn toggle_overlay_interactive(
    state: tauri::State<'_, AppState>,
    interactive: bool,
) -> Result<(), IpcError> {
    if let Some(ref overlay) = state.magic_overlay {
        overlay.set_interactive(interactive);
    }
    Ok(())
}

/// Return the latest fullscreen policy decision for overlay triggering.
#[command]
pub async fn get_overlay_fullscreen_policy_state(
    state: tauri::State<'_, AppState>,
) -> Result<OverlayFullscreenPolicyResponse, IpcError> {
    Ok(OverlayFullscreenPolicyResponse {
        ok: true,
        state: state
            .magic_overlay
            .as_ref()
            .and_then(|overlay| overlay.fullscreen_policy_state()),
    })
}

/// Toggle overlay suggestions panel mode.
///
/// When `open = true`, resizes the overlay to a compact strip on the right
/// edge so only the panel area captures mouse events (the rest of the desktop
/// remains interactive). When `open = false`, recalculates layout based on
/// remaining active modes (detection, automation may keep it full-screen).
#[command]
pub async fn toggle_suggestions_panel(
    state: tauri::State<'_, AppState>,
    open: bool,
) -> Result<(), IpcError> {
    if let Some(ref overlay) = state.magic_overlay {
        overlay.set_panel_mode(open).await;
    }
    Ok(())
}

/// Toggle overlay automation confirmation mode.
///
/// Full-screen interactive — highest priority mode. The modal has a
/// full-screen backdrop so the entire overlay must capture events.
/// When dismissed, recalculates layout based on remaining active modes.
#[command]
pub async fn toggle_automation_confirm(
    state: tauri::State<'_, AppState>,
    active: bool,
) -> Result<(), IpcError> {
    if let Some(ref overlay) = state.magic_overlay {
        overlay.set_automation_confirm_mode(active).await;
    }
    Ok(())
}

/// Get coaching event history with pagination.
#[command]
pub async fn get_coaching_history(
    state: tauri::State<'_, AppState>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Vec<maekon_core::models::coaching::CoachingEventRow>, IpcError> {
    <SqliteStorage as CoachingQueryStorage>::query_coaching_events(
        &state.storage,
        limit.unwrap_or(50),
        offset.unwrap_or(0),
    )
    .await
    .map_err(IpcError::from)
}

/// Get goal progress for all configured regimes.
#[command]
pub async fn get_goal_progress(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<maekon_core::models::coaching::GoalProgressView>, IpcError> {
    if let Some(ref engine) = state.coaching_engine {
        Ok(engine.all_goal_progress().await)
    } else {
        Ok(vec![])
    }
}

/// Get habit streak data for all regimes within the last N days.
#[command]
pub async fn get_habit_streaks(
    state: tauri::State<'_, AppState>,
    days: u32,
) -> Result<Vec<maekon_core::models::coaching::HabitStreakRow>, IpcError> {
    <SqliteStorage as HabitStorage>::query_habit_streaks(&state.storage, days)
        .await
        .map_err(IpcError::from)
}

/// Update regime goal targets and persist to config.
#[command]
pub async fn update_regime_goals(
    state: tauri::State<'_, AppState>,
    config_state: tauri::State<'_, ConfigRuntimeState>,
    goals: std::collections::HashMap<String, u32>,
) -> Result<(), IpcError> {
    if let Some(ref engine) = state.coaching_engine {
        engine.update_regime_goals(&goals).await;
    }

    // Persist to config file
    config_state
        .config_manager()
        .update_with(|config| {
            config.coaching.regime_goals = goals.clone();
            Ok(())
        })
        .map_err(IpcError::from)?;

    Ok(())
}
