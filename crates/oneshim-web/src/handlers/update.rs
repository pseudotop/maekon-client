use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{extract::State, Json};
use futures::stream::Stream;
use oneshim_api_contracts::update::{UpdateActionRequest, UpdateActionResponse, UpdateStatus};
use std::convert::Infallible;
use std::time::Duration;

use crate::error::ApiError;
use crate::services::update_service::{
    UpdateCommandService, UpdateQueryService, UpdateStreamService,
};
use crate::services::web_contexts::UpdateWebContext;

pub async fn get_update_status(
    State(context): State<UpdateWebContext>,
) -> Result<Json<UpdateStatus>, ApiError> {
    Ok(Json(UpdateQueryService::new(context).get_status().await?))
}

pub async fn post_update_action(
    State(context): State<UpdateWebContext>,
    Json(body): Json<UpdateActionRequest>,
) -> Result<Json<UpdateActionResponse>, ApiError> {
    Ok(Json(
        UpdateCommandService::new(context)
            .post_action(&body)
            .await?,
    ))
}

pub async fn get_update_stream(
    State(context): State<UpdateWebContext>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let stream = UpdateStreamService::new(context).event_stream()?;

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::web_contexts::UpdateWebContext;
    use crate::update_control::{UpdateAction, UpdateControl, UpdatePhase};
    use crate::AppState;
    use oneshim_storage::sqlite::SqliteStorage;
    use std::sync::Arc;
    use tokio::sync::{broadcast, mpsc};

    async fn make_state_with_update_control() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(16);
        let (action_tx, mut action_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move { while action_rx.recv().await.is_some() {} });
        let control = UpdateControl::new(action_tx, UpdateStatus::default());

        let mut state = AppState::with_core(storage, event_tx);
        state.core.update_control = Some(control);
        state
    }

    fn context_from_state(state: &AppState) -> UpdateWebContext {
        UpdateWebContext::from_state(state)
    }

    #[tokio::test]
    async fn get_update_status_returns_snapshot() {
        let state = make_state_with_update_control().await;
        let control = state
            .core
            .update_control
            .as_ref()
            .expect("update control should exist")
            .clone();

        {
            let mut guard = control.state.write().await;
            guard.phase = UpdatePhase::PendingApproval;
            guard.message = Some("pending".to_string());
        }

        let response = get_update_status(State(context_from_state(&state)))
            .await
            .expect("status endpoint should return payload")
            .0;

        assert_eq!(response.phase, UpdatePhase::PendingApproval);
        assert_eq!(response.message.as_deref(), Some("pending"));
    }

    #[tokio::test]
    async fn post_update_action_accepts_request() {
        let state = make_state_with_update_control().await;

        let response = post_update_action(
            State(context_from_state(&state)),
            Json(UpdateActionRequest {
                action: UpdateAction::CheckNow,
            }),
        )
        .await
        .expect("action endpoint should accept request")
        .0;

        assert!(response.accepted);
    }

    #[tokio::test]
    async fn get_update_status_returns_expected_fields() {
        let state = make_state_with_update_control().await;

        let response = get_update_status(State(context_from_state(&state)))
            .await
            .expect("status endpoint should return payload")
            .0;

        // Default UpdateStatus should have Idle phase and no message
        assert_eq!(response.phase, UpdatePhase::Idle);
        assert!(response.message.is_none());
    }
}
