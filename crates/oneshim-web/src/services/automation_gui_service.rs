use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use oneshim_api_contracts::automation_gui::{
    GuiConfirmRequest, GuiConfirmResponse, GuiCreateSessionRequest, GuiCreateSessionResponse,
    GuiExecuteResponse, GuiExecutionRequest, GuiHighlightRequest, GuiSessionResponse,
};
use oneshim_core::error::GuiInteractionError;
use oneshim_core::ports::automation::AutomationPort;

use super::automation_gui_assembler::{
    assemble_confirm_response, assemble_create_session_response, assemble_execute_response,
    assemble_session_response, map_confirm_request, map_create_session_request,
    map_execution_request, map_highlight_request,
};
use crate::error::ApiError;
use crate::services::web_contexts::AutomationGuiWebContext;

pub const GUI_SESSION_HEADER: &str = "x-gui-session-token";
pub const GUI_SCHEMA_VERSION: &str = "automation.gui.v2";

#[derive(Clone)]
pub struct AutomationGuiQueryService {
    ctx: AutomationGuiWebContext,
}

impl AutomationGuiQueryService {
    pub fn new(ctx: AutomationGuiWebContext) -> Self {
        Self { ctx }
    }

    pub async fn get_gui_session(
        &self,
        session_id: &str,
        headers: &HeaderMap,
    ) -> Result<GuiSessionResponse, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let capability_token = read_capability_token(headers)?;
        let session = controller
            .gui_get_session(session_id, &capability_token)
            .await
            .map_err(map_gui_error)?;

        Ok(assemble_session_response(GUI_SCHEMA_VERSION, session))
    }
}

#[derive(Clone)]
pub struct AutomationGuiCommandService {
    ctx: AutomationGuiWebContext,
}

impl AutomationGuiCommandService {
    pub fn new(ctx: AutomationGuiWebContext) -> Self {
        Self { ctx }
    }

    pub async fn create_gui_session(
        &self,
        request: GuiCreateSessionRequest,
    ) -> Result<GuiCreateSessionResponse, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let created = controller
            .gui_create_session(map_create_session_request(request))
            .await
            .map_err(map_gui_error)?;

        Ok(assemble_create_session_response(
            GUI_SCHEMA_VERSION,
            created.session,
            created.capability_token,
        ))
    }

    pub async fn highlight_gui_session(
        &self,
        session_id: &str,
        headers: &HeaderMap,
        request: GuiHighlightRequest,
    ) -> Result<GuiSessionResponse, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let capability_token = read_capability_token(headers)?;
        let session = controller
            .gui_highlight_session(
                session_id,
                &capability_token,
                map_highlight_request(request),
            )
            .await
            .map_err(map_gui_error)?;

        Ok(assemble_session_response(GUI_SCHEMA_VERSION, session))
    }

    pub async fn confirm_gui_session(
        &self,
        session_id: &str,
        headers: &HeaderMap,
        request: GuiConfirmRequest,
    ) -> Result<GuiConfirmResponse, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let capability_token = read_capability_token(headers)?;
        let ticket = controller
            .gui_confirm_candidate(session_id, &capability_token, map_confirm_request(request))
            .await
            .map_err(map_gui_error)?;

        Ok(assemble_confirm_response(GUI_SCHEMA_VERSION, ticket))
    }

    pub async fn execute_gui_session(
        &self,
        session_id: &str,
        headers: &HeaderMap,
        request: GuiExecutionRequest,
    ) -> Result<GuiExecuteResponse, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let capability_token = read_capability_token(headers)?;
        let result = controller
            .gui_execute(
                session_id,
                &capability_token,
                map_execution_request(request),
            )
            .await
            .map_err(map_gui_error)?;

        Ok(assemble_execute_response(GUI_SCHEMA_VERSION, result))
    }

    pub async fn delete_gui_session(
        &self,
        session_id: &str,
        headers: &HeaderMap,
    ) -> Result<GuiSessionResponse, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let capability_token = read_capability_token(headers)?;
        let session = controller
            .gui_cancel_session(session_id, &capability_token)
            .await
            .map_err(map_gui_error)?;

        Ok(assemble_session_response(GUI_SCHEMA_VERSION, session))
    }
}

#[derive(Clone)]
pub struct AutomationGuiStreamService {
    ctx: AutomationGuiWebContext,
}

impl AutomationGuiStreamService {
    pub fn new(ctx: AutomationGuiWebContext) -> Self {
        Self { ctx }
    }

    pub async fn gui_session_event_stream(
        &self,
        session_id: &str,
        headers: &HeaderMap,
    ) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
        let controller = require_controller(&self.ctx)?;
        let capability_token = read_capability_token(headers)?;
        let rx = controller
            .gui_subscribe_events(session_id, &capability_token)
            .await
            .map_err(map_gui_error)?;

        let session_id = session_id.to_string();
        let sse_stream = BroadcastStream::new(rx).filter_map(move |result| match result {
            Ok(event) if event.session_id == session_id => {
                let data = serde_json::to_string(&event).ok()?;
                Some(Ok(Event::default()
                    .event(event.event_type.clone())
                    .data(data)))
            }
            _ => None,
        });

        Ok(Sse::new(sse_stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("ping"),
        ))
    }
}

fn require_controller(context: &AutomationGuiWebContext) -> Result<&dyn AutomationPort, ApiError> {
    context.automation_controller.as_deref().ok_or_else(|| {
        ApiError::ServiceUnavailable("Automation controller is disabled".to_string())
    })
}

pub(crate) fn read_capability_token(headers: &HeaderMap) -> Result<String, ApiError> {
    headers
        .get(GUI_SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            ApiError::Unauthorized(format!(
                "Missing required header '{}': session capability token",
                GUI_SESSION_HEADER
            ))
        })
}

pub(crate) fn map_gui_error(error: GuiInteractionError) -> ApiError {
    match error {
        GuiInteractionError::Unauthorized { .. } => {
            ApiError::Unauthorized("Invalid GUI session token".to_string())
        }
        GuiInteractionError::NotFound { name, .. } => ApiError::NotFound(name),
        GuiInteractionError::BadRequest { message, .. } => ApiError::BadRequest(message),
        GuiInteractionError::Forbidden { message, .. } => ApiError::Forbidden(message),
        GuiInteractionError::FocusDrift { message, .. } => ApiError::Conflict(message),
        GuiInteractionError::TicketInvalid { message, .. } => ApiError::Unprocessable(message),
        GuiInteractionError::Unavailable { message, .. } => ApiError::ServiceUnavailable(message),
        GuiInteractionError::Internal { message, .. } => ApiError::Internal(message),
    }
}
