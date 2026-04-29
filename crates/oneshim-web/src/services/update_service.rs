use axum::response::sse::Event;
use futures::stream::Stream;
use oneshim_api_contracts::update::{UpdateActionRequest, UpdateActionResponse, UpdateStatus};
use std::convert::Infallible;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::error::ApiError;
use crate::services::web_contexts::UpdateWebContext;
use crate::update_control::UpdateControl;

#[derive(Clone)]
pub struct UpdateQueryService {
    ctx: UpdateWebContext,
}

impl UpdateQueryService {
    pub fn new(ctx: UpdateWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_status(&self) -> Result<UpdateStatus, ApiError> {
        let control = require_update_control(&self.ctx)?;
        let snapshot = control.state.read().await.clone();
        Ok(snapshot)
    }
}

#[derive(Clone)]
pub struct UpdateCommandService {
    ctx: UpdateWebContext,
}

impl UpdateCommandService {
    pub fn new(ctx: UpdateWebContext) -> Self {
        Self { ctx }
    }

    pub async fn post_action(
        &self,
        request: &UpdateActionRequest,
    ) -> Result<UpdateActionResponse, ApiError> {
        let control = require_update_control(&self.ctx)?;

        control
            .action_tx
            .send(request.action.clone())
            .map_err(|error| {
                ApiError::Internal(format!("Failed to send update action: {error}"))
            })?;

        let status = control.state.read().await.clone();
        Ok(UpdateActionResponse {
            accepted: true,
            status,
        })
    }
}

#[derive(Clone)]
pub struct UpdateStreamService {
    ctx: UpdateWebContext,
}

impl UpdateStreamService {
    pub fn new(ctx: UpdateWebContext) -> Self {
        Self { ctx }
    }

    pub fn event_stream(
        &self,
    ) -> Result<impl Stream<Item = Result<Event, Infallible>> + Send + 'static, ApiError> {
        let control = require_update_control(&self.ctx)?;
        let rx = control.subscribe();

        Ok(BroadcastStream::new(rx).filter_map(|result| match result {
            Ok(status) => {
                let json = serde_json::to_string(&status).ok()?;
                Some(Ok(Event::default().event("update_status").data(json)))
            }
            Err(_) => None,
        }))
    }
}

fn require_update_control(context: &UpdateWebContext) -> Result<UpdateControl, ApiError> {
    context
        .update_control
        .clone()
        .ok_or_else(|| ApiError::NotFound("Update control is not enabled".to_string()))
}
