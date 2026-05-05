use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Shared metadata envelope for outbound and inbound integration messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationEnvelope {
    pub envelope_id: String,
    pub schema_version: String,
    pub message_type: IntegrationMessageType,
    pub timestamp: DateTime<Utc>,
    pub nonce: String,
    pub origin: IntegrationOrigin,
    pub capability_scope: IntegrationCapabilityScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationMessageType {
    InsightPacket,
    ProactivePrompt,
    SessionState,
    Bootstrap,
    PromptReceipt,
    Ack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationOrigin {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationTransportKind {
    #[default]
    WebSocket,
    HttpsSse,
    HttpsLongPoll,
    GrpcBidirectional,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationAuthScheme {
    #[default]
    BearerToken,
    DpopBearer,
}

impl std::fmt::Debug for IntegrationAuthScheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BearerToken => f.write_str("BearerToken"),
            Self::DpopBearer => f.write_str("DpopBearer"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationAuthProfileKind {
    #[default]
    EnvToken,
    OidcDeviceFlow,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct IntegrationAuthContext {
    pub access_token: String,
    pub scheme: IntegrationAuthScheme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_indicator: Option<String>,
}

impl std::fmt::Debug for IntegrationAuthContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntegrationAuthContext")
            .field("access_token", &"[REDACTED]")
            .field("scheme", &self.scheme)
            .field("expires_at", &self.expires_at)
            .field("resource_indicator", &self.resource_indicator)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationAuthStatusKind {
    #[default]
    Unconfigured,
    Unauthenticated,
    AwaitingUserAuthorization,
    Ready,
    Expired,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationDeviceAuthorizationFlow {
    pub flow_id: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub interval_secs: u64,
    #[serde(default)]
    pub requested_scopes: Vec<IntegrationCapabilityScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_indicator: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAuthStatus {
    pub profile_kind: IntegrationAuthProfileKind,
    pub status: IntegrationAuthStatusKind,
    pub interactive: bool,
    pub authenticated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_indicator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_flow: Option<IntegrationDeviceAuthorizationFlow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationRuntimeLaneTelemetry {
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backoff_until: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationRuntimeTelemetry {
    #[serde(default)]
    pub connect: IntegrationRuntimeLaneTelemetry,
    #[serde(default)]
    pub heartbeat: IntegrationRuntimeLaneTelemetry,
    #[serde(default)]
    pub egress: IntegrationRuntimeLaneTelemetry,
    #[serde(default)]
    pub inbox: IntegrationRuntimeLaneTelemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationCapabilityScope {
    InsightWrite,
    PromptRead,
    PromptAck,
    DevicePresenceWrite,
    SessionManage,
    PolicyRead,
}

impl IntegrationCapabilityScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InsightWrite => "insight:write",
            Self::PromptRead => "prompt:read",
            Self::PromptAck => "prompt:ack",
            Self::DevicePresenceWrite => "device_presence:write",
            Self::SessionManage => "session:manage",
            Self::PolicyRead => "policy:read",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let normalized = raw.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "insight:write" => Some(Self::InsightWrite),
            "prompt:read" => Some(Self::PromptRead),
            "prompt:ack" => Some(Self::PromptAck),
            "device_presence:write" => Some(Self::DevicePresenceWrite),
            "session:manage" => Some(Self::SessionManage),
            "policy:read" => Some(Self::PolicyRead),
            _ => None,
        }
    }
}

pub fn default_integration_runtime_scopes() -> Vec<IntegrationCapabilityScope> {
    vec![
        IntegrationCapabilityScope::SessionManage,
        IntegrationCapabilityScope::InsightWrite,
        IntegrationCapabilityScope::PromptRead,
        IntegrationCapabilityScope::PromptAck,
    ]
}

/// Privacy-filtered outbound packet sent to an integration backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightPacket {
    pub packet_id: String,
    pub summary: String,
    #[serde(default)]
    pub derived_tags: Vec<String>,
    pub source_window: InsightSourceWindow,
    pub privacy_classification: IntegrationPrivacyClassification,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_reference_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IntegrationOutboundPayload {
    Insight(InsightPacket),
    PromptReceipt(IntegrationPromptReceipt),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedIntegrationEgressMessage {
    pub queue_id: String,
    pub envelope: IntegrationEnvelope,
    pub payload: IntegrationOutboundPayload,
    pub queued_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInsightCandidate {
    pub source_cursor: String,
    pub envelope: IntegrationEnvelope,
    pub packet: InsightPacket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightSourceWindow {
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationPrivacyClassification {
    DeviceLocal,
    DerivedSummary,
    UserApprovedAttachment,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationEgressDisposition {
    Allow,
    Deny,
    RequireUserApproval,
}

/// Inbound prompt/task packet delivered to the desktop client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactivePrompt {
    pub prompt_id: String,
    pub category: ProactivePromptCategory,
    pub title: String,
    pub body: String,
    pub priority: ProactivePromptPriority,
    #[serde(default)]
    pub actions: Vec<ProactivePromptAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub provenance: PromptProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProactivePrompt {
    pub prompt: ProactivePrompt,
    pub received_at: DateTime<Utc>,
    pub status: IntegrationInboxItemStatus,
    pub status_updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presented_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismiss_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationInboxItemStatus {
    Pending,
    Acknowledged,
    Dismissed,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationPromptReceiptAction {
    Acknowledged,
    Dismissed,
}

impl IntegrationPromptReceiptAction {
    pub fn to_inbox_status(&self) -> IntegrationInboxItemStatus {
        match self {
            Self::Acknowledged => IntegrationInboxItemStatus::Acknowledged,
            Self::Dismissed => IntegrationInboxItemStatus::Dismissed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationPromptReceipt {
    pub receipt_id: String,
    pub prompt_id: String,
    pub action: IntegrationPromptReceiptAction,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProactivePromptCategory {
    Insight,
    Task,
    Reminder,
    Escalation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ProactivePromptPriority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactivePromptAction {
    pub action_id: String,
    pub label: String,
    pub action_type: ProactivePromptActionType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProactivePromptActionType {
    OpenLink,
    OpenSettings,
    RunAutomation,
    Dismiss,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptProvenance {
    pub source_system: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationSessionState {
    pub session_id: String,
    pub device_id: String,
    pub status: IntegrationSessionStatus,
    #[serde(default)]
    pub transport_kind: IntegrationTransportKind,
    #[serde(default)]
    pub auth_scheme: IntegrationAuthScheme,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub requested_scopes: Vec<IntegrationCapabilityScope>,
    #[serde(default)]
    pub granted_scopes: Vec<IntegrationCapabilityScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ack_cursors: Vec<IntegrationAckCursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationSessionStatus {
    Disconnected,
    Connecting,
    Connected,
    Degraded,
    Reconnecting,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationAckCursor {
    pub stream_id: String,
    pub cursor: String,
    pub acknowledged_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationInsightAuditRecord {
    pub record_id: String,
    pub envelope_id: String,
    pub packet_id: String,
    pub disposition: IntegrationEgressDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub privacy_classification: IntegrationPrivacyClassification,
    pub capability_scope: IntegrationCapabilityScope,
    pub occurred_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integration_envelope_roundtrip() {
        let envelope = IntegrationEnvelope {
            envelope_id: "env-001".to_string(),
            schema_version: "integration.envelope.v1".to_string(),
            message_type: IntegrationMessageType::InsightPacket,
            timestamp: Utc::now(),
            nonce: "nonce-001".to_string(),
            origin: IntegrationOrigin {
                device_id: "device-001".to_string(),
                workspace_id: Some("workspace-001".to_string()),
                session_id: Some("session-001".to_string()),
                source: "desktop-client".to_string(),
            },
            capability_scope: IntegrationCapabilityScope::InsightWrite,
        };

        let json = serde_json::to_string(&envelope).unwrap();
        let parsed: IntegrationEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.envelope_id, "env-001");
        assert_eq!(parsed.message_type, IntegrationMessageType::InsightPacket);
        assert_eq!(
            parsed.capability_scope,
            IntegrationCapabilityScope::InsightWrite
        );
    }

    #[test]
    fn proactive_prompt_roundtrip() {
        let prompt = ProactivePrompt {
            prompt_id: "prompt-001".to_string(),
            category: ProactivePromptCategory::Task,
            title: "Review inbox".to_string(),
            body: "A teammate requested a review.".to_string(),
            priority: ProactivePromptPriority::High,
            actions: vec![ProactivePromptAction {
                action_id: "open-settings".to_string(),
                label: "Open Settings".to_string(),
                action_type: ProactivePromptActionType::OpenSettings,
                payload: None,
            }],
            expires_at: None,
            provenance: PromptProvenance {
                source_system: "team-server".to_string(),
                source_actor: Some("scheduler".to_string()),
                correlation_id: Some("corr-001".to_string()),
            },
        };

        let json = serde_json::to_string(&prompt).unwrap();
        let parsed: ProactivePrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.prompt_id, "prompt-001");
        assert_eq!(parsed.priority, ProactivePromptPriority::High);
        assert_eq!(parsed.actions.len(), 1);
    }

    #[test]
    fn stored_prompt_roundtrip() {
        let stored = StoredProactivePrompt {
            prompt: ProactivePrompt {
                prompt_id: "prompt-001".to_string(),
                category: ProactivePromptCategory::Insight,
                title: "Review focus".to_string(),
                body: "A server-side insight is ready.".to_string(),
                priority: ProactivePromptPriority::Medium,
                actions: Vec::new(),
                expires_at: None,
                provenance: PromptProvenance {
                    source_system: "team-server".to_string(),
                    source_actor: None,
                    correlation_id: Some("corr-002".to_string()),
                },
            },
            received_at: Utc::now(),
            status: IntegrationInboxItemStatus::Pending,
            status_updated_at: Utc::now(),
            presented_at: None,
            dismiss_reason: None,
        };

        let json = serde_json::to_string(&stored).unwrap();
        let parsed: StoredProactivePrompt = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.prompt.prompt_id, "prompt-001");
        assert_eq!(parsed.status, IntegrationInboxItemStatus::Pending);
    }

    #[test]
    fn session_state_roundtrip() {
        let state = IntegrationSessionState {
            session_id: "session-001".to_string(),
            device_id: "device-001".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: IntegrationTransportKind::WebSocket,
            auth_scheme: IntegrationAuthScheme::BearerToken,
            connected_at: Some(Utc::now()),
            last_heartbeat_at: Some(Utc::now()),
            requested_scopes: vec![IntegrationCapabilityScope::SessionManage],
            granted_scopes: vec![IntegrationCapabilityScope::SessionManage],
            ack_cursors: vec![IntegrationAckCursor {
                stream_id: "inbox".to_string(),
                cursor: "42".to_string(),
                acknowledged_at: Utc::now(),
            }],
        };

        let json = serde_json::to_string(&state).unwrap();
        let parsed: IntegrationSessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.status, IntegrationSessionStatus::Connected);
        assert_eq!(parsed.granted_scopes.len(), 1);
        assert_eq!(parsed.ack_cursors[0].cursor, "42");
        assert_eq!(parsed.transport_kind, IntegrationTransportKind::WebSocket);
        assert_eq!(parsed.auth_scheme, IntegrationAuthScheme::BearerToken);
    }

    #[test]
    fn queued_outbound_message_roundtrip() {
        let queued = QueuedIntegrationEgressMessage {
            queue_id: "queue-001".to_string(),
            envelope: IntegrationEnvelope {
                envelope_id: "env-001".to_string(),
                schema_version: "integration.envelope.v1".to_string(),
                message_type: IntegrationMessageType::InsightPacket,
                timestamp: Utc::now(),
                nonce: "nonce-001".to_string(),
                origin: IntegrationOrigin {
                    device_id: "device-001".to_string(),
                    workspace_id: None,
                    session_id: None,
                    source: "desktop-client".to_string(),
                },
                capability_scope: IntegrationCapabilityScope::InsightWrite,
            },
            payload: IntegrationOutboundPayload::PromptReceipt(IntegrationPromptReceipt {
                receipt_id: "receipt-001".to_string(),
                prompt_id: "prompt-001".to_string(),
                action: IntegrationPromptReceiptAction::Acknowledged,
                occurred_at: Utc::now(),
                reason: None,
            }),
            queued_at: Utc::now(),
        };

        let json = serde_json::to_string(&queued).unwrap();
        let parsed: QueuedIntegrationEgressMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.queue_id, "queue-001");
        match parsed.payload {
            IntegrationOutboundPayload::PromptReceipt(receipt) => {
                assert_eq!(receipt.prompt_id, "prompt-001");
                assert_eq!(receipt.action, IntegrationPromptReceiptAction::Acknowledged);
            }
            IntegrationOutboundPayload::Insight(_) => panic!("expected prompt receipt payload"),
        }
    }

    #[test]
    fn insight_outbound_message_roundtrip() {
        let queued = QueuedIntegrationEgressMessage {
            queue_id: "queue-002".to_string(),
            envelope: IntegrationEnvelope {
                envelope_id: "env-002".to_string(),
                schema_version: "integration.envelope.v1".to_string(),
                message_type: IntegrationMessageType::InsightPacket,
                timestamp: Utc::now(),
                nonce: "nonce-002".to_string(),
                origin: IntegrationOrigin {
                    device_id: "device-001".to_string(),
                    workspace_id: None,
                    session_id: None,
                    source: "desktop-client".to_string(),
                },
                capability_scope: IntegrationCapabilityScope::InsightWrite,
            },
            payload: IntegrationOutboundPayload::Insight(InsightPacket {
                packet_id: "packet-001".to_string(),
                summary: "summary".to_string(),
                derived_tags: vec!["focus".to_string()],
                source_window: InsightSourceWindow {
                    started_at: Utc::now(),
                    ended_at: Utc::now(),
                },
                privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
                audit_reference_id: Some("audit-001".to_string()),
            }),
            queued_at: Utc::now(),
        };

        let json = serde_json::to_string(&queued).unwrap();
        let parsed: QueuedIntegrationEgressMessage = serde_json::from_str(&json).unwrap();
        match parsed.payload {
            IntegrationOutboundPayload::Insight(packet) => {
                assert_eq!(packet.packet_id, "packet-001");
            }
            IntegrationOutboundPayload::PromptReceipt(_) => {
                panic!("expected insight payload")
            }
        }
    }

    #[test]
    fn capability_scope_parse_roundtrip() {
        assert_eq!(
            IntegrationCapabilityScope::parse("prompt:read"),
            Some(IntegrationCapabilityScope::PromptRead)
        );
        assert_eq!(
            IntegrationCapabilityScope::PromptRead.as_str(),
            "prompt:read"
        );
        assert_eq!(IntegrationCapabilityScope::parse("unknown"), None);
    }
}
