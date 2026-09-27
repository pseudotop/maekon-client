//! Provider-neutral advisory results. A selection is never an execution capability.

use std::fmt;
use std::time::Instant;

use serde::Serialize;
use uuid::Uuid;

use super::candidate_decision::{
    CandidateDecision, CandidateDecisionResult, ChoiceEvidence, ConfidenceKind, DecisionAttempt,
    DecisionBinding, DecisionUnavailable, DecisionUsage,
};
use super::candidate_decision_policy::CandidateProvider;

pub const ASSESSMENT_SCHEMA: &str = "candidate-assessment.v1";
pub const EXACT_LABEL_RULE: &str = "unique-exact-label.v1";

#[derive(Clone, PartialEq, Eq)]
pub enum AssessmentOutcome {
    Selected { candidate_id: String },
    None,
    Delegate,
    Unavailable(DecisionUnavailable),
}

impl fmt::Debug for AssessmentOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selected { .. } => f.write_str("Selected { .. }"),
            Self::None => f.write_str("None"),
            Self::Delegate => f.write_str("Delegate"),
            Self::Unavailable(reason) => f.debug_tuple("Unavailable").field(reason).finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentTransport {
    InProcess,
    LoopbackHttp,
    OfficialCli,
    DirectHttps,
    GatewayHttps,
}

/// Requested routing and actually observed inference identity are distinct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssessmentProvenance {
    pub provider: CandidateProvider,
    pub transport: AssessmentTransport,
    pub inference_provider: String,
    pub requested_model: Option<String>,
    pub observed_model: Option<String>,
    pub model_revision: Option<String>,
    pub rubric_revision: String,
}

impl AssessmentProvenance {
    pub fn local_rules() -> Self {
        Self {
            provider: CandidateProvider::LocalRules,
            transport: AssessmentTransport::InProcess,
            inference_provider: "maekon".into(),
            requested_model: None,
            observed_model: None,
            model_revision: None,
            rubric_revision: EXACT_LABEL_RULE.into(),
        }
    }
}

/// Only Jev evidence carries its measured distribution and selected suitability.
/// Rule matches and categorical model output are not calibrated probabilities.
#[derive(Debug, Clone)]
pub enum AssessmentEvidence {
    NotObserved,
    LocalHeuristic {
        exact_matches: usize,
    },
    ModelReported,
    GatewayDistribution {
        choice: super::candidate_gateway::GatewayChoice,
        suitability: Option<f64>,
    },
    JevDistribution {
        choice: Option<ChoiceEvidence>,
        confidence_kind: ConfidenceKind,
        selected_probability: Option<f64>,
        suitability: Option<f64>,
    },
}

/// Preserve existing Jev attempts verbatim, including unknown usage/cost.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum AssessmentAttempt {
    JevV1(DecisionAttempt),
    Local(LocalAssessmentAttempt),
    Gateway(super::candidate_gateway::GatewayAttempt),
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalAssessmentAttempt {
    pub id: Uuid,
    pub request_hash: Option<String>,
    pub attempted: bool,
    pub elapsed_ms: u64,
    pub usage: Option<DecisionUsage>,
    pub failure: Option<DecisionUnavailable>,
}

#[derive(Debug, Clone)]
pub struct CandidateAssessmentResult {
    pub outcome: AssessmentOutcome,
    pub binding: DecisionBinding,
    pub provenance: Option<AssessmentProvenance>,
    pub evidence: AssessmentEvidence,
    pub sanitized_state_hash: Option<String>,
    pub attempts: Vec<AssessmentAttempt>,
    pub cache_source: Option<Uuid>,
    pub decision_id: Uuid,
}

impl CandidateAssessmentResult {
    pub fn unavailable(binding: &DecisionBinding, reason: DecisionUnavailable) -> Self {
        Self {
            outcome: AssessmentOutcome::Unavailable(reason),
            binding: binding.clone(),
            provenance: None,
            evidence: AssessmentEvidence::NotObserved,
            sanitized_state_hash: None,
            attempts: Vec::new(),
            cache_source: None,
            decision_id: Uuid::new_v4(),
        }
    }

