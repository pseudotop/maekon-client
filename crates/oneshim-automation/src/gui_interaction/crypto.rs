use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use oneshim_core::models::gui::GuiExecutionTicket;

use super::types::GuiInteractionError;
use crate::controller::AutomationAction;
use tracing::debug;

pub(super) type HmacSha256 = Hmac<Sha256>;

#[tracing::instrument(skip_all)]
pub(super) fn sign_ticket(
    secret: &[u8],
    ticket: &GuiExecutionTicket,
) -> Result<String, GuiInteractionError> {
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| GuiInteractionError::Internal {
            code: oneshim_core::error_codes::GuiCode::InternalError,
            message: format!("hmac key init failed: {e}"),
        })?;
    mac.update(ticket_signature_payload(ticket).as_bytes());
    let signature = mac.finalize().into_bytes();
    Ok(encode_hex(signature.as_slice()))
}

#[tracing::instrument(skip_all)]
pub(super) fn verify_ticket(
    secret: &[u8],
    ticket: &GuiExecutionTicket,
) -> Result<(), GuiInteractionError> {
    let signature_bytes =
        decode_hex(&ticket.signature).ok_or_else(|| GuiInteractionError::TicketInvalid {
            code: oneshim_core::error_codes::GuiCode::TicketInvalid,
            message: "ticket signature format is invalid".to_string(),
        })?;

    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|e| GuiInteractionError::Internal {
            code: oneshim_core::error_codes::GuiCode::InternalError,
            message: format!("hmac key init failed: {e}"),
        })?;
    mac.update(ticket_signature_payload(ticket).as_bytes());

    mac.verify_slice(&signature_bytes)
        .map_err(|_| GuiInteractionError::TicketInvalid {
            code: oneshim_core::error_codes::GuiCode::TicketInvalid,
            message: "ticket signature mismatch".to_string(),
        })
}

pub(super) fn ticket_signature_payload(ticket: &GuiExecutionTicket) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}",
        ticket.session_id,
        ticket.scene_id,
        ticket.element_id,
        ticket.action_hash,
        ticket.focus_hash,
        ticket.issued_at.timestamp_millis(),
        ticket.expires_at.timestamp_millis(),
        ticket.nonce,
    )
}

pub(super) fn new_capability_token() -> String {
    let random = Uuid::new_v4().to_string();
    let digest = Sha256::digest(random.as_bytes());
    encode_hex(digest.as_slice())
}

pub(super) fn hash_actions(actions: &[AutomationAction]) -> Result<String, GuiInteractionError> {
    let payload = serde_json::to_vec(actions).map_err(|e| GuiInteractionError::Internal {
        code: oneshim_core::error_codes::GuiCode::InternalError,
        message: format!("action hash serialization failed: {e}"),
    })?;
    let digest = Sha256::digest(payload);
    Ok(encode_hex(digest.as_slice()))
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        if let Err(e) = write!(&mut out, "{byte:02x}") {
            debug!("operation failed: {e}");
        }
    }
    out
}

pub(super) fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }

    (0..input.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&input[i..i + 2], 16).ok())
        .collect()
}
