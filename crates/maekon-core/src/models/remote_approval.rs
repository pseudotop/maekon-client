use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const REMOTE_OBSERVE_APPROVE_SCHEMA_VERSION: &str = "automation.remote.observe_approve.v1";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCompanionMode {
    #[default]
    ObserveApprove,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteCompanionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub mode: RemoteCompanionMode,
    #[serde(default = "default_session_ttl_secs")]
    pub session_ttl_secs: u64,
}

impl RemoteCompanionConfig {
    pub fn allows_remote_control(&self) -> bool {
        false
    }
}

impl Default for RemoteCompanionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: RemoteCompanionMode::ObserveApprove,
            session_ttl_secs: default_session_ttl_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteReadinessSummary {
    pub readiness_state: String,
    pub connected_mode_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePendingApprovalSummary {
    pub request_id: String,
    pub matched_policy_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub wants_network: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteObserveSnapshot {
    pub schema_version: String,
    pub mode: RemoteCompanionMode,
    pub readiness: RemoteReadinessSummary,
    pub pending_approvals: Vec<RemotePendingApprovalSummary>,
    pub masked_timeline_summary: Vec<String>,
    pub audit_excerpt_codes: Vec<String>,
    pub automation_trend_codes: Vec<String>,
    pub privacy_consent_summary: Vec<String>,
}

impl RemoteObserveSnapshot {
    pub fn new(
        readiness: RemoteReadinessSummary,
        pending_approvals: Vec<RemotePendingApprovalSummary>,
    ) -> Self {
        Self {
            schema_version: REMOTE_OBSERVE_APPROVE_SCHEMA_VERSION.to_string(),
            mode: RemoteCompanionMode::ObserveApprove,
            readiness,
            pending_approvals,
            masked_timeline_summary: Vec::new(),
            audit_excerpt_codes: Vec::new(),
            automation_trend_codes: Vec::new(),
            privacy_consent_summary: Vec::new(),
        }
    }

    pub fn allows_remote_control(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteApprovalToken {
    pub request_id: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce_hash: String,
    pub device_binding_hash: String,
    #[serde(default)]
    used: bool,
}

impl RemoteApprovalToken {
    pub fn new_hashed(
        request_id: impl Into<String>,
        issued_at: DateTime<Utc>,
        ttl_secs: i64,
        nonce_hash: impl Into<String>,
        device_binding_hash: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            issued_at,
            expires_at: issued_at + Duration::seconds(ttl_secs),
            nonce_hash: nonce_hash.into(),
            device_binding_hash: device_binding_hash.into(),
            used: false,
        }
    }

    pub fn validate_for_request(&self, request_id: &str, now: DateTime<Utc>) -> Result<(), String> {
        if self.request_id != request_id {
            return Err("request_id_mismatch".to_string());
        }
        if self.used {
            return Err("replay_detected".to_string());
        }
        if now >= self.expires_at {
            return Err("token_expired".to_string());
        }
        Ok(())
    }

    pub fn mark_used(&mut self) {
        self.used = true;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteApprovalRequest {
    pub request_id: String,
    pub matched_policy_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub wants_network: bool,
    pub local_policy_allows_network: bool,
}

impl RemoteApprovalRequest {
    pub fn remote_decision_can_grant_network(&self, decision: &RemoteApprovalDecision) -> bool {
        decision.decision == RemoteApprovalDisposition::Approve
            && decision.request_id == self.request_id
            && decision.decided_at < self.expires_at
            && self.wants_network
            && self.local_policy_allows_network
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteApprovalDisposition {
    Approve,
    Decline,
    Cancel,
    Pause,
    RequestLocalDiagnosticExport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteApprovalDecision {
    pub request_id: String,
    pub decision: RemoteApprovalDisposition,
    pub remote_origin: String,
    pub decided_at: DateTime<Utc>,
}

impl RemoteApprovalDecision {
    pub fn approve(request_id: impl Into<String>, remote_origin: impl Into<String>) -> Self {
        Self::new(
            request_id,
            remote_origin,
            RemoteApprovalDisposition::Approve,
        )
    }

    pub fn decline(request_id: impl Into<String>, remote_origin: impl Into<String>) -> Self {
        Self::new(
            request_id,
            remote_origin,
            RemoteApprovalDisposition::Decline,
        )
    }

    fn new(
        request_id: impl Into<String>,
        remote_origin: impl Into<String>,
        decision: RemoteApprovalDisposition,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            decision,
            remote_origin: remote_origin.into(),
            decided_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteApprovalAuditEntry {
    pub schema_version: String,
    pub request_id: String,
    pub remote_origin: String,
    pub decision: RemoteApprovalDisposition,
    pub expires_at: DateTime<Utc>,
    pub matched_policy_id: Option<String>,
    pub wants_network: bool,
    pub local_policy_allows_network: bool,
}

impl RemoteApprovalAuditEntry {
    pub fn from_decision(
        request: &RemoteApprovalRequest,
        decision: &RemoteApprovalDecision,
    ) -> Self {
        Self {
            schema_version: REMOTE_OBSERVE_APPROVE_SCHEMA_VERSION.to_string(),
            request_id: request.request_id.clone(),
            remote_origin: sanitize_remote_origin(&decision.remote_origin),
            decision: decision.decision,
            expires_at: request.expires_at,
            matched_policy_id: request.matched_policy_id.clone(),
            wants_network: request.wants_network,
            local_policy_allows_network: request.local_policy_allows_network,
        }
    }
}

fn default_session_ttl_secs() -> u64 {
    300
}

fn sanitize_remote_origin(origin: &str) -> String {
    let without_fragment = origin.split('#').next().unwrap_or(origin);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let without_userinfo = if let Some((scheme, rest)) = without_query.split_once("://") {
        let authority_end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let suffix = &rest[authority_end..];
        authority
            .rsplit_once('@')
            .map(|(_, host)| format!("{scheme}://{host}{suffix}"))
            .unwrap_or_else(|| without_query.to_string())
    } else {
        without_query.to_string()
    };

    without_userinfo
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};

    #[test]
    fn observe_snapshot_is_not_remote_control_and_omits_raw_content() {
        let snapshot = RemoteObserveSnapshot::new(
            RemoteReadinessSummary {
                readiness_state: "available".to_string(),
                connected_mode_enabled: true,
            },
            vec![RemotePendingApprovalSummary {
                request_id: "req-1".to_string(),
                matched_policy_id: Some("policy-safe".to_string()),
                expires_at: Utc::now() + Duration::seconds(30),
                wants_network: false,
            }],
        );

        assert_eq!(snapshot.mode, RemoteCompanionMode::ObserveApprove);
        assert!(!snapshot.allows_remote_control());
        let serialized = serde_json::to_string(&snapshot).expect("must serialize");
        assert!(!serialized.contains("rawWindowTitle"));
        assert!(!serialized.contains("rawLabel"));
        assert!(!serialized.contains("broad_screenshot"));
    }

    #[test]
    fn remote_approval_token_expires_and_is_replay_safe() {
        let issued_at = Utc::now();
        let mut token = RemoteApprovalToken::new_hashed(
            "req-1",
            issued_at,
            30,
            "nonce-hash",
            "device-binding-hash",
        );

        token
            .validate_for_request("req-1", issued_at + Duration::seconds(5))
            .expect("fresh request-bound token should validate before expiry");
        token.mark_used();
        assert!(token
            .validate_for_request("req-1", issued_at + Duration::seconds(6))
            .expect_err("used token must be rejected")
            .contains("replay"));

        let expired = RemoteApprovalToken::new_hashed(
            "req-2",
            issued_at,
            1,
            "nonce-hash-2",
            "device-binding-hash",
        );
        assert!(expired
            .validate_for_request("req-2", issued_at + Duration::seconds(2))
            .expect_err("expired token must be rejected")
            .contains("expired"));
    }

    #[test]
    fn remote_approval_cannot_grant_network_without_local_policy() {
        let request = RemoteApprovalRequest {
            request_id: "req-network".to_string(),
            matched_policy_id: Some("policy-confirm".to_string()),
            expires_at: Utc::now() + Duration::seconds(30),
            wants_network: true,
            local_policy_allows_network: false,
        };
        let decision = RemoteApprovalDecision::approve("req-network", "remote-companion");

        assert!(!request.remote_decision_can_grant_network(&decision));
    }

    #[test]
    fn remote_approval_network_grant_requires_matching_unexpired_decision() {
        let now = Utc::now();
        let request = RemoteApprovalRequest {
            request_id: "req-network".to_string(),
            matched_policy_id: Some("policy-confirm".to_string()),
            expires_at: now + Duration::seconds(30),
            wants_network: true,
            local_policy_allows_network: true,
        };
        let matching = RemoteApprovalDecision {
            request_id: "req-network".to_string(),
            decision: RemoteApprovalDisposition::Approve,
            remote_origin: "remote-companion".to_string(),
            decided_at: now + Duration::seconds(5),
        };
        let mismatched = RemoteApprovalDecision {
            request_id: "other-request".to_string(),
            ..matching.clone()
        };
        let stale = RemoteApprovalDecision {
            decided_at: now + Duration::seconds(31),
            ..matching.clone()
        };

        assert!(request.remote_decision_can_grant_network(&matching));
        assert!(!request.remote_decision_can_grant_network(&mismatched));
        assert!(!request.remote_decision_can_grant_network(&stale));
    }

    #[test]
    fn audit_entry_records_remote_origin_without_secret_material() {
        let request = RemoteApprovalRequest {
            request_id: "req-1".to_string(),
            matched_policy_id: Some("policy-confirm".to_string()),
            expires_at: Utc::now() + Duration::seconds(30),
            wants_network: false,
            local_policy_allows_network: false,
        };
        let decision = RemoteApprovalDecision::decline(
            "req-1",
            "https://device:secret-token@remote-companion.local/path?token=abc#fragment",
        );
        let audit = RemoteApprovalAuditEntry::from_decision(&request, &decision);

        let serialized = serde_json::to_string(&audit).expect("must serialize");
        assert!(serialized.contains("remote_companion.local/path"));
        assert!(serialized.contains("policy-confirm"));
        assert!(serialized.contains("req-1"));
        assert!(!serialized.contains("nonce"));
        assert!(!serialized.contains("token"));
        assert!(!serialized.contains("abc"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("device-binding-hash"));
    }

    #[test]
    fn remote_companion_is_opt_in_observe_approve_by_default() {
        let config = RemoteCompanionConfig::default();

        assert!(!config.enabled);
        assert_eq!(config.mode, RemoteCompanionMode::ObserveApprove);
        assert!(!config.allows_remote_control());
    }
}
