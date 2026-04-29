//! GUI interaction execution methods — confirm, prepare, complete execution.

use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use oneshim_core::models::gui::{ExecutionBinding, GuiSessionState};

use super::crypto::{hash_actions, sign_ticket, verify_ticket};
use super::helpers::{
    build_actions_for_candidate, is_expired, is_expired_past_grace, map_core_error,
};
use super::types::{
    ConfirmedAction, GuiConfirmRequest, GuiExecutionOutcome, GuiExecutionPlan, GuiExecutionRequest,
    GuiInteractionError,
};
use super::{
    DEFAULT_TICKET_TTL_SECS, FOCUS_DRIFT_MAX_RETRIES, FOCUS_DRIFT_RETRY_DELAY_MS,
    TICKET_EXPIRY_GRACE_SECS,
};
use tracing::debug;

use super::service::GuiInteractionService;

impl GuiInteractionService {
    #[tracing::instrument(skip_all, fields(session_id = %session_id))]
    pub async fn confirm_candidate(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiConfirmRequest,
    ) -> Result<oneshim_core::models::gui::GuiExecutionTicket, GuiInteractionError> {
        let secret = self.require_hmac_secret()?;
        self.assert_capability_token(session_id, capability_token)
            .await?;

        let (session_focus, candidate, scene_id) = {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };

            if is_expired(&stored.session.expires_at) {
                stored.session.state = GuiSessionState::Expired;
                return Err(GuiInteractionError::TicketInvalid {
                    code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                    message: "Session already expired".to_string(),
                });
            }

            let Some(candidate) = stored
                .session
                .candidates
                .iter()
                .find(|candidate| candidate.element.element_id == req.candidate_id)
                .cloned()
            else {
                return Err(GuiInteractionError::BadRequest {
                    code: oneshim_core::error_codes::GuiCode::BadRequest,
                    message: format!("Unknown candidate_id '{}'", req.candidate_id),
                });
            };

            if !candidate.eligible {
                return Err(GuiInteractionError::BadRequest {
                    code: oneshim_core::error_codes::GuiCode::BadRequest,
                    message: "Selected candidate is not eligible".to_string(),
                });
            }

            (
                stored.session.focus.clone(),
                candidate,
                stored.session.scene.scene_id.clone(),
            )
        };

        let binding = ExecutionBinding {
            focus_hash: session_focus.focus_hash.clone(),
            app_name: Some(session_focus.app_name.clone()),
            pid: Some(session_focus.pid),
        };
        let focus_validation = self
            .focus_probe
            .validate_execution_binding(&binding)
            .await
            .map_err(map_core_error)?;
        if !focus_validation.valid {
            return Err(GuiInteractionError::FocusDrift {
                code: oneshim_core::error_codes::GuiCode::FocusDrift,
                message: focus_validation
                    .reason
                    .unwrap_or_else(|| "Focused window changed".to_string()),
            });
        }

        let actions = build_actions_for_candidate(&candidate, &req.action)?;
        let action_hash = hash_actions(&actions)?;
        let now = Utc::now();
        let ticket_ttl_secs = req
            .ticket_ttl_secs
            .map(|value| value as i64)
            .unwrap_or(DEFAULT_TICKET_TTL_SECS)
            .clamp(5, 300);
        let ticket = oneshim_core::models::gui::GuiExecutionTicket {
            schema_version: oneshim_core::models::gui::GUI_TICKET_SCHEMA_VERSION.to_string(),
            ticket_id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            scene_id,
            element_id: candidate.element.element_id.clone(),
            action_hash: action_hash.clone(),
            focus_hash: session_focus.focus_hash,
            issued_at: now,
            expires_at: now + ChronoDuration::seconds(ticket_ttl_secs),
            nonce: Uuid::new_v4().simple().to_string(),
            signature: String::new(),
        };
        let signature = sign_ticket(secret, &ticket)?;
        let mut signed_ticket = ticket;
        signed_ticket.signature = signature;

