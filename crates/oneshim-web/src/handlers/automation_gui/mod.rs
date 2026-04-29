use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, Sse};
use axum::Json;
use futures::stream::Stream;
use oneshim_api_contracts::automation_gui::{
    GuiConfirmRequest, GuiConfirmResponse, GuiCreateSessionRequest, GuiCreateSessionResponse,
    GuiExecuteResponse, GuiExecutionRequest, GuiHighlightRequest, GuiSessionPath,
    GuiSessionResponse,
};
use std::convert::Infallible;

use crate::error::ApiError;
#[cfg(test)]
use crate::services::automation_gui_service::{
    map_gui_error, read_capability_token, GUI_SCHEMA_VERSION, GUI_SESSION_HEADER,
};
use crate::services::automation_gui_service::{
    AutomationGuiCommandService, AutomationGuiQueryService, AutomationGuiStreamService,
};
use crate::services::web_contexts::AutomationGuiWebContext;
#[cfg(test)]
use oneshim_core::error::GuiInteractionError;

pub async fn create_gui_session(
    State(context): State<AutomationGuiWebContext>,
    Json(req): Json<GuiCreateSessionRequest>,
) -> Result<Json<GuiCreateSessionResponse>, ApiError> {
    Ok(Json(
        AutomationGuiCommandService::new(context)
            .create_gui_session(req)
            .await?,
    ))
}

pub async fn get_gui_session(
    State(context): State<AutomationGuiWebContext>,
    Path(path): Path<GuiSessionPath>,
    headers: HeaderMap,
) -> Result<Json<GuiSessionResponse>, ApiError> {
    Ok(Json(
        AutomationGuiQueryService::new(context)
            .get_gui_session(&path.id, &headers)
            .await?,
    ))
}

pub async fn highlight_gui_session(
    State(context): State<AutomationGuiWebContext>,
    Path(path): Path<GuiSessionPath>,
    headers: HeaderMap,
    Json(req): Json<GuiHighlightRequest>,
) -> Result<Json<GuiSessionResponse>, ApiError> {
    Ok(Json(
        AutomationGuiCommandService::new(context)
            .highlight_gui_session(&path.id, &headers, req)
            .await?,
    ))
}

pub async fn confirm_gui_session(
    State(context): State<AutomationGuiWebContext>,
    Path(path): Path<GuiSessionPath>,
    headers: HeaderMap,
    Json(req): Json<GuiConfirmRequest>,
) -> Result<Json<GuiConfirmResponse>, ApiError> {
    Ok(Json(
        AutomationGuiCommandService::new(context)
            .confirm_gui_session(&path.id, &headers, req)
            .await?,
    ))
}

pub async fn execute_gui_session(
    State(context): State<AutomationGuiWebContext>,
    Path(path): Path<GuiSessionPath>,
    headers: HeaderMap,
    Json(req): Json<GuiExecutionRequest>,
) -> Result<Json<GuiExecuteResponse>, ApiError> {
    Ok(Json(
        AutomationGuiCommandService::new(context)
            .execute_gui_session(&path.id, &headers, req)
            .await?,
    ))
}

pub async fn delete_gui_session(
    State(context): State<AutomationGuiWebContext>,
    Path(path): Path<GuiSessionPath>,
    headers: HeaderMap,
) -> Result<Json<GuiSessionResponse>, ApiError> {
    Ok(Json(
        AutomationGuiCommandService::new(context)
            .delete_gui_session(&path.id, &headers)
            .await?,
    ))
}

pub async fn gui_session_event_stream(
    State(context): State<AutomationGuiWebContext>,
    Path(path): Path<GuiSessionPath>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    AutomationGuiStreamService::new(context)
        .gui_session_event_stream(&path.id, &headers)
        .await
}

#[cfg(test)]
mod tests_m4;
#[cfg(test)]
mod tests_m5;
#[cfg(test)]
mod tests_unit;
