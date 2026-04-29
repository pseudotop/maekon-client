use oneshim_api_contracts::automation_gui::{
    GuiConfirmRequest, GuiConfirmResponse, GuiCreateSessionRequest, GuiCreateSessionResponse,
    GuiExecuteResponse, GuiExecutionOutcome, GuiExecutionRequest, GuiHighlightRequest,
    GuiSessionResponse,
};
use oneshim_core::models::automation::GuiExecutionResult;
use oneshim_core::models::gui::{
    GuiConfirmRequest as AutomationGuiConfirmRequest,
    GuiCreateSessionRequest as AutomationGuiCreateSessionRequest,
    GuiExecutionRequest as AutomationGuiExecutionRequest,
    GuiHighlightRequest as AutomationGuiHighlightRequest, GuiInteractionSession,
};

pub(crate) fn map_create_session_request(
    request: GuiCreateSessionRequest,
) -> AutomationGuiCreateSessionRequest {
    AutomationGuiCreateSessionRequest {
        app_name: request.app_name,
        screen_id: request.screen_id,
        min_confidence: request.min_confidence,
        max_candidates: request.max_candidates,
        session_ttl_secs: request.session_ttl_secs,
    }
}

pub(crate) fn map_highlight_request(request: GuiHighlightRequest) -> AutomationGuiHighlightRequest {
    AutomationGuiHighlightRequest {
        candidate_ids: request.candidate_ids,
    }
}

pub(crate) fn map_confirm_request(request: GuiConfirmRequest) -> AutomationGuiConfirmRequest {
    AutomationGuiConfirmRequest {
        candidate_id: request.candidate_id,
        action: request.action,
        ticket_ttl_secs: request.ticket_ttl_secs,
    }
}

pub(crate) fn map_execution_request(request: GuiExecutionRequest) -> AutomationGuiExecutionRequest {
    AutomationGuiExecutionRequest {
        ticket: request.ticket,
    }
}

pub(crate) fn assemble_create_session_response(
    schema_version: &str,
    session: GuiInteractionSession,
    capability_token: String,
) -> GuiCreateSessionResponse {
    GuiCreateSessionResponse {
        schema_version: schema_version.to_string(),
        session,
        capability_token,
    }
}

pub(crate) fn assemble_session_response(
    schema_version: &str,
    session: GuiInteractionSession,
) -> GuiSessionResponse {
    GuiSessionResponse {
        schema_version: schema_version.to_string(),
        session,
    }
}

pub(crate) fn assemble_confirm_response(
    schema_version: &str,
    ticket: oneshim_core::models::gui::GuiExecutionTicket,
) -> GuiConfirmResponse {
    GuiConfirmResponse {
        schema_version: schema_version.to_string(),
        ticket,
    }
}

pub(crate) fn assemble_execute_response(
    schema_version: &str,
    result: GuiExecutionResult,
) -> GuiExecuteResponse {
    GuiExecuteResponse {
        schema_version: schema_version.to_string(),
        command_id: result.command_id,
        ticket: result.ticket,
        result: result.result,
        outcome: GuiExecutionOutcome {
            session: result.outcome.session,
            succeeded: result.outcome.succeeded,
            detail: result.outcome.detail,
            steps_completed: result.outcome.steps_completed,
            total_steps: result.outcome.total_steps,
        },
    }
}
