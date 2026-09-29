//! Bounded WBS interaction messages. None of these values independently grants a write.

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::models::wbs_assignment_candidates::{
    local::WbsLocalInputRevision, WbsAssignmentContext, WbsCandidateSourceKind,
};
use crate::models::wbs_cell_apply::WbsCellAnchor;

pub const WBS_CONSENT_NOTICE: &str = "wbs-cell-assignee.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WbsPermittedOperation {
    ReadContext,
    FetchCandidates,
    ApplyOneLiteralAssignee,
}
pub const WBS_CONSENT_OPERATIONS: [WbsPermittedOperation; 3] = [
    WbsPermittedOperation::ReadContext,
    WbsPermittedOperation::FetchCandidates,
    WbsPermittedOperation::ApplyOneLiteralAssignee,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum WbsAutomationError {
    #[error("wbs_unavailable")]
    Unavailable,
    #[error("wbs_invalid_input")]
    InvalidInput,
    #[error("wbs_automation_disabled")]
    AutomationDisabled,
    #[error("wbs_sandbox_unsupported")]
    SandboxUnsupported,
    #[error("wbs_policy_blocked")]
    PolicyBlocked,
    #[error("wbs_consent_required")]
    ConsentRequired,
    #[error("wbs_consent_changed")]
    ConsentChanged,
    #[error("wbs_configuration_changed")]
    ConfigurationChanged,
    #[error("wbs_expired")]
    Expired,
    #[error("wbs_invalid_capability")]
    InvalidCapability,
    #[error("wbs_invalid_proof")]
    InvalidProof,
    #[error("wbs_stale_selection")]
    StaleSelection,
    #[error("wbs_stale_candidate")]
    StaleCandidate,
    #[error("wbs_confirmation_denied")]
    ConfirmationDenied,
    #[error("wbs_audit_unavailable")]
    AuditUnavailable,
    #[error("wbs_capacity_exceeded")]
    CapacityExceeded,
    #[error("wbs_operation_not_found")]
    OperationNotFound,
    #[error("wbs_interaction_busy")]
    Busy,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}
fn proof(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

macro_rules! bounded_string {
    ($name:ident, $validator:ident) => {
        #[derive(Clone, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: String) -> Result<Self, WbsAutomationError> {
                if !$validator(&value) {
                    return Err(WbsAutomationError::InvalidInput);
                }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    };
}
bounded_string!(WbsIdentifier, identifier);
bounded_string!(WbsProof, proof);
bounded_string!(WbsDisplayText, text);

/// Decimal-string wire representation preserves the entire u64 range in JavaScript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WbsGeneration(u64);
impl WbsGeneration {
    pub fn new(value: u64) -> Result<Self, WbsAutomationError> {
        if value == 0 {
            return Err(WbsAutomationError::InvalidInput);
        }
        Ok(Self(value))
    }
    pub fn value(self) -> u64 {
        self.0
    }
}
impl Serialize for WbsGeneration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for WbsGeneration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = String::deserialize(d)?;
        if value.is_empty()
            || value.len() > 20
            || value.starts_with('0')
            || !value.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(serde::de::Error::custom("invalid WBS generation"));
        }
        let value = value.parse().map_err(serde::de::Error::custom)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

macro_rules! input_message {
    ($name:ident { $($field:ident: $ty:ty),+ $(,)? }) => {
        #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name { $($field: $ty),+ }
        impl $name {
            pub fn new($($field: $ty),+) -> Self { Self { $($field),+ } }
            $(pub fn $field(&self) -> &$ty { &self.$field })+
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

input_message!(WbsSessionAuthorization {
    session_id: WbsIdentifier,
    capability: WbsProof
});

macro_rules! message {
    ($name:ident { $($field:ident: $ty:ty),+ $(,)? }) => {
        #[derive(Clone, PartialEq, Eq, Serialize)]
        pub struct $name { $($field: $ty),+ }
        impl $name {
            pub fn new($($field: $ty),+) -> Self { Self { $($field),+ } }
            $(pub fn $field(&self) -> &$ty { &self.$field })+
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }
    };
}

message!(WbsLocalContextView {
    document_registration_id: WbsIdentifier,
    input_revision: WbsIdentifier,
    input_hash: WbsProof,
    roster_revision: WbsIdentifier,
    roster_hash: WbsProof
});

/// Trusted composition input, deliberately not deserializable from a WebView.
#[derive(Clone)]
pub struct WbsDocumentScope {
    scope_id: WbsIdentifier,
    native_registration_digest: WbsProof,
    server_origin_digest: Option<WbsProof>,
    document_display_label: WbsDisplayText,
    allowed_contexts: Vec<WbsAssignmentContext>,
}
impl WbsDocumentScope {
    pub fn new(
        scope_id: WbsIdentifier,
        native_registration_digest: WbsProof,
        server_origin_digest: WbsProof,
        document_display_label: WbsDisplayText,
        allowed_contexts: Vec<WbsAssignmentContext>,
    ) -> Result<Self, WbsAutomationError> {
        let keys: BTreeSet<_> = allowed_contexts
            .iter()
            .map(|c| (c.organization_id(), c.wbs_item_id()))
            .collect();
        if allowed_contexts.is_empty()
            || allowed_contexts.len() > 64
            || keys.len() != allowed_contexts.len()
            || allowed_contexts
                .iter()
                .any(|c| c.source_kind() != WbsCandidateSourceKind::ServerBoard)
        {
            return Err(WbsAutomationError::InvalidInput);
        }
        Ok(Self {
            scope_id,
            native_registration_digest,
            server_origin_digest: Some(server_origin_digest),
            document_display_label,
            allowed_contexts,
        })
    }
    pub fn new_local(
        scope_id: WbsIdentifier,
        native_registration_digest: WbsProof,
        document_display_label: WbsDisplayText,
        allowed_contexts: Vec<WbsAssignmentContext>,
    ) -> Result<Self, WbsAutomationError> {
        let input = allowed_contexts
            .first()
            .and_then(WbsAssignmentContext::local_input)
            .ok_or(WbsAutomationError::InvalidInput)?;
        let rows: BTreeSet<_> = allowed_contexts
            .iter()
            .map(WbsAssignmentContext::wbs_item_id)
            .collect();
        if allowed_contexts.len() > 64
            || rows.len() != allowed_contexts.len()
            || allowed_contexts
                .iter()
                .any(|c| c.local_input() != Some(input))
        {
            return Err(WbsAutomationError::InvalidInput);
        }
        Ok(Self {
            scope_id,
            native_registration_digest,
            server_origin_digest: None,
            document_display_label,
            allowed_contexts,
        })
    }
    pub fn scope_id(&self) -> &WbsIdentifier {
        &self.scope_id
    }
    pub fn native_registration_digest(&self) -> &WbsProof {
        &self.native_registration_digest
    }
    pub fn server_origin_digest(&self) -> Option<&WbsProof> {
        self.server_origin_digest.as_ref()
    }
    pub fn source_kind(&self) -> WbsCandidateSourceKind {
        if self.server_origin_digest.is_some() {
            WbsCandidateSourceKind::ServerBoard
        } else {
            WbsCandidateSourceKind::LocalDocumentRoster
        }
    }
    pub fn local_input(&self) -> Option<&WbsLocalInputRevision> {
        self.allowed_contexts
            .first()
            .and_then(WbsAssignmentContext::local_input)
    }
    pub fn document_display_label(&self) -> &WbsDisplayText {
        &self.document_display_label
    }
    pub fn allowed_contexts(&self) -> &[WbsAssignmentContext] {
        &self.allowed_contexts
    }
}
impl fmt::Debug for WbsDocumentScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WbsDocumentScope([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct WbsContextView {
    source_kind: WbsCandidateSourceKind,
    organization_id: Option<WbsIdentifier>,
    wbs_item_id: WbsIdentifier,
    wbs_version_id: Option<WbsIdentifier>,
    local: Option<WbsLocalContextView>,
}
impl WbsContextView {
    pub fn new(
        organization_id: WbsIdentifier,
        wbs_item_id: WbsIdentifier,
        wbs_version_id: Option<WbsIdentifier>,
    ) -> Self {
        Self {
            source_kind: WbsCandidateSourceKind::ServerBoard,
            organization_id: Some(organization_id),
            wbs_item_id,
            wbs_version_id,
            local: None,
        }
    }
    pub fn new_local(wbs_item_id: WbsIdentifier, local: WbsLocalContextView) -> Self {
        Self {
            source_kind: WbsCandidateSourceKind::LocalDocumentRoster,
            organization_id: None,
            wbs_item_id,
            wbs_version_id: None,
            local: Some(local),
        }
    }
    pub fn source_kind(&self) -> WbsCandidateSourceKind {
        self.source_kind
    }
    pub fn organization_id(&self) -> Option<&WbsIdentifier> {
        self.organization_id.as_ref()
    }
    pub fn wbs_item_id(&self) -> &WbsIdentifier {
        &self.wbs_item_id
    }
    pub fn wbs_version_id(&self) -> &Option<WbsIdentifier> {
        &self.wbs_version_id
    }
    pub fn local(&self) -> Option<&WbsLocalContextView> {
        self.local.as_ref()
    }
}
impl fmt::Debug for WbsContextView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WbsContextView([REDACTED])")
    }
}

message!(WbsConsentOffer {
    offer_id: WbsIdentifier, nonce: WbsProof, scope_id: WbsIdentifier,
    notice_version: WbsIdentifier, document_display_label: WbsDisplayText,
    permitted_operations: [WbsPermittedOperation; 3],
    expires_at: DateTime<Utc>
});
input_message!(WbsConsentAcceptance {
    offer_id: WbsIdentifier,
    nonce: WbsProof,
    scope_id: WbsIdentifier,
    notice_version: WbsIdentifier
});
message!(WbsSessionView {
    authorization: WbsSessionAuthorization, scope_id: WbsIdentifier, expires_at: DateTime<Utc>
});
message!(WbsLocalSourceView {
    input: WbsLocalContextView,
    provider_selection_digest: WbsProof,
    provider_name: WbsDisplayText,
    model: WbsDisplayText,
    prompt_revision: WbsIdentifier
});

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct WbsSourceView {
    source_kind: WbsCandidateSourceKind,
    snapshot_id: WbsIdentifier,
    snapshot_version: WbsIdentifier,
    snapshot_hash: WbsProof,
    wbs_version_id: Option<WbsIdentifier>,
    wbs_content_hash: Option<WbsProof>,
    approval_id: Option<WbsIdentifier>,
    local: Option<WbsLocalSourceView>,
}
impl WbsSourceView {
    pub fn new(
        snapshot_id: WbsIdentifier,
        snapshot_version: WbsIdentifier,
        snapshot_hash: WbsProof,
        wbs_version_id: WbsIdentifier,
        wbs_content_hash: WbsProof,
        approval_id: WbsIdentifier,
    ) -> Self {
        Self {
            source_kind: WbsCandidateSourceKind::ServerBoard,
            snapshot_id,
            snapshot_version,
            snapshot_hash,
            wbs_version_id: Some(wbs_version_id),
            wbs_content_hash: Some(wbs_content_hash),
            approval_id: Some(approval_id),
            local: None,
        }
    }
    pub fn new_local(
        snapshot_id: WbsIdentifier,
        snapshot_version: WbsIdentifier,
        snapshot_hash: WbsProof,
        local: WbsLocalSourceView,
    ) -> Self {
        Self {
            source_kind: WbsCandidateSourceKind::LocalDocumentRoster,
            snapshot_id,
            snapshot_version,
            snapshot_hash,
            wbs_version_id: None,
            wbs_content_hash: None,
            approval_id: None,
            local: Some(local),
        }
    }
    pub fn source_kind(&self) -> WbsCandidateSourceKind {
        self.source_kind
    }
    pub fn snapshot_id(&self) -> &WbsIdentifier {
        &self.snapshot_id
    }
    pub fn snapshot_version(&self) -> &WbsIdentifier {
        &self.snapshot_version
    }
    pub fn snapshot_hash(&self) -> &WbsProof {
        &self.snapshot_hash
    }
    pub fn wbs_version_id(&self) -> Option<&WbsIdentifier> {
        self.wbs_version_id.as_ref()
    }
    pub fn wbs_content_hash(&self) -> Option<&WbsProof> {
        self.wbs_content_hash.as_ref()
    }
    pub fn approval_id(&self) -> Option<&WbsIdentifier> {
        self.approval_id.as_ref()
    }
    pub fn local(&self) -> Option<&WbsLocalSourceView> {
        self.local.as_ref()
    }
}
impl fmt::Debug for WbsSourceView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WbsSourceView([REDACTED])")
    }
}
message!(WbsTargetHint {
    row: u32,
    column: u32
});
message!(WbsCandidateCard {
    candidate_id: WbsIdentifier,
    display_name: WbsDisplayText,
    rank: u32,
    eligible: bool,
    rank_reason: WbsDisplayText,
    candidate_proof: WbsProof
});
/// Output-only aggregate. The service creates this from bounded core candidates.
#[derive(Clone, Serialize)]
pub struct WbsRecommendationView {
    session_id: WbsIdentifier,
    query_generation: WbsGeneration,
    context: WbsContextView,
    source: Option<WbsSourceView>,
    synthetic: bool,
    provenance: Vec<WbsDisplayText>,
    target_hint: WbsTargetHint,
    candidates: Vec<WbsCandidateCard>,
    expires_at: DateTime<Utc>,
    #[serde(skip)]
    anchor: Option<WbsCellAnchor>,
}
impl WbsRecommendationView {
    pub fn new(
        session: &WbsSessionView,
        query_generation: WbsGeneration,
        context: WbsContextView,
        source: Option<WbsSourceView>,
        provenance: (bool, Vec<WbsDisplayText>),
        target_hint: WbsTargetHint,
        candidates: Vec<WbsCandidateCard>,
    ) -> Result<Self, WbsAutomationError> {
        if candidates.len() > 10
            || provenance.1.len() > 16
            || (source.is_none() && !candidates.is_empty())
        {
            return Err(WbsAutomationError::InvalidInput);
        }
        if let Some(source) = &source {
            if source.source_kind != context.source_kind
                || source.local.as_ref().map(WbsLocalSourceView::input) != context.local.as_ref()
            {
                return Err(WbsAutomationError::InvalidInput);
            }
        }
        Ok(Self {
            session_id: session.authorization.session_id.clone(),
            query_generation,
            context,
            source,
            synthetic: provenance.0,
            provenance: provenance.1,
            target_hint,
            candidates,
            expires_at: session.expires_at,
            anchor: None,
        })
    }
    pub fn query_generation(&self) -> WbsGeneration {
        self.query_generation
    }
    pub fn candidates(&self) -> &[WbsCandidateCard] {
        &self.candidates
    }
    pub fn source(&self) -> Option<&WbsSourceView> {
        self.source.as_ref()
    }
    pub fn context(&self) -> &WbsContextView {
        &self.context
    }
    pub fn session_id(&self) -> &WbsIdentifier {
        &self.session_id
    }
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }
    /// Presentation uses the already bound native observation, never a second
    /// selection capture. Unverified screen conversion remains None.
    pub fn with_anchor(mut self, anchor: Option<WbsCellAnchor>) -> Self {
        self.anchor = anchor;
        self
    }
    pub fn anchor(&self) -> Option<WbsCellAnchor> {
        self.anchor
    }
}
impl fmt::Debug for WbsRecommendationView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WbsRecommendationView([REDACTED])")
    }
}
input_message!(WbsCandidateChoice {
    query_generation: WbsGeneration,
    candidate_id: WbsIdentifier,
    candidate_proof: WbsProof
});
input_message!(WbsExecutionTicket {
    operation_id: WbsIdentifier, session_id: WbsIdentifier, nonce: WbsProof,
    payload_digest: WbsProof, expires_at: DateTime<Utc>, signature: WbsProof
});
input_message!(WbsOperationRef {
    operation_id: WbsIdentifier
});