        {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };
            stored.confirmed_action = Some(ConfirmedAction {
                candidate_id: req.candidate_id.clone(),
                actions,
                action_hash,
                ticket: signed_ticket.clone(),
            });
            stored.session.state = GuiSessionState::Confirmed;
            stored.session.selected_element_id = Some(req.candidate_id.clone());
            stored.session.updated_at = Utc::now();
        }

        self.publish_event(
            session_id.to_string(),
            GuiSessionState::Confirmed,
            "gui_session.confirmed",
            Some(format!("candidate_id={}", req.candidate_id)),
        );

        Ok(signed_ticket)
    }

    #[tracing::instrument(skip_all, fields(session_id = %session_id))]
    pub async fn prepare_execution(
        &self,
        session_id: &str,
        capability_token: &str,
        req: GuiExecutionRequest,
    ) -> Result<GuiExecutionPlan, GuiInteractionError> {
        let secret = self.require_hmac_secret()?;
        self.assert_capability_token(session_id, capability_token)
            .await?;

        let (session_focus, confirmed_action, session_state, expires_at) = {
            let sessions = self.sessions.read().await;
            let Some(stored) = sessions.get(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };

            let Some(confirmed_action) = stored.confirmed_action.clone() else {
                return Err(GuiInteractionError::TicketInvalid {
                    code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                    message: "Session has no confirmed action".to_string(),
                });
            };

            (
                stored.session.focus.clone(),
                confirmed_action,
                stored.session.state,
                stored.session.expires_at,
            )
        };

        if is_expired(&expires_at) {
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: "Session already expired".to_string(),
            });
        }

        if session_state != GuiSessionState::Confirmed {
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: format!(
                    "Session state must be confirmed, current={:?}",
                    session_state
                ),
            });
        }

        verify_ticket(secret, &req.ticket)?;

        if req.ticket.session_id != session_id {
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: "ticket.session_id mismatch".to_string(),
            });
        }
        if req.ticket.ticket_id != confirmed_action.ticket.ticket_id {
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: "ticket_id mismatch".to_string(),
            });
        }
        if req.ticket.element_id != confirmed_action.candidate_id {
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: "element_id mismatch".to_string(),
            });
        }
        if req.ticket.action_hash != confirmed_action.action_hash {
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: "action_hash mismatch".to_string(),
            });
        }
        if is_expired_past_grace(&req.ticket.expires_at, TICKET_EXPIRY_GRACE_SECS) {
            tracing::warn!(
                session_id,
                ticket_id = %req.ticket.ticket_id,
                "GUI ticket expired past grace period"
            );
            return Err(GuiInteractionError::TicketInvalid {
                code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                message: "ticket expired".to_string(),
            });
        }
        if is_expired(&req.ticket.expires_at) {
            tracing::debug!(
                session_id,
                ticket_id = %req.ticket.ticket_id,
                grace_secs = TICKET_EXPIRY_GRACE_SECS,
                "GUI ticket nominally expired but within grace window"
            );
        }

        let binding = ExecutionBinding {
            focus_hash: session_focus.focus_hash,
            app_name: Some(session_focus.app_name),
            pid: Some(session_focus.pid),
        };

        let mut last_drift_reason = String::new();
        let mut focus_valid = false;
        for attempt in 0..=FOCUS_DRIFT_MAX_RETRIES {
            let focus_validation = self
                .focus_probe
                .validate_execution_binding(&binding)
                .await
                .map_err(map_core_error)?;
            if focus_validation.valid {
                focus_valid = true;
                break;
            }
            last_drift_reason = focus_validation
                .reason
                .unwrap_or_else(|| "Focused window changed".to_string());
            if attempt < FOCUS_DRIFT_MAX_RETRIES {
                tracing::debug!(
                    session_id,
                    attempt = attempt + 1,
                    max_retries = FOCUS_DRIFT_MAX_RETRIES,
                    reason = %last_drift_reason,
                    "Focus drift detected, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(FOCUS_DRIFT_RETRY_DELAY_MS))
                    .await;
            }
        }
        if !focus_valid {
            tracing::warn!(
                session_id,
                retries_exhausted = FOCUS_DRIFT_MAX_RETRIES,
                reason = %last_drift_reason,
                "Focus drift — all retries exhausted"
            );
            return Err(GuiInteractionError::FocusDrift {
                code: oneshim_core::error_codes::GuiCode::FocusDrift,
                message: last_drift_reason,
            });
        }

        {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };
            if stored.used_ticket_nonces.contains(&req.ticket.nonce) {
                return Err(GuiInteractionError::TicketInvalid {
                    code: oneshim_core::error_codes::GuiCode::TicketInvalid,
                    message: "ticket nonce replay detected".to_string(),
                });
            }
            stored.used_ticket_nonces.insert(req.ticket.nonce.clone());
            stored.session.state = GuiSessionState::Executing;
            stored.session.updated_at = Utc::now();
        }

        self.publish_event(
            session_id.to_string(),
            GuiSessionState::Executing,
            "gui_session.executing",
            Some(format!("ticket_id={}", req.ticket.ticket_id)),
        );

        let plan = GuiExecutionPlan {
            session_id: session_id.to_string(),
            command_id: format!("gui-action-{}", Utc::now().timestamp_millis().abs()),
            actions: confirmed_action.actions,
            ticket: req.ticket,
        };

        tracing::info!(
            session_id,
            command_id = %plan.command_id,
            action_count = plan.actions.len(),
            "GUI execution prepared"
        );

        Ok(plan)
    }

    pub async fn complete_execution(
        &self,
        session_id: &str,
        succeeded: bool,
        detail: Option<String>,
        steps_completed: usize,
        total_steps: usize,
    ) -> Result<GuiExecutionOutcome, GuiInteractionError> {
        let (updated_session, overlay_handle_id) = {
            let mut sessions = self.sessions.write().await;
            let Some(stored) = sessions.get_mut(session_id) else {
                return Err(GuiInteractionError::NotFound {
                    code: oneshim_core::error_codes::GuiCode::NotFound,
                    name: session_id.to_string(),
                });
            };

            if succeeded {
                stored.session.state = GuiSessionState::Executed;
            } else {
                stored.session.state = GuiSessionState::Confirmed;
            }
            stored.session.updated_at = Utc::now();

            (stored.session.clone(), stored.overlay_handle_id.take())
        };

        if let Some(handle_id) = overlay_handle_id {
            if let Err(e) = self.overlay_driver.clear_highlights(&handle_id).await {
                debug!("clear_highlights failed: {e}");
            }
        }

        self.publish_event(
            session_id.to_string(),
            updated_session.state,
            if succeeded {
                "gui_session.executed"
            } else {
                "gui_session.execution_failed"
            },
            detail.clone(),
        );

        if succeeded {
            tracing::info!(
                session_id,
                steps_completed,
                total_steps,
                "GUI execution completed successfully"
            );
        } else {
            tracing::warn!(
                session_id,
                steps_completed,
                total_steps,
                detail = detail.as_deref().unwrap_or("unknown"),
                "GUI execution failed"
            );
        }

        Ok(GuiExecutionOutcome {
            session: updated_session,
            succeeded,
            detail,
            steps_completed,
            total_steps,
        })
    }
}
