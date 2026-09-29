//! Pure construction selection and compatibility ports. No provider discovery.

use async_trait::async_trait;
use maekon_core::config::AiAccessMode;
use maekon_core::models::candidate_assessment::CandidateAssessmentResult;
use maekon_core::models::candidate_decision::{CandidateDecisionRequest, DecisionUnavailable};
#[cfg(feature = "analysis")]
use maekon_core::models::candidate_decision_policy::CandidateCostPolicy;
use maekon_core::models::candidate_decision_policy::{CandidateDecisionPolicy, CandidateProvider};
use maekon_core::ports::candidate_assessment::CandidateAssessmentPort;
use maekon_core::ports::candidate_decision::CandidateDecisionPort;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateAssessmentRoute {
    LocalRules,
    LocalModel,
    JevDirect,
}

/// Select construction before credentials or HTTP factories are consulted.
/// Success is not readiness, authentication, billing evidence, or permission.
pub fn candidate_assessment_route(
    policy: CandidateDecisionPolicy,
    mode: AiAccessMode,
) -> Result<CandidateAssessmentRoute, DecisionUnavailable> {
    match policy.provider {
        None => Err(DecisionUnavailable::Off),
        Some(CandidateProvider::LocalRules) => Ok(CandidateAssessmentRoute::LocalRules),
        Some(CandidateProvider::LocalModel) => {
            if mode != AiAccessMode::LocalModel {
                return Err(DecisionUnavailable::LocalOnly);
            }
            #[cfg(feature = "analysis")]
            {
                Ok(CandidateAssessmentRoute::LocalModel)
            }
            #[cfg(not(feature = "analysis"))]
            {
                Err(DecisionUnavailable::Rejected)
            }
        }
        Some(CandidateProvider::JevDirect) => {
            #[cfg(feature = "analysis")]
            {
                if mode != AiAccessMode::ProviderApiKey {
                    return Err(DecisionUnavailable::LocalOnly);
                }
                if policy.cost != CandidateCostPolicy::ExplicitPaidApiAllowed {
                    return Err(DecisionUnavailable::Rejected);
                }
                Ok(CandidateAssessmentRoute::JevDirect)
            }
            #[cfg(not(feature = "analysis"))]
            {
                Err(DecisionUnavailable::Rejected)
            }
        }
        Some(
            CandidateProvider::CodexCli
            | CandidateProvider::ClaudeCli
            | CandidateProvider::JevGateway,
        ) => Err(DecisionUnavailable::Rejected),
    }
}

/// Inert advisory fallback with no fabricated observation or provider attempt.
pub struct UnavailableCandidateAssessment(DecisionUnavailable);

impl UnavailableCandidateAssessment {
    pub fn new(reason: DecisionUnavailable) -> Self {
        Self(reason)
    }
}

#[async_trait]
impl CandidateAssessmentPort for UnavailableCandidateAssessment {
    async fn assess(&self, request: &CandidateDecisionRequest) -> CandidateAssessmentResult {
        CandidateAssessmentResult::unavailable(request.binding(), self.0)
    }
}

/// Lossless port compatibility. The injected legacy runtime retains its own
/// authority, privacy, audit and cost gates; this wrapper grants none of them.
pub struct JevCandidateAssessment(Arc<dyn CandidateDecisionPort>);

impl JevCandidateAssessment {
    pub fn new(provider: Arc<dyn CandidateDecisionPort>) -> Self {
        Self(provider)
    }
}

#[async_trait]
impl CandidateAssessmentPort for JevCandidateAssessment {
    async fn assess(&self, request: &CandidateDecisionRequest) -> CandidateAssessmentResult {
        CandidateAssessmentResult::from_jev(self.0.decide(request).await)
    }
}

#[cfg(test)]
#[path = "candidate_assessment_routing_tests.rs"]
mod tests;
