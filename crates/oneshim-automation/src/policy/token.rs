//! Execution ticket generation, parsing, and signature verification.
//!
//! Policy tokens are time-limited tickets that authorize a specific automation
//! command. The TTL is governed by `PolicyCache.ttl_seconds` (default: 300s / 5min).
//! Under high load, a 5-second grace window in `PolicyClient::validate_command()`
//! absorbs clock skew. Tokens are single-use and HMAC-SHA256 signed.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;
use uuid::Uuid;

use crate::controller::AutomationCommand;
use crate::error::AutomationError;

use super::models::ExecutionPolicy;

pub(super) const POLICY_TOKEN_SIGNING_SECRET_ENV: &str = "ONESHIM_POLICY_TOKEN_SIGNING_SECRET";
pub(super) const COMMAND_HASH_SEGMENT_PREFIX: char = 'h';

pub(super) struct ParsedPolicyToken<'a> {
    pub(super) policy_id: &'a str,
    pub(super) nonce: &'a str,
    pub(super) command_hash: Option<&'a str>,
    pub(super) signature: Option<&'a str>,
}

pub(super) fn parse_policy_token(token: &str) -> Option<ParsedPolicyToken<'_>> {
    let parts: Vec<&str> = token.split(':').map(str::trim).collect();
    let (policy_id, nonce, command_hash, signature) = match parts.as_slice() {
        [policy_id, nonce] => (*policy_id, *nonce, None, None),
        [policy_id, nonce, third] => {
            if let Some(command_hash) = parse_command_hash_segment(third) {
                (*policy_id, *nonce, Some(command_hash), None)
            } else {
                (*policy_id, *nonce, None, Some(*third))
            }
        }
        [policy_id, nonce, third, fourth] => {
            let command_hash = parse_command_hash_segment(third)?;
            (*policy_id, *nonce, Some(command_hash), Some(*fourth))
        }
        _ => return None,
    };

    if policy_id.is_empty() || nonce.is_empty() {
        return None;
    }
    if command_hash.is_some_and(|hash| !is_valid_hash(hash)) {
        return None;
    }
    if signature.is_some_and(|sig| sig.is_empty()) {
        return None;
    }

    Some(ParsedPolicyToken {
        policy_id,
        nonce,
        command_hash,
        signature,
    })
}

pub(super) fn is_valid_nonce(nonce: &str) -> bool {
    nonce.len() >= 8
        && nonce
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub(super) fn is_valid_signature(signature: &str) -> bool {
    is_valid_hash(signature)
}

pub(super) fn issue_policy_nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

pub(super) fn issue_command_token_for_policy(
    policy: &ExecutionPolicy,
    nonce: &str,
    command_hash: Option<&str>,
) -> Result<String, AutomationError> {
    if !is_valid_nonce(nonce) {
        return Err(AutomationError::InvalidArguments(
            "policy token nonce 형식이 유효하지 않습니다".to_string(),
        ));
    }
    if command_hash.is_some_and(|hash| !is_valid_hash(hash)) {
        return Err(AutomationError::InvalidArguments(
            "policy token command hash 형식이 유효하지 않습니다".to_string(),
        ));
    }

    let mut token = format!("{}:{nonce}", policy.policy_id);
    if let Some(command_hash) = command_hash {
        token.push(':');
        token.push(COMMAND_HASH_SEGMENT_PREFIX);
        token.push_str(command_hash);
    }

    if policy.require_signed_token {
        let secret = load_signing_secret().ok_or_else(|| {
            // Iter-100: env var empty = Missing semantic, not Invalid.
            // Route via Core wrapper so wire code is config.missing, helping
            // telemetry distinguish "admin forgot to set env var" from
            // "admin set an invalid value".
            AutomationError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
                message: format!(
                    "서명 policy이 active화되어 있지만 {} 환경 변수가 비어 있습니다.",
                    POLICY_TOKEN_SIGNING_SECRET_ENV
                ),
            })
        })?;
        let signature =
            compute_policy_token_signature(&policy.policy_id, nonce, command_hash, &secret);
        token.push(':');
        token.push_str(&signature);
    }

    Ok(token)
}

pub(super) fn parse_command_hash_segment(segment: &str) -> Option<&str> {
    let mut chars = segment.chars();
    if chars.next()? != COMMAND_HASH_SEGMENT_PREFIX {
        return None;
    }
    let hash = chars.as_str();
    if !is_valid_hash(hash) {
        return None;
    }
    Some(hash)
}

pub(super) fn is_valid_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit())
}

pub(super) fn compute_command_scope_hash(
    cmd: &AutomationCommand,
) -> Result<String, AutomationError> {
    #[derive(serde::Serialize)]
    struct PolicyCommandScope<'a> {
        command_id: &'a str,
        session_id: &'a str,
        action: &'a crate::controller::AutomationAction,
        timeout_ms: Option<u64>,
    }

    let scope = PolicyCommandScope {
        command_id: cmd.command_id.as_str(),
        session_id: cmd.session_id.as_str(),
        action: &cmd.action,
        timeout_ms: cmd.timeout_ms,
    };
    let serialized = serde_json::to_vec(&scope).map_err(|e| {
        AutomationError::Internal(format!(
            "Failed to serialize policy token command scope: {e}"
        ))
    })?;
    let digest = Sha256::digest(serialized);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn verify_policy_token_signature(
    policy_id: &str,
    nonce: &str,
    command_hash: Option<&str>,
    signature: &str,
) -> bool {
    let Some(secret) = load_signing_secret() else {
        tracing::warn!(
            env = POLICY_TOKEN_SIGNING_SECRET_ENV,
            "signature policy is enabled but token signing secret is not configured"
        );
        return false;
    };

    // Decode the provided hex signature into bytes for constant-time comparison
    let sig_bytes = match hex_decode(signature) {
        Some(b) => b,
        None => return false,
    };

    // Build HMAC and verify in constant time via hmac::Mac::verify_slice
    let payload = build_signature_payload(policy_id, nonce, command_hash);
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    mac.verify_slice(&sig_bytes).is_ok()
}

/// Decode a lowercase hex string into bytes. Returns `None` on invalid input.
fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

pub(super) fn load_signing_secret() -> Option<String> {
    std::env::var(POLICY_TOKEN_SIGNING_SECRET_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Build the canonical payload string for signing.
fn build_signature_payload(policy_id: &str, nonce: &str, command_hash: Option<&str>) -> String {
    if let Some(command_hash) = command_hash {
        format!("{policy_id}:{nonce}:{command_hash}")
    } else {
        format!("{policy_id}:{nonce}")
    }
}

pub(super) fn compute_policy_token_signature(
    policy_id: &str,
    nonce: &str,
    command_hash: Option<&str>,
    secret: &str,
) -> String {
    let payload = build_signature_payload(policy_id, nonce, command_hash);
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload.as_bytes());
    let result = mac.finalize();
    result
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