    /// Lossless v1 projection: every original field is explicitly consumed.
    /// The guarded v1 provider still owns its authorization and two-stage protocol.
    pub fn from_jev(result: CandidateDecisionResult) -> Self {
        let CandidateDecisionResult {
            decision,
            binding,
            provider,
            rubric_revision,
            sanitized_state_hash,
            requested_model,
            observed_model,
            confidence_kind,
            choice,
            attempts,
            cache_source,
            decision_id,
        } = result;
        let (outcome, selected_probability, suitability) = match decision {
            CandidateDecision::Selected {
                candidate_id,
                selected_probability,
                suitability,
            } => (
                AssessmentOutcome::Selected { candidate_id },
                Some(selected_probability),
                Some(suitability),
            ),
            CandidateDecision::None => (AssessmentOutcome::None, None, None),
            CandidateDecision::Delegate => (AssessmentOutcome::Delegate, None, None),
            CandidateDecision::Unavailable(reason) => {
                (AssessmentOutcome::Unavailable(reason), None, None)
            }
        };
        Self {
            outcome,
            binding,
            provenance: Some(AssessmentProvenance {
                provider: CandidateProvider::JevDirect,
                transport: AssessmentTransport::DirectHttps,
                inference_provider: provider.into(),
                requested_model: Some(requested_model.into()),
                observed_model,
                // v1 pins the model name; it does not attest a weights digest.
                model_revision: None,
                rubric_revision: rubric_revision.into(),
            }),
            evidence: AssessmentEvidence::JevDistribution {
                choice,
                confidence_kind,
                selected_probability,
                suitability,
            },
            sanitized_state_hash,
            attempts: attempts.into_iter().map(AssessmentAttempt::JevV1).collect(),
            cache_source,
            decision_id,
        }
    }
}

/// Trusted local daemon approval, never deserializable from an IPC request.
/// The caller must verify the daemon/configuration reference; loopback alone
/// cannot establish local inference. A fresh runtime is required after changes.
#[derive(Clone)]
pub struct LocalModelApproval {
    pub daemon_reference: String,
    pub configuration_revision: String,
    pub endpoint_origin: String,
    pub model: String,
    pub model_digest: String,
    pub expires_at: Instant,
}

impl LocalModelApproval {
    pub fn validate(&self, now: Instant) -> Result<(), DecisionUnavailable> {
        if [
            &self.daemon_reference,
            &self.configuration_revision,
            &self.endpoint_origin,
            &self.model,
        ]
        .iter()
        .any(|v| v.trim().is_empty() || v.len() > 256)
            || self.model.chars().any(char::is_control)
            || !self
                .model_digest
                .strip_prefix("sha256:")
                .is_some_and(|digest| {
                    digest.len() == 64
                        && digest
                            .bytes()
                            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                })
            || now >= self.expires_at
            || self.expires_at.duration_since(now) > std::time::Duration::from_secs(3600)
        {
            return Err(DecisionUnavailable::ApprovalMissing);
        }
        Ok(())
    }
}

/// Opaque option IDs only. Neither model output nor rule output contains actions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalAssessmentChoice {
    Selected(String),
    None,
    Delegate,
}

#[derive(Debug, Clone)]
pub struct LocalAssessmentAnswer {
    pub choice: LocalAssessmentChoice,
    pub request_hash: String,
    pub observed_model: String,
    pub model_digest: String,
    pub usage: Option<DecisionUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssessmentAuditPhase {
    BeforeAttempt,
    AfterAttempt,
    CacheHit,
}

/// Metadata only: no raw prompt, local candidate IDs or provider reply.
#[derive(Debug, Clone, Serialize)]
pub struct AssessmentAuditRecord {
    pub id: Uuid,
    pub decision_id: Uuid,
    pub attempt_id: Option<Uuid>,
    pub phase: AssessmentAuditPhase,
    pub namespace_hash: String,
    pub provider: CandidateProvider,
    pub transport: AssessmentTransport,
    pub schema_revision: &'static str,
    pub attempt: Option<LocalAssessmentAttempt>,
    pub cache_source: Option<Uuid>,
}

#[cfg(test)]
#[path = "candidate_assessment_tests.rs"]
mod tests;
