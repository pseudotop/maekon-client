//! Pomodoro focus timer REST handlers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use maekon_api_contracts::pomodoro::{PomodoroSessionResponse, StartPomodoroRequest};
use maekon_core::id_generation::generate_id;
use maekon_core::models::pomodoro::{PomodoroSession, PomodoroStatus};
use maekon_core::ports::pomodoro_store::PomodoroStorePort;
use std::sync::Arc;
use tracing::debug;

use crate::error::ApiError;
use crate::AppState;

/// Default work duration in minutes.
const DEFAULT_DURATION_MINUTES: u32 = 25;
/// Default break duration in minutes.
const DEFAULT_BREAK_MINUTES: u32 = 5;
/// Maximum allowed duration in minutes (2 hours).
const MAX_DURATION_MINUTES: u32 = 120;

fn require_store(state: &AppState) -> Result<Arc<dyn PomodoroStorePort>, ApiError> {
    state
        .session
        .pomodoro_store
        .clone()
        .ok_or_else(|| ApiError::ServiceUnavailable("Pomodoro storage is unavailable".to_string()))
}

async fn hydrate_if_needed(
    runtime: &mut crate::app_state::PomodoroRuntimeState,
    store: &dyn PomodoroStorePort,
) -> Result<(), ApiError> {
    if !runtime.hydrated {
        runtime.session = store.load_pomodoro_session().await?;
        runtime.hydrated = true;
    }
    Ok(())
}

fn session_to_response(session: &PomodoroSession) -> PomodoroSessionResponse {
    let effective = session.effective_status();
    PomodoroSessionResponse {
        id: session.id.clone(),
        started_at: session.started_at.to_rfc3339(),
        duration_minutes: session.duration_minutes,
        break_minutes: session.break_minutes,
        status: match effective {
            PomodoroStatus::Running => "running".to_string(),
            PomodoroStatus::OnBreak => "on_break".to_string(),
            PomodoroStatus::Completed => "completed".to_string(),
            PomodoroStatus::Cancelled => "cancelled".to_string(),
        },
        remaining_secs: session.remaining_secs(),
        completed_at: session.completed_at.map(|t| t.to_rfc3339()),
    }
}

