use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use oneshim_core::models::gui::{GuiExecutionTicket, GuiInteractionSession};

use crate::controller::AutomationAction;

// Canonical types from oneshim-core — re-exported for backward compat
pub use oneshim_core::error::GuiInteractionError;
pub use oneshim_core::models::gui::{
    GuiConfirmRequest, GuiCreateSessionRequest, GuiCreateSessionResponse, GuiExecutionOutcome,
    GuiExecutionRequest, GuiHighlightRequest,
};

/// 실행 계획 (내부용) — 자동화 실행 준비 데이터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionPlan {
    pub session_id: String,
    pub command_id: String,
    pub actions: Vec<AutomationAction>,
    pub ticket: GuiExecutionTicket,
}

#[derive(Debug, Clone)]
pub(super) struct ConfirmedAction {
    pub(super) candidate_id: String,
    pub(super) actions: Vec<AutomationAction>,
    pub(super) action_hash: String,
    pub(super) ticket: GuiExecutionTicket,
}

#[derive(Debug, Clone)]
pub(super) struct StoredSession {
    pub(super) session: GuiInteractionSession,
    pub(super) capability_token: String,
    pub(super) overlay_handle_id: Option<String>,
    pub(super) confirmed_action: Option<ConfirmedAction>,
    pub(super) used_ticket_nonces: HashSet<String>,
}
