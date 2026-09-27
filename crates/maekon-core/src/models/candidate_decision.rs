//! Advisory candidate decisions. These values never authorize GUI execution.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::context::WindowInfo;
use super::gui::GuiCandidate;

pub const JEV_MODEL: &str = "jev-1.13.0";
pub const JEV_ENDPOINT: &str = "https://api.typesafe.ai/v1/systemone";
pub const DECISION_SCHEMA: &str = "candidate-decision.v1";
pub const DECISION_RUBRIC: &str = "goal-bound-candidate.v1";
pub const MAX_CANDIDATES: usize = 32;
pub const MAX_PAYLOAD_BYTES: usize = 16_384;
pub const NONE_OPTION: &str = "none";
pub const DELEGATE_OPTION: &str = "delegate";

/// Local-only input; intentionally has no serialization or raw-text Debug.
#[derive(Clone)]
pub struct CandidateSnapshot {
    pub goal: String,
    pub scene_id: String,
    pub frame_id: String,
    pub generation: u64,
    pub window: WindowInfo,
    pub candidates: Vec<GuiCandidate>,
}

#[derive(Clone)]
pub struct CandidateDecisionRequest {
    snapshot: CandidateSnapshot,
    binding: DecisionBinding,
}

/// Both monotonic timestamps participate in equality; a cache cannot renew TTL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionBinding {
    pub digest: String,
    pub window_digest: String,
    pub observed_at: Instant,
    pub deadline: Instant,
}

impl CandidateDecisionRequest {
    pub fn new(
        snapshot: CandidateSnapshot,
        observed_at: Instant,
        ttl: Duration,
    ) -> Result<Self, DecisionUnavailable> {
        if ttl.is_zero()
            || ttl > Duration::from_secs(30)
            || snapshot.goal.trim().is_empty()
            || snapshot.goal.len() > 4096
            || snapshot.scene_id.is_empty()
            || snapshot.frame_id.is_empty()
            || snapshot.scene_id.len() > 256
            || snapshot.frame_id.len() > 256
            || snapshot.candidates.is_empty()
            || snapshot.candidates.len() > MAX_CANDIDATES
        {
            return Err(DecisionUnavailable::InvalidInput);
        }
        let mut ids = BTreeSet::new();
        for candidate in &snapshot.candidates {
            let e = &candidate.element;
            let b = &e.bbox_norm;
            if !candidate.eligible
                || e.element_id.is_empty()
                || e.element_id.len() > 256
                || !ids.insert(&e.element_id)
                || !e.confidence.is_finite()
                || !(0.0..=1.0).contains(&e.confidence)
                || [b.x, b.y, b.width, b.height]
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                || e.label.len() > 2048
                || [
                    &e.role,
                    &e.intent,
                    &e.state,
                    &e.text_masked,
                    &e.parent_id,
                    &candidate.ranking_reason,
                ]
                .iter()
                .any(|v| v.as_ref().is_some_and(|s| s.len() > 2048))
            {
                return Err(DecisionUnavailable::InvalidInput);
            }
        }
        // GuiCandidate intentionally omits raw labels in serde. Bind them explicitly:
        // two different local scenes must not become identical after redaction.
        let labels: Vec<_> = snapshot
            .candidates
            .iter()
            .map(|c| c.element.label.as_str())
            .collect();
        let raw = serde_json::to_vec(&(
            DECISION_SCHEMA,
            &snapshot.goal,
            &snapshot.scene_id,
            &snapshot.frame_id,
            snapshot.generation,
            &snapshot.window,
            &snapshot.candidates,
            labels,
        ))
        .map_err(|_| DecisionUnavailable::InvalidInput)?;
        if raw.len() > 131_072 {
            return Err(DecisionUnavailable::InvalidInput);
        }
        let window_digest = window_digest(&snapshot.window)?;
        let binding = DecisionBinding {
            digest: digest_bytes(&raw),
            window_digest,
            observed_at,
            deadline: observed_at
                .checked_add(ttl)
                .ok_or(DecisionUnavailable::InvalidInput)?,
        };
        Ok(Self { snapshot, binding })
    }

    pub fn snapshot(&self) -> &CandidateSnapshot {
        &self.snapshot
    }
    pub fn binding(&self) -> &DecisionBinding {
        &self.binding
    }
}

