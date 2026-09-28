//! Provider routing before optional HTTP factories and credential discovery.

use maekon_core::config::AiAccessMode;
use maekon_core::models::candidate_decision::DecisionUnavailable;
use maekon_core::models::candidate_decision_policy::CandidateDecisionPolicy;
use maekon_core::ports::candidate_assessment::{CandidateAssessmentPort, LocalCandidateModelPort};
use maekon_core::ports::secret_store::SecretStore;
use std::sync::Arc;

use super::local_candidate_assessment::{LocalAssessmentControl, LocalCandidateAssessment};
#[cfg(feature = "analysis")]
use super::JevCandidateAssessment;
use super::{
    candidate_assessment_route, CandidateAssessmentRoute, ExternalOcrPrivacyGuard,
    UnavailableCandidateAssessment,
};

pub struct CandidateAssessmentRuntime {
    pub provider: Arc<dyn CandidateAssessmentPort>,
    pub control: AssessmentRuntimeControl,
}

pub enum AssessmentRuntimeControl {
    Off,
    Local(LocalAssessmentControl),
    #[cfg(feature = "analysis")]
    Jev(super::CandidateDecisionControl),
}

fn unavailable(reason: DecisionUnavailable) -> CandidateAssessmentRuntime {
    CandidateAssessmentRuntime {
        provider: Arc::new(UnavailableCandidateAssessment::new(reason)),
        control: AssessmentRuntimeControl::Off,
    }
}

/// Build an inert runtime. Callers must bind and explicitly approve it before
/// use. A missing provider always preserves the advisory/manual fallback contract.
/// No consumer, model download, daemon startup or OS execution is wired here.
pub fn build_candidate_assessment_runtime(
    policy: CandidateDecisionPolicy,
    mode: AiAccessMode,
    privacy: ExternalOcrPrivacyGuard,
    storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    local_endpoint: Option<&str>,
    secrets: Option<Arc<dyn SecretStore>>,
) -> CandidateAssessmentRuntime {
    let route = match candidate_assessment_route(policy, mode) {
        Ok(route) => route,
        Err(reason) => return unavailable(reason),
    };
    match route {
        CandidateAssessmentRoute::LocalRules | CandidateAssessmentRoute::LocalModel => {
            let model: Option<Arc<dyn LocalCandidateModelPort>> =
                if route == CandidateAssessmentRoute::LocalModel {
                    #[cfg(feature = "analysis")]
                    {
                        let Some(endpoint) = local_endpoint else {
                            return unavailable(DecisionUnavailable::Rejected);
                        };
                        let Ok(client) =
                            maekon_network::local_candidate_model::LocalCandidateModelClient::new(
                                endpoint,
                            )
                        else {
                            return unavailable(DecisionUnavailable::Transport);
                        };
                        Some(Arc::new(client))
                    }
                    #[cfg(not(feature = "analysis"))]
                    {
                        let _ = local_endpoint;
                        return unavailable(DecisionUnavailable::Rejected);
                    }
                } else {
                    None
                };
            let audit = Arc::new(maekon_storage::sqlite::SqliteCandidateDecisionAudit::new(
                storage,
            ));
            let (provider, control) =
                LocalCandidateAssessment::new(policy, mode, privacy, audit, model);
            CandidateAssessmentRuntime {
                provider: Arc::new(provider),
                control: AssessmentRuntimeControl::Local(control),
            }
        }
        CandidateAssessmentRoute::JevDirect => {
            #[cfg(feature = "analysis")]
            {
                let Some(secrets) = secrets else {
                    return unavailable(DecisionUnavailable::CredentialUnavailable);
                };
                match super::build_candidate_decision_runtime(privacy, storage, secrets, mode) {
                    Ok((provider, control)) => CandidateAssessmentRuntime {
                        provider: Arc::new(JevCandidateAssessment::new(provider)),
                        control: AssessmentRuntimeControl::Jev(control),
                    },
                    Err(_) => unavailable(DecisionUnavailable::Transport),
                }
            }
            #[cfg(not(feature = "analysis"))]
            {
                let _ = secrets;
                unavailable(DecisionUnavailable::Rejected)
            }
        }
    }
}