/// POST /api/pomodoro/start — start a new Pomodoro session.
// Guard is intentionally held across hydrate + mutation so the session state
// transition is atomic (#8685 public clippy gate; house precedent in lib.rs).
#[allow(clippy::significant_drop_tightening)]
pub async fn start_pomodoro(
    State(state): State<AppState>,
    Json(request): Json<StartPomodoroRequest>,
) -> Result<(StatusCode, Json<PomodoroSessionResponse>), ApiError> {
    debug!("POST /api/pomodoro/start");

    let duration = request.duration_minutes.unwrap_or(DEFAULT_DURATION_MINUTES);
    let break_mins = request.break_minutes.unwrap_or(DEFAULT_BREAK_MINUTES);

    if duration == 0 || duration > MAX_DURATION_MINUTES {
        return Err(ApiError::BadRequest(format!(
            "duration_minutes must be between 1 and {MAX_DURATION_MINUTES}"
        )));
    }
    if break_mins > MAX_DURATION_MINUTES {
        return Err(ApiError::BadRequest(format!(
            "break_minutes must be at most {MAX_DURATION_MINUTES}"
        )));
    }

    let store = require_store(&state)?;
    let mut runtime = state.session.pomodoro.lock().await;
    hydrate_if_needed(&mut runtime, store.as_ref()).await?;

    if let Some(existing) = runtime.session.as_ref() {
        let eff = existing.effective_status();
        if eff == PomodoroStatus::Running || eff == PomodoroStatus::OnBreak {
            return Err(ApiError::Conflict(
                "A Pomodoro session is already active".to_string(),
            ));
        }
    }

    let session = PomodoroSession::new(generate_id("pomo"), duration, break_mins);
    store.save_pomodoro_session(&session).await?;
    let response = session_to_response(&session);
    runtime.session = Some(session);

    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /api/pomodoro/current — get current session status + remaining time.
// Guard is intentionally held across hydrate + mutation so the session state
// transition is atomic (#8685 public clippy gate; house precedent in lib.rs).
#[allow(clippy::significant_drop_tightening)]
pub async fn get_current_pomodoro(
    State(state): State<AppState>,
) -> Result<Json<Option<PomodoroSessionResponse>>, ApiError> {
    debug!("GET /api/pomodoro/current");

    let store = require_store(&state)?;
    let mut runtime = state.session.pomodoro.lock().await;
    hydrate_if_needed(&mut runtime, store.as_ref()).await?;
    let response = runtime.session.as_ref().map(session_to_response);
    Ok(Json(response))
}

/// POST /api/pomodoro/cancel — cancel the current session.
// Guard is intentionally held across hydrate + mutation so the session state
// transition is atomic (#8685 public clippy gate; house precedent in lib.rs).
#[allow(clippy::significant_drop_tightening)]
pub async fn cancel_pomodoro(
    State(state): State<AppState>,
) -> Result<Json<PomodoroSessionResponse>, ApiError> {
    debug!("POST /api/pomodoro/cancel");

    let store = require_store(&state)?;
    let mut runtime = state.session.pomodoro.lock().await;
    hydrate_if_needed(&mut runtime, store.as_ref()).await?;
    let mut session = runtime
        .session
        .clone()
        .ok_or_else(|| ApiError::NotFound("No active Pomodoro session".to_string()))?;

    let eff = session.effective_status();
    if eff == PomodoroStatus::Completed || eff == PomodoroStatus::Cancelled {
        return Err(ApiError::Conflict(
            "Session is already finished".to_string(),
        ));
    }

    session.status = PomodoroStatus::Cancelled;
    session.completed_at = Some(chrono::Utc::now());
    store.save_pomodoro_session(&session).await?;
    runtime.session = Some(session.clone());
    Ok(Json(session_to_response(&session)))
}

/// POST /api/pomodoro/complete — mark session as completed (auto or manual).
// Guard is intentionally held across hydrate + mutation so the session state
// transition is atomic (#8685 public clippy gate; house precedent in lib.rs).
#[allow(clippy::significant_drop_tightening)]
pub async fn complete_pomodoro(
    State(state): State<AppState>,
) -> Result<Json<PomodoroSessionResponse>, ApiError> {
    debug!("POST /api/pomodoro/complete");

    let store = require_store(&state)?;
    let mut runtime = state.session.pomodoro.lock().await;
    hydrate_if_needed(&mut runtime, store.as_ref()).await?;
    let mut session = runtime
        .session
        .clone()
        .ok_or_else(|| ApiError::NotFound("No active Pomodoro session".to_string()))?;

    if session.status == PomodoroStatus::Cancelled {
        return Err(ApiError::Conflict(
            "Session was already cancelled".to_string(),
        ));
    }

    session.status = PomodoroStatus::Completed;
    session.completed_at = Some(chrono::Utc::now());
    store.save_pomodoro_session(&session).await?;
    runtime.session = Some(session.clone());
    Ok(Json(session_to_response(&session)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use maekon_storage::sqlite::SqliteStorage;

    fn state_with_store(storage: Arc<SqliteStorage>) -> AppState {
        let (event_tx, _) = tokio::sync::broadcast::channel(4);
        let mut state = AppState::with_core(storage.clone(), event_tx);
        state.session.pomodoro_store = Some(storage);
        state
    }

    #[test]
    fn session_to_response_running() {
        let session = PomodoroSession::new("abc".to_string(), 25, 5);
        let resp = session_to_response(&session);
        assert_eq!(resp.status, "running");
        assert_eq!(resp.duration_minutes, 25);
        assert_eq!(resp.break_minutes, 5);
        assert!(resp.remaining_secs > 0);
        assert!(resp.completed_at.is_none());
    }

    #[test]
    fn session_to_response_cancelled() {
        let mut session = PomodoroSession::new("def".to_string(), 25, 5);
        session.status = PomodoroStatus::Cancelled;
        session.completed_at = Some(Utc::now());
        let resp = session_to_response(&session);
        assert_eq!(resp.status, "cancelled");
        assert!(resp.completed_at.is_some());
    }

    #[test]
    fn session_to_response_auto_break() {
        let mut session = PomodoroSession::new("ghi".to_string(), 25, 5);
        // Move started_at back so elapsed > work_secs
        session.started_at = Utc::now() - Duration::minutes(26);
        let resp = session_to_response(&session);
        assert_eq!(resp.status, "on_break");
    }

    #[test]
    fn session_to_response_auto_completed() {
        let mut session = PomodoroSession::new("jkl".to_string(), 25, 5);
        // Move started_at back so elapsed > work_secs + break_secs
        session.started_at = Utc::now() - Duration::minutes(31);
        let resp = session_to_response(&session);
        assert_eq!(resp.status, "completed");
        assert_eq!(resp.remaining_secs, 0);
    }

    #[tokio::test]
    async fn transitions_persist_across_fresh_runtime_state() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let first_state = state_with_store(storage.clone());
        let (_, Json(started)) = start_pomodoro(
            State(first_state),
            Json(StartPomodoroRequest {
                duration_minutes: Some(25),
                break_minutes: Some(5),
            }),
        )
        .await
        .unwrap();
        assert_eq!(started.status, "running");

        let restarted_state = state_with_store(storage.clone());
        let Json(restored) = get_current_pomodoro(State(restarted_state.clone()))
            .await
            .unwrap();
        assert_eq!(restored.unwrap().id, started.id);

        let Json(cancelled) = cancel_pomodoro(State(restarted_state)).await.unwrap();
        assert_eq!(cancelled.status, "cancelled");
        let after_cancel_restart = state_with_store(storage.clone());
        let Json(restored_cancelled) = get_current_pomodoro(State(after_cancel_restart.clone()))
            .await
            .unwrap();
        assert_eq!(restored_cancelled.unwrap().status, "cancelled");

        let (_, Json(restarted)) = start_pomodoro(
            State(after_cancel_restart.clone()),
            Json(StartPomodoroRequest {
                duration_minutes: Some(25),
                break_minutes: Some(5),
            }),
        )
        .await
        .unwrap();
        assert_ne!(restarted.id, started.id);
        let Json(completed) = complete_pomodoro(State(after_cancel_restart))
            .await
            .unwrap();
        assert_eq!(completed.status, "completed");

        let after_complete_restart = state_with_store(storage);
        let Json(restored_completed) = get_current_pomodoro(State(after_complete_restart))
            .await
            .unwrap();
        assert_eq!(restored_completed.unwrap().status, "completed");
    }
}