impl fmt::Debug for CandidateDecisionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CandidateDecisionRequest")
            .field("candidate_count", &self.snapshot.candidates.len())
            .finish_non_exhaustive()
    }
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for &byte in digest.iter() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

pub fn window_digest(window: &WindowInfo) -> Result<String, DecisionUnavailable> {
    serde_json::to_vec(window)
        .map(|bytes| digest_bytes(&bytes))
        .map_err(|_| DecisionUnavailable::InvalidInput)
}

/// Minimal sanitized text, never an action, screenshot, AX tree, ticket or capability.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionText {
    pub goal: String,
    pub candidates: Vec<DecisionOption>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionOption {
    pub id: String,
    pub text: String,
    pub role: Option<String>,
    pub intent: Option<String>,
    pub state: Option<String>,
}

impl DecisionText {
    pub fn validate(&self) -> Result<(), DecisionUnavailable> {
        if self.goal.trim().is_empty()
            || self.candidates.is_empty()
            || self.candidates.len() > MAX_CANDIDATES
            || self
                .candidates
                .iter()
                .enumerate()
                .any(|(i, c)| c.id != format!("c{i}"))
            || serde_json::to_vec(self).map_or(true, |v| v.len() > MAX_PAYLOAD_BYTES)
        {
            return Err(DecisionUnavailable::InvalidInput);
        }
        Ok(())
    }
}

