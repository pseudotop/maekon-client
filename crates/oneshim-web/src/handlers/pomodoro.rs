//! Pomodoro focus timer REST handlers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use oneshim_api_contracts::pomodoro::{PomodoroSessionResponse, StartPomodoroRequest};
use oneshim_core::models::pomodoro::{PomodoroSession, PomodoroStatus};
use tracing::debug;
use uuid::Uuid;

use crate::error::ApiError;
use crate::AppState;

/// Default work duration in minutes.
const DEFAULT_DURATION_MINUTES: u32 = 25;
/// Default break duration in minutes.
const DEFAULT_BREAK_MINUTES: u32 = 5;
/// Maximum allowed duration in minutes (2 hours).
const MAX_DURATION_MINUTES: u32 = 120;

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

    // Scope the mutex guard so it drops before we hand the response to axum.
    // P2 PR-A: avoids `clippy::significant_drop_tightening` flag; guard is
    // only needed during state mutation, not response construction.
    let response = {
        let mut guard = state
            .session
            .pomodoro
            .lock()
            .map_err(|_| ApiError::Internal("pomodoro lock poisoned".into()))?;

        // Reject if a session is already active
        if let Some(existing) = guard.as_ref() {
            let eff = existing.effective_status();
            if eff == PomodoroStatus::Running || eff == PomodoroStatus::OnBreak {
                return Err(ApiError::Conflict(
                    "A Pomodoro session is already active".to_string(),
                ));
            }
        }

        let session = PomodoroSession::new(Uuid::new_v4().to_string(), duration, break_mins);
        let response = session_to_response(&session);
        *guard = Some(session);
        response
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /api/pomodoro/current — get current session status + remaining time.
pub async fn get_current_pomodoro(
    State(state): State<AppState>,
) -> Result<Json<Option<PomodoroSessionResponse>>, ApiError> {
    debug!("GET /api/pomodoro/current");

    // P2 PR-A: scope the guard so it drops before Json construction.
    let response = {
        let guard = state
            .session
            .pomodoro
            .lock()
            .map_err(|_| ApiError::Internal("pomodoro lock poisoned".into()))?;
        guard.as_ref().map(session_to_response)
    };
    Ok(Json(response))
}

/// POST /api/pomodoro/cancel — cancel the current session.
///
/// `#[allow(clippy::significant_drop_tightening)]`: the `session: &mut
/// PomodoroSession` borrow is derived from the guard; Rust's borrow checker
/// does not permit dropping the guard before the `&mut` borrow ends. The
/// block-scoped clone-out pattern below already tightens the lock window
/// to the minimum possible given the borrow constraint — clippy's suggested
/// `merge-with-single-usage` rewrite produces invalid Rust (`?.` syntax).
#[allow(clippy::significant_drop_tightening)]
pub async fn cancel_pomodoro(
    State(state): State<AppState>,
) -> Result<Json<PomodoroSessionResponse>, ApiError> {
    debug!("POST /api/pomodoro/cancel");

    // P2 PR-A: mutate under the guard, clone the mutated session out, drop
    // the guard, then format the response. This pattern tightens the lock
    // window to only the mutation path.
    let session = {
        let mut guard = state
            .session
            .pomodoro
            .lock()
            .map_err(|_| ApiError::Internal("pomodoro lock poisoned".into()))?;
        let session = guard
            .as_mut()
            .ok_or_else(|| ApiError::NotFound("No active Pomodoro session".to_string()))?;

        let eff = session.effective_status();
        if eff == PomodoroStatus::Completed || eff == PomodoroStatus::Cancelled {
            return Err(ApiError::Conflict(
                "Session is already finished".to_string(),
            ));
        }

        session.status = PomodoroStatus::Cancelled;
        session.completed_at = Some(chrono::Utc::now());

        session.clone()
    };
    Ok(Json(session_to_response(&session)))
}

/// POST /api/pomodoro/complete — mark session as completed (auto or manual).
///
/// `#[allow(clippy::significant_drop_tightening)]`: same rationale as
/// `cancel_pomodoro` — `session: &mut` borrows from the guard and the
/// block-scoped clone-out is already the tightest lock window possible.
#[allow(clippy::significant_drop_tightening)]
pub async fn complete_pomodoro(
    State(state): State<AppState>,
) -> Result<Json<PomodoroSessionResponse>, ApiError> {
    debug!("POST /api/pomodoro/complete");

    // P2 PR-A: mutate under the guard, clone the mutated session out, drop
    // the guard, then format the response. This pattern tightens the lock
    // window to only the mutation path.
    let session = {
        let mut guard = state
            .session
            .pomodoro
            .lock()
            .map_err(|_| ApiError::Internal("pomodoro lock poisoned".into()))?;
        let session = guard
            .as_mut()
            .ok_or_else(|| ApiError::NotFound("No active Pomodoro session".to_string()))?;

        if session.status == PomodoroStatus::Cancelled {
            return Err(ApiError::Conflict(
                "Session was already cancelled".to_string(),
            ));
        }

        session.status = PomodoroStatus::Completed;
        session.completed_at = Some(chrono::Utc::now());

        session.clone()
    };
    Ok(Json(session_to_response(&session)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

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
}
