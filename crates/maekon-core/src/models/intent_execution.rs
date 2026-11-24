//! Bounded, execution-local observations for natural-language automation (#12060).
//!
//! These records describe observations, never permission to execute. Their wire
//! surface contains only opaque UUIDs, fixed enums, counters and timestamps;
//! provider names, prompts, endpoints and error messages cannot enter it.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::CoreError;
use crate::models::automation::PlannedIntentResult;

mod observer;
pub use observer::IntentExecutionObserver;

pub const MAX_INTENT_ATTEMPTS: usize = 8;
pub const MAX_INTENT_EVENTS: usize = 32;

/// A random correlation, not a command, secret, authorization or replay token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct OpaqueIntentId(Uuid);

impl<'de> Deserialize<'de> for OpaqueIntentId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl OpaqueIntentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for OpaqueIntentId {
    fn default() -> Self {
        Self::new()
    }
}

impl FromStr for OpaqueIntentId {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let id = Uuid::parse_str(value).map_err(|_| "invalid intent correlation")?;
        if id.is_nil() || id.to_string() != value {
            return Err("intent correlation must be a canonical, non-nil UUID");
        }
        Ok(Self(id))
    }
}

impl fmt::Display for OpaqueIntentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntentSubmission {
    pub submission_id: Option<OpaqueIntentId>,
    /// The controller preserves this only when the prior execution is known.
    pub retry_of: Option<OpaqueIntentId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentProvenanceVersion {
    #[serde(rename = "maekon.intent_execution.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentProviderClass {
    LocalModel,
    RemoteModel,
    Rule,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentRevision {
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentConfigurationReason {
    RuleOnlyDegradation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentSkillUse {
    Used,
    NotUsed,
    Unsupported,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentSkillContextKind {
    #[default]
    None,
    Catalog,
    Activated,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentCapabilityObservation {
    pub skills_requested: bool,
    /// None means the adapter does not observe whether it forwards context.
    pub skills_context_forwarded: Option<bool>,
    pub skill_context_kind: IntentSkillContextKind,
    pub skills_effective: IntentSkillUse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentSafeReason {
    Denied,
    Timeout,
    Unavailable,
    InvalidOutput,
    ProviderError,
    ConfidenceRejected,
    UnsupportedAction,
    ActionFailed,
    ObservationUnavailable,
    LegacyUnknown,
}

impl IntentSafeReason {
    /// Classify variants; never retain or serialize the error's Display text.
    pub fn from_error(error: &CoreError) -> Self {
        match error {
            CoreError::PolicyDenied { .. }
            | CoreError::PermissionDenied { .. }
            | CoreError::PrivacyDenied { .. }
            | CoreError::ConsentRequired { .. }
            | CoreError::ConsentExpired { .. }
            | CoreError::Auth { .. } => Self::Denied,
            CoreError::RequestTimeout { .. } | CoreError::ExecutionTimeout { .. } => Self::Timeout,
            CoreError::Network { .. }
            | CoreError::ServiceUnavailable { .. }
            | CoreError::RateLimit { .. }
            | CoreError::Config { .. } => Self::Unavailable,
            CoreError::Analysis { .. } | CoreError::InvalidArguments { .. } => Self::InvalidOutput,
            _ => Self::ProviderError,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentAttemptOutcome {
    Succeeded,
    Denied,
    Timeout,
    InvalidOutput,
    Unavailable,
    Error,
    Unknown,
}

impl IntentAttemptOutcome {
    pub fn from_error(error: &CoreError) -> Self {
        match IntentSafeReason::from_error(error) {
            IntentSafeReason::Denied => Self::Denied,
            IntentSafeReason::Timeout => Self::Timeout,
            IntentSafeReason::Unavailable => Self::Unavailable,
            IntentSafeReason::InvalidOutput => Self::InvalidOutput,
            _ => Self::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentProviderAttempt {
    pub attempt_id: OpaqueIntentId,
    pub predecessor: Option<OpaqueIntentId>,
    pub effective_provider_class: IntentProviderClass,
    pub model_revision: IntentRevision,
    pub outcome: IntentAttemptOutcome,
    pub reason: Option<IntentSafeReason>,
    pub capabilities: IntentCapabilityObservation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentPlanningOutcome {
    Llm,
    Rule,
    Unsupported,
    Denied,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentActionOutcome {
    NotStarted,
    Denied,
    Completed,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentObservation {
    Complete,
    Partial,
    LegacyUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentObservationStorage {
    /// A bounded process observation, not acknowledgment of a database commit.
    ProcessMemory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntentExecutionEventKind {
    // Empty struct variants enforce deny_unknown_fields for payload-free stages.
    // Serde's internally tagged unit variants silently accept extra fields.
    Started {},
    AttemptStarted { attempt_id: OpaqueIntentId },
    AttemptFinished { attempt_id: OpaqueIntentId },
    PlanningFinished { outcome: IntentPlanningOutcome },
    ActionStarted {},
    ActionFinished { outcome: IntentActionOutcome },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentExecutionEvent {
    pub execution_id: OpaqueIntentId,
    pub event_id: OpaqueIntentId,
    pub sequence: u32,
    pub observed_at: DateTime<Utc>,
    pub kind: IntentExecutionEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntentExecutionProvenance {
    pub schema_version: IntentProvenanceVersion,
    pub execution_id: OpaqueIntentId,
    pub submission_id: Option<OpaqueIntentId>,
    pub retry_of: Option<OpaqueIntentId>,
    pub config_revision: IntentRevision,
    pub requested_provider_class: IntentProviderClass,
    pub configuration_reason: Option<IntentConfigurationReason>,
    pub attempts: Vec<IntentProviderAttempt>,
    pub planning_outcome: IntentPlanningOutcome,
    pub action_outcome: IntentActionOutcome,
    pub observation: IntentObservation,
    pub observation_reason: Option<IntentSafeReason>,
    pub storage: IntentObservationStorage,
    pub events: Vec<IntentExecutionEvent>,
}

#[derive(Debug)]
pub struct ObservedIntentResult {
    pub result: PlannedIntentResult,
    pub provenance: Option<IntentExecutionProvenance>,
}

#[derive(Debug)]
pub struct ObservedIntentFailure {
    pub error: CoreError,
    pub provenance: Option<Box<IntentExecutionProvenance>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentLookupUnknownReason {
    Missing,
    Ambiguous,
    Expired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentExecutionLookup {
    Found(Box<IntentExecutionProvenance>),
    Unknown(IntentLookupUnknownReason),
}

/// Reassemble late delivery by execution and sequence; never use arrival order.
pub fn reconcile_intent_events(
    execution_id: OpaqueIntentId,
    events: &[IntentExecutionEvent],
) -> (Vec<IntentExecutionEvent>, IntentObservation) {
    let mut by_sequence = BTreeMap::new();
    let mut by_id = BTreeMap::new();
    let mut valid = events.len() <= MAX_INTENT_EVENTS * 2;
    for event in events.iter().take(MAX_INTENT_EVENTS * 2) {
        if event.execution_id != execution_id || event.sequence >= MAX_INTENT_EVENTS as u32 {
            valid = false;
            continue;
        }
        if let Some(previous) = by_id.insert(event.event_id, event) {
            if previous != event {
                valid = false;
            }
            continue;
        }
        if let Some(previous) = by_sequence.insert(event.sequence, event) {
            if previous != event {
                valid = false;
            }
        }
    }
    let ordered: Vec<_> = by_sequence.into_values().cloned().collect();
    valid &= ordered
        .iter()
        .enumerate()
        .all(|(index, event)| event.sequence as usize == index);
    valid &= matches!(
        ordered.first().map(|e| &e.kind),
        Some(IntentExecutionEventKind::Started {})
    );
    valid &= matches!(
        ordered.last().map(|e| &e.kind),
        Some(IntentExecutionEventKind::ActionFinished { .. })
    );
    let terminal_count = ordered
        .iter()
        .filter(|e| matches!(e.kind, IntentExecutionEventKind::ActionFinished { .. }))
        .count();
    valid &= terminal_count == 1;
    (
        ordered,
        if valid {
            IntentObservation::Complete
        } else {
            IntentObservation::Partial
        },
    )
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::error_codes::{InternalCode, NetworkCode, PolicyCode, ProviderCode};

    #[test]
    fn observed_intent_failure_stays_below_large_error_threshold() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ObservedIntentFailure>();
        let size = std::mem::size_of::<ObservedIntentFailure>();
        println!("ObservedIntentFailure size: {size} bytes");
        // Keep the named error below Clippy's default large-error threshold.
        assert!(size < 128, "observed intent failure is too large: {size}");
    }

    fn event_stream(count: usize) -> (OpaqueIntentId, Vec<IntentExecutionEvent>) {
        let execution_id = OpaqueIntentId::new();
        let events = (0..count)
            .map(|index| IntentExecutionEvent {
                execution_id,
                event_id: OpaqueIntentId::new(),
                sequence: index as u32,
                observed_at: Utc::now(),
                kind: if index == 0 {
                    IntentExecutionEventKind::Started {}
                } else if index == count - 1 {
                    IntentExecutionEventKind::ActionFinished {
                        outcome: IntentActionOutcome::Completed,
                    }
                } else {
                    IntentExecutionEventKind::ActionStarted {}
                },
            })
            .collect();
        (execution_id, events)
    }

    #[test]
    fn opaque_ids_require_canonical_non_nil_uuid_and_preserve_wire_identity() {
        let canonical = "7ea47fbb-bd3a-4d36-a2f7-aa12e57e9559";
        let id: OpaqueIntentId = canonical.parse().unwrap();
        assert_eq!(id.to_string(), canonical);
        assert_eq!(serde_json::to_value(id).unwrap(), canonical);
        assert_eq!(
            serde_json::from_value::<OpaqueIntentId>(serde_json::json!(canonical)).unwrap(),
            id
        );
        for (invalid, expected) in [
            ("raw prompt", "invalid intent correlation"),
            ("/private/key", "invalid intent correlation"),
            (
                "00000000-0000-0000-0000-000000000000",
                "intent correlation must be a canonical, non-nil UUID",
            ),
            (
                "7EA47FBB-BD3A-4D36-A2F7-AA12E57E9559",
                "intent correlation must be a canonical, non-nil UUID",
            ),
            (
                "7ea47fbbbd3a4d36a2f7aa12e57e9559",
                "intent correlation must be a canonical, non-nil UUID",
            ),
        ] {
            assert_eq!(invalid.parse::<OpaqueIntentId>().unwrap_err(), expected);
            let error =
                serde_json::from_value::<OpaqueIntentId>(serde_json::json!(invalid)).unwrap_err();
            assert_eq!(error.to_string(), expected);
        }
        assert_ne!(OpaqueIntentId::new(), OpaqueIntentId::new());
    }

    #[test]
    fn safe_reason_and_attempt_outcome_classify_without_error_text() {
        let canary = "CANARY_TOKEN endpoint=https://private.local /private/key OCR_CANARY";
        for (error, reason, outcome) in [
            (
                CoreError::PolicyDenied {
                    code: PolicyCode::Denied,
                    message: canary.into(),
                },
                IntentSafeReason::Denied,
                IntentAttemptOutcome::Denied,
            ),
            (
                CoreError::RequestTimeout {
                    code: NetworkCode::Timeout,
                    timeout_ms: 10,
                },
                IntentSafeReason::Timeout,
                IntentAttemptOutcome::Timeout,
            ),
            (
                CoreError::Network {
                    code: NetworkCode::Generic,
                    message: canary.into(),
                },
                IntentSafeReason::Unavailable,
                IntentAttemptOutcome::Unavailable,
            ),
            (
                CoreError::Analysis {
                    code: ProviderCode::AnalysisFailed,
                    message: canary.into(),
                },
                IntentSafeReason::InvalidOutput,
                IntentAttemptOutcome::InvalidOutput,
            ),
            (
                CoreError::Internal {
                    code: InternalCode::Generic,
                    message: canary.into(),
                },
                IntentSafeReason::ProviderError,
                IntentAttemptOutcome::Error,
            ),
        ] {
            assert_eq!(IntentSafeReason::from_error(&error), reason);
            assert_eq!(IntentAttemptOutcome::from_error(&error), outcome);
            let wire = serde_json::to_string(&(
                IntentSafeReason::from_error(&error),
                IntentAttemptOutcome::from_error(&error),
            ))
            .unwrap();
            for forbidden in ["CANARY", "private.local", "/private", "endpoint", "message"] {
                assert!(!wire.contains(forbidden));
            }
        }
    }

    #[test]
    fn wire_contract_rejects_unversioned_and_unapproved_observation_data() {
        let (execution_id, events) = event_stream(3);
        let receipt = IntentExecutionProvenance {
            schema_version: IntentProvenanceVersion::V1,
            execution_id,
            submission_id: Some(OpaqueIntentId::new()),
            retry_of: None,
            config_revision: IntentRevision::Unknown,
            requested_provider_class: IntentProviderClass::Unknown,
            configuration_reason: None,
            attempts: vec![IntentProviderAttempt {
                attempt_id: OpaqueIntentId::new(),
                predecessor: None,
                effective_provider_class: IntentProviderClass::Rule,
                model_revision: IntentRevision::NotApplicable,
                outcome: IntentAttemptOutcome::Succeeded,
                reason: None,
                capabilities: IntentCapabilityObservation::default(),
            }],
            planning_outcome: IntentPlanningOutcome::Rule,
            action_outcome: IntentActionOutcome::Completed,
            observation: IntentObservation::Complete,
            observation_reason: None,
            storage: IntentObservationStorage::ProcessMemory,
            events,
        };
        let value = serde_json::to_value(&receipt).unwrap();
        assert_eq!(value["schema_version"], "maekon.intent_execution.v1");
        assert_eq!(
            serde_json::from_value::<IntentExecutionProvenance>(value.clone()).unwrap(),
            receipt
        );
        for pointer in [
            "",
            "/attempts/0",
            "/attempts/0/capabilities",
            "/events/0",
            "/events/0/kind",
            "/events/1/kind",
            "/events/2/kind",
        ] {
            let mut injected = value.clone();
            injected.pointer_mut(pointer).unwrap()["raw_error"] = serde_json::json!("CANARY_TOKEN");
            let error = serde_json::from_value::<IntentExecutionProvenance>(injected).unwrap_err();
            assert!(
                error.to_string().contains("unknown field `raw_error`"),
                "wrong rejection at {pointer}: {error}"
            );
        }
        let mut unknown_version = value;
        unknown_version["schema_version"] = serde_json::json!("maekon.intent_execution.v999");
        let error =
            serde_json::from_value::<IntentExecutionProvenance>(unknown_version).unwrap_err();
        assert!(error
            .to_string()
            .contains("unknown variant `maekon.intent_execution.v999`"));
    }

    #[test]
    fn reverse_delivery_and_exact_duplicates_reconstruct_one_execution() {
        let (execution_id, events) = event_stream(4);
        let mut reverse = events.clone();
        reverse.reverse();
        reverse.extend(events.iter().cloned());
        let (ordered, status) = reconcile_intent_events(execution_id, &reverse);
        assert_eq!(ordered, events);
        assert_eq!(status, IntentObservation::Complete);
    }

    #[test]
    fn gaps_missing_boundaries_and_multiple_terminal_events_remain_partial() {
        let (execution_id, events) = event_stream(4);
        let mut gap = events.clone();
        gap.remove(1);
        let mut missing_start = events.clone();
        missing_start[0].kind = IntentExecutionEventKind::ActionStarted {};
        let mut missing_terminal = events.clone();
        missing_terminal[3].kind = IntentExecutionEventKind::ActionStarted {};
        let mut double_terminal = events.clone();
        double_terminal[1].kind = events[3].kind.clone();
        for invalid in [
            vec![],
            gap,
            missing_start,
            missing_terminal,
            double_terminal,
        ] {
            assert_eq!(
                reconcile_intent_events(execution_id, &invalid).1,
                IntentObservation::Partial
            );
        }
    }

    #[test]
    fn foreign_identity_conflicting_duplicates_and_out_of_range_sequences_fail_closed() {
        let (execution_id, events) = event_stream(3);
        let (_, foreign) = event_stream(3);
        assert_eq!(
            reconcile_intent_events(execution_id, &foreign),
            (vec![], IntentObservation::Partial)
        );
        let mut same_id_conflict = events.clone();
        let mut changed = events[1].clone();
        changed.kind = IntentExecutionEventKind::PlanningFinished {
            outcome: IntentPlanningOutcome::Rule,
        };
        same_id_conflict.push(changed);
        let mut same_sequence_conflict = events.clone();
        let mut changed = events[1].clone();
        changed.event_id = OpaqueIntentId::new();
        same_sequence_conflict.push(changed);
        let mut out_of_range = events.clone();
        let mut changed = events[1].clone();
        changed.event_id = OpaqueIntentId::new();
        changed.sequence = MAX_INTENT_EVENTS as u32;
        out_of_range.push(changed);
        for invalid in [same_id_conflict, same_sequence_conflict, out_of_range] {
            assert_eq!(
                reconcile_intent_events(execution_id, &invalid).1,
                IntentObservation::Partial
            );
        }
        let mut mixed = events.clone();
        mixed.extend(foreign);
        assert_eq!(
            reconcile_intent_events(execution_id, &mixed),
            (events, IntentObservation::Partial)
        );
    }

    #[test]
    fn bounded_delivery_accepts_the_exact_duplicate_budget_and_marks_overflow_partial() {
        let (execution_id, events) = event_stream(MAX_INTENT_EVENTS);
        // A delayed tail still fits when the first event is repeatedly delivered.
        let mut delayed = vec![events[0].clone(); MAX_INTENT_EVENTS + 1];
        delayed.extend(events.iter().skip(1).cloned());
        assert_eq!(delayed.len(), MAX_INTENT_EVENTS * 2);
        assert_eq!(
            reconcile_intent_events(execution_id, &delayed),
            (events.clone(), IntentObservation::Complete)
        );
        delayed.push(events[0].clone());
        let (ordered, status) = reconcile_intent_events(execution_id, &delayed);
        assert_eq!(ordered, events);
        assert_eq!(status, IntentObservation::Partial);
    }
}