macro_rules! status_enum {
    ($name:ident { $($variant:ident),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }
    };
}
status_enum!(WbsDisposition {
    Ready,
    InProgress,
    AppliedVerified,
    CancelledBeforeStart,
    Denied,
    OutcomeUnknown,
    ReadbackMismatch,
    AuditCompletionFailed,
    AlreadyAttempted
});
status_enum!(WbsNativeOutcome {
    NotStarted,
    WrittenUnverified,
    CancelledBeforeStart,
    AlreadyAttempted,
    OutcomeUnknown,
    Rejected
});
status_enum!(WbsReadbackState {
    NotRead,
    Matches,
    Mismatch,
    Unavailable
});
status_enum!(WbsAuditState {
    NotStarted,
    StartAccepted,
    CompletionAccepted,
    CompletionRejected
});
message!(WbsExecutionView {
    operation_id: WbsIdentifier,
    disposition: WbsDisposition,
    native_outcome: WbsNativeOutcome,
    readback: WbsReadbackState,
    audit: WbsAuditState
});
#[derive(Clone, Serialize)]
pub struct WbsSessionCancellation {
    session_id: WbsIdentifier,
    closed: bool,
    operations: Vec<WbsExecutionView>,
}
impl WbsSessionCancellation {
    pub fn new(
        session_id: WbsIdentifier,
        operations: Vec<WbsExecutionView>,
    ) -> Result<Self, WbsAutomationError> {
        if operations.len() > 64 {
            return Err(WbsAutomationError::InvalidInput);
        }
        Ok(Self {
            session_id,
            closed: true,
            operations,
        })
    }
    pub fn operations(&self) -> &[WbsExecutionView] {
        &self.operations
    }
}
impl fmt::Debug for WbsSessionCancellation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WbsSessionCancellation([REDACTED])")
    }
}

#[cfg(test)]
mod tests;