impl fmt::Debug for DecisionText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecisionText")
            .field("candidate_count", &self.candidates.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionUnavailable {
    Off,
    LocalOnly,
    ApprovalMissing,
    ConsentOrPolicyDenied,
    Stale,
    Expired,
    InvalidInput,
    CredentialUnavailable,
    AuditUnavailable,
    BudgetExceeded,
    Unauthorized,
    RateLimited,
    Overloaded,
    Rejected,
    Timeout,
    Transport,
    InvalidResponse,
    ResponseTooLarge,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionStage {
    Choice,
    Suitability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DecisionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct ChoiceEvidence {
    pub selected: String,
    pub probabilities: BTreeMap<String, f64>,
    /// Provider-reported Choice confidence, not the selected probability.
    pub confidence: f64,
}

#[derive(Debug, Clone)]
pub struct SuitabilityEvidence {
    pub selected: String,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct BackendResponse<T> {
    pub request_hash: String,
    pub value: T,
    pub observed_model: String,
    pub usage: DecisionUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendFailure {
    pub request_hash: Option<String>,
    pub usage: Option<DecisionUsage>,
    pub reason: DecisionUnavailable,
    pub attempted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceKind {
    ChoiceDistribution,
}

#[derive(Clone)]
pub enum CandidateDecision {
    Selected {
        candidate_id: String,
        selected_probability: f64,
        suitability: f64,
    },
    None,
    Delegate,
    Unavailable(DecisionUnavailable),
}

impl fmt::Debug for CandidateDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selected {
                selected_probability,
                suitability,
                ..
            } => f
                .debug_struct("Selected")
                .field("selected_probability", selected_probability)
                .field("suitability", suitability)
                .finish_non_exhaustive(),
            Self::None => f.write_str("None"),
            Self::Delegate => f.write_str("Delegate"),
            Self::Unavailable(reason) => f.debug_tuple("Unavailable").field(reason).finish(),
        }
    }
}

impl CandidateDecision {
    pub fn reason_code(&self) -> Option<&'static str> {
        match self {
            Self::None => Some("no_suitable_candidate"),
            Self::Delegate => Some("insufficient_evidence"),
            Self::Selected { .. } | Self::Unavailable(_) => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionAttempt {
    pub request_hash: Option<String>,
    pub id: Uuid,
    pub stage: DecisionStage,
    pub attempted: bool,
    pub elapsed_ms: u64,
    pub usage: Option<DecisionUsage>,
    /// Estimated from the approved price; unknown usage is never zero cost.
    pub estimated_cost_microusd: Option<u64>,
    pub failure: Option<DecisionUnavailable>,
}

#[derive(Debug, Clone)]
pub struct CandidateDecisionResult {
    pub decision: CandidateDecision,
    pub binding: DecisionBinding,
    pub provider: &'static str,
    pub rubric_revision: &'static str,
    pub sanitized_state_hash: Option<String>,
    pub requested_model: &'static str,
    pub observed_model: Option<String>,
    pub confidence_kind: ConfidenceKind,
    pub choice: Option<ChoiceEvidence>,
    pub attempts: Vec<DecisionAttempt>,
    /// Cache hits reference original attempts and never create new billed attempts.
    pub cache_source: Option<Uuid>,
    pub decision_id: Uuid,
}

impl CandidateDecisionResult {
    pub fn unavailable(binding: &DecisionBinding, reason: DecisionUnavailable) -> Self {
        Self {
            decision: CandidateDecision::Unavailable(reason),
            binding: binding.clone(),
            provider: "typesafe",
            rubric_revision: DECISION_RUBRIC,
            sanitized_state_hash: None,
            requested_model: JEV_MODEL,
            observed_model: None,
            confidence_kind: ConfidenceKind::ChoiceDistribution,
            choice: None,
            attempts: Vec::new(),
            cache_source: None,
            decision_id: Uuid::new_v4(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAuditPhase {
    BeforeSend,
    AfterSend,
    CacheHit,
}

/// Metadata-only durable audit. No arbitrary details or provider payload strings.
#[derive(Debug, Clone, Serialize)]
pub struct DecisionAuditContext {
    /// Hash of binding, sanitized input and non-secret authority revisions.
    pub namespace_hash: String,
    pub provider: &'static str,
    pub requested_model: &'static str,
    pub schema_revision: &'static str,
    pub rubric_revision: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionAuditRecord {
    pub id: Uuid,
    pub decision_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub phase: DecisionAuditPhase,
    pub stage: DecisionStage,
    pub attempt: Option<DecisionAttempt>,
    pub context: DecisionAuditContext,
    pub cache_source: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        intent::ElementBounds,
        ui_scene::{NormalizedBounds, UiSceneElement},
    };

    fn snapshot() -> CandidateSnapshot {
        CandidateSnapshot {
            goal: "Save".into(),
            scene_id: "scene".into(),
            frame_id: "frame".into(),
            generation: 1,
            window: WindowInfo {
                title: "Editor".into(),
                app_name: "Editor".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: None,
            },
            candidates: vec![GuiCandidate {
                eligible: true,
                ranking_reason: None,
                element: UiSceneElement {
                    element_id: "button".into(),
                    label: "alice@example.com".into(),
                    bbox_abs: ElementBounds {
                        x: 1,
                        y: 2,
                        width: 3,
                        height: 4,
                    },
                    bbox_norm: NormalizedBounds::new(0.1, 0.2, 0.3, 0.4),
                    role: Some("button".into()),
                    intent: None,
                    state: None,
                    confidence: 0.8,
                    text_masked: Some("[EMAIL]".into()),
                    parent_id: None,
                },
            }],
        }
    }

    #[test]
    fn raw_binding_distinguishes_redaction_collisions_and_every_source_revision() {
        let at = Instant::now();
        let original =
            CandidateDecisionRequest::new(snapshot(), at, Duration::from_secs(10)).unwrap();
        for mutate in [
            |s: &mut CandidateSnapshot| s.candidates[0].element.label = "bob@example.com".into(),
            |s: &mut CandidateSnapshot| s.goal.push('!'),
            |s: &mut CandidateSnapshot| s.scene_id.push('!'),
            |s: &mut CandidateSnapshot| s.frame_id.push('!'),
            |s: &mut CandidateSnapshot| s.generation += 1,
            |s: &mut CandidateSnapshot| s.candidates[0].element.bbox_abs.x += 1,
            |s: &mut CandidateSnapshot| s.window.pid += 1,
        ] {
            let mut changed = snapshot();
            mutate(&mut changed);
            let other =
                CandidateDecisionRequest::new(changed, at, Duration::from_secs(10)).unwrap();
            assert_ne!(original.binding(), other.binding());
        }
        let longer =
            CandidateDecisionRequest::new(snapshot(), at, Duration::from_secs(11)).unwrap();
        assert_ne!(original.binding(), longer.binding());
        let debug = format!("{original:?}");
        assert!(debug.contains("candidate_count: 1"));
        assert!(!debug.contains("alice"));
        assert_eq!(original.snapshot().goal, "Save");
        assert_eq!(original.binding().observed_at, at);
        assert_eq!(original.binding().deadline, at + Duration::from_secs(10));
    }

    #[test]
    fn malformed_candidates_do_not_form_a_binding() {
        let mut s = snapshot();
        s.candidates[0].element.bbox_norm.x = f32::NAN;
        assert_eq!(
            CandidateDecisionRequest::new(s, Instant::now(), Duration::from_secs(1)).unwrap_err(),
            DecisionUnavailable::InvalidInput
        );
        let mut s = snapshot();
        s.candidates.push(s.candidates[0].clone());
        assert_eq!(
            CandidateDecisionRequest::new(s, Instant::now(), Duration::from_secs(1)).unwrap_err(),
            DecisionUnavailable::InvalidInput
        );
        let mut s = snapshot();
        s.candidates[0].eligible = false;
        assert_eq!(
            CandidateDecisionRequest::new(s, Instant::now(), Duration::from_secs(1)).unwrap_err(),
            DecisionUnavailable::InvalidInput
        );
    }

    #[test]
    fn decision_debug_does_not_expose_local_candidate_identifiers() {
        let result = CandidateDecision::Selected {
            candidate_id: "private-local-id".into(),
            selected_probability: 0.8,
            suitability: 0.7,
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("Selected"));
        assert!(!debug.contains("private-local-id"));
    }

    fn wire_text(count: usize) -> DecisionText {
        DecisionText {
            goal: "Save changes".into(),
            candidates: (0..count)
                .map(|i| DecisionOption {
                    id: format!("c{i}"),
                    text: "Save".into(),
                    role: Some("button".into()),
                    intent: None,
                    state: None,
                })
                .collect(),
        }
    }

    #[test]
    fn wire_text_enforces_candidate_identity_and_inclusive_size_limits() {
        for count in [1, MAX_CANDIDATES] {
            assert_eq!(wire_text(count).validate(), Ok(()));
        }
        let mut invalid = vec![wire_text(0), wire_text(MAX_CANDIDATES + 1)];
        let mut empty_goal = wire_text(1);
        empty_goal.goal = " \t\n".into();
        invalid.push(empty_goal);
        let mut reordered = wire_text(2);
        reordered.candidates.swap(0, 1);
        invalid.push(reordered);
        let mut reserved = wire_text(1);
        reserved.candidates[0].id = NONE_OPTION.into();
        invalid.push(reserved);
        for text in invalid {
            assert_eq!(text.validate(), Err(DecisionUnavailable::InvalidInput));
        }

        let mut text = wire_text(1);
        let size = serde_json::to_vec(&text).unwrap().len();
        text.goal.push_str(&"x".repeat(MAX_PAYLOAD_BYTES - size));
        assert_eq!(serde_json::to_vec(&text).unwrap().len(), MAX_PAYLOAD_BYTES);
        assert_eq!(text.validate(), Ok(()));
        text.goal.push('x');
        assert_eq!(text.validate(), Err(DecisionUnavailable::InvalidInput));

        let debug = format!("{:?}", wire_text(1));
        assert!(debug.contains("candidate_count: 1"));
        assert!(!debug.contains("Save"));
    }

    #[test]
    fn digests_bind_exact_bytes_and_window_identity_independently() {
        assert_eq!(
            digest_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let window = snapshot().window;
        let digest = window_digest(&window).unwrap();
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, digest_bytes(&serde_json::to_vec(&window).unwrap()));
        let mut changed = window.clone();
        changed.pid += 1;
        assert_ne!(digest, window_digest(&changed).unwrap());
    }

    #[test]
    fn unavailable_is_distinct_from_deliberate_none_and_delegation() {
        assert_eq!(
            CandidateDecision::None.reason_code(),
            Some("no_suitable_candidate")
        );
        assert_eq!(
            CandidateDecision::Delegate.reason_code(),
            Some("insufficient_evidence")
        );
        let request =
            CandidateDecisionRequest::new(snapshot(), Instant::now(), Duration::from_secs(1))
                .unwrap();
        let result =
            CandidateDecisionResult::unavailable(request.binding(), DecisionUnavailable::Off);
        assert!(matches!(
            result.decision,
            CandidateDecision::Unavailable(DecisionUnavailable::Off)
        ));
        assert_eq!(result.decision.reason_code(), None);
        assert_eq!(result.binding, *request.binding());
        assert!(result.attempts.is_empty());
        assert_eq!(result.observed_model, None);
        assert_eq!(result.cache_source, None);
        assert_ne!(result.decision_id, Uuid::nil());
    }
}
