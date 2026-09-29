//! Provider eligibility for candidate recommendations (#12455).
//!
//! This pure policy neither discovers providers nor authorizes egress/execution.
//! Runtime adapters must independently validate fresh account/model observations,
//! consent, privacy, binding, audit, quotas and deadlines before every attempt.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::AiAccessMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateProvider {
    LocalRules,
    LocalModel,
    CodexCli,
    ClaudeCli,
    JevDirect,
    JevGateway,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateCostPolicy {
    #[default]
    NoAdditionalApiCost,
    /// Still requires a current price, bounded approval and atomic reservation.
    ExplicitPaidApiAllowed,
}

/// Independent of the global AI mode: missing candidate configuration stays off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CandidateDecisionPolicy {
    pub provider: Option<CandidateProvider>,
    pub cost: CandidateCostPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateTrigger {
    UserRequest,
    Background,
}

/// Trusted runtime observations, never a deserializable client assertion.
/// An authenticated CLI alone cannot establish its billing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateAuthentication {
    NotRequired,
    Verified,
    Unknown,
    LoginRequired,
}

/// Describes the selected account/endpoint/model, not a portable cost approval.
/// Unknown usage and consumed credits must not be reported as zero cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateBilling {
    Unknown,
    Local,
    IncludedSubscription,
    MeteredApi,
    /// A verified zero-inference-price offer, bound to this route and account.
    /// Runtime derives this bound from a current wall-clock check, rechecks on
    /// resume, and never extends the original request or authorization deadline.
    Promotion {
        valid_until: Instant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateReadiness {
    Ready,
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
pub struct CandidateProviderObservation {
    pub provider: CandidateProvider,
    pub readiness: CandidateReadiness,
    pub authentication: CandidateAuthentication,
    pub billing: CandidateBilling,
}

#[derive(Debug, Clone, Copy)]
pub struct CandidatePolicyContext {
    pub access_mode: AiAccessMode,
    pub trigger: CandidateTrigger,
    /// Current managed/user policy; this is only one of the runtime gates.
    pub permitted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidatePolicyDenial {
    Off,
    PolicyDenied,
    ProviderMismatch,
    AccessModeMismatch,
    UserRequestRequired,
    ProviderUnavailable,
    AuthenticationUnverified,
    UnknownCost,
    BillingModeMismatch,
    AdditionalApiCostDenied,
    PromotionExpired,
}

impl CandidateDecisionPolicy {
    /// Checks route eligibility only; success is not an execution/egress ticket.
    /// No provider fallback, credential lookup, retry or model call occurs here.
    pub fn check_eligibility(
        &self,
        observed: CandidateProviderObservation,
        context: CandidatePolicyContext,
        now: Instant,
    ) -> Result<(), CandidatePolicyDenial> {
        use CandidatePolicyDenial as Denial;
        use CandidateProvider as Provider;

        let selected = self.provider.ok_or(Denial::Off)?;
        if !context.permitted {
            return Err(Denial::PolicyDenied);
        }
        if selected != observed.provider {
            return Err(Denial::ProviderMismatch);
        }

        match (selected, context.access_mode) {
            (Provider::LocalRules, _)
            | (Provider::LocalModel, AiAccessMode::LocalModel)
            | (Provider::CodexCli | Provider::ClaudeCli, AiAccessMode::ProviderSubscriptionCli)
            | (Provider::JevDirect | Provider::JevGateway, AiAccessMode::ProviderApiKey) => {}
            _ => return Err(Denial::AccessModeMismatch),
        }
        if matches!(selected, Provider::CodexCli | Provider::ClaudeCli)
            && context.trigger != CandidateTrigger::UserRequest
        {
            return Err(Denial::UserRequestRequired);
        }
        if observed.readiness != CandidateReadiness::Ready {
            return Err(Denial::ProviderUnavailable);
        }
        match (selected, observed.authentication) {
            (Provider::LocalRules | Provider::LocalModel, CandidateAuthentication::NotRequired)
            | (
                Provider::CodexCli
                | Provider::ClaudeCli
                | Provider::JevDirect
                | Provider::JevGateway,
                CandidateAuthentication::Verified,
            ) => {}
            _ => return Err(Denial::AuthenticationUnverified),
        }

        match (selected, observed.billing) {
            (_, CandidateBilling::Unknown) => Err(Denial::UnknownCost),
            (Provider::LocalRules | Provider::LocalModel, CandidateBilling::Local)
            | (Provider::CodexCli | Provider::ClaudeCli, CandidateBilling::IncludedSubscription) => {
                Ok(())
            }
            (
                Provider::JevDirect | Provider::JevGateway,
                CandidateBilling::Promotion { valid_until },
            ) => {
                if now < valid_until {
                    Ok(())
                } else {
                    // Expiry never upgrades this offer to metered billing.
                    Err(Denial::PromotionExpired)
                }
            }
            (Provider::JevDirect | Provider::JevGateway, CandidateBilling::MeteredApi) => {
                match self.cost {
                    CandidateCostPolicy::ExplicitPaidApiAllowed => Ok(()),
                    CandidateCostPolicy::NoAdditionalApiCost => {
                        Err(Denial::AdditionalApiCostDenied)
                    }
                }
            }
            _ => Err(Denial::BillingModeMismatch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const PROVIDERS: [CandidateProvider; 6] = [
        CandidateProvider::LocalRules,
        CandidateProvider::LocalModel,
        CandidateProvider::CodexCli,
        CandidateProvider::ClaudeCli,
        CandidateProvider::JevDirect,
        CandidateProvider::JevGateway,
    ];
    const MODES: [AiAccessMode; 4] = [
        AiAccessMode::LocalModel,
        AiAccessMode::ProviderSubscriptionCli,
        AiAccessMode::ProviderApiKey,
        AiAccessMode::ProviderOAuth,
    ];

    fn ready(
        provider: CandidateProvider,
    ) -> (
        CandidateDecisionPolicy,
        CandidateProviderObservation,
        CandidatePolicyContext,
    ) {
        let (access_mode, authentication, billing) = match provider {
            CandidateProvider::LocalRules | CandidateProvider::LocalModel => (
                AiAccessMode::LocalModel,
                CandidateAuthentication::NotRequired,
                CandidateBilling::Local,
            ),
            CandidateProvider::CodexCli | CandidateProvider::ClaudeCli => (
                AiAccessMode::ProviderSubscriptionCli,
                CandidateAuthentication::Verified,
                CandidateBilling::IncludedSubscription,
            ),
            CandidateProvider::JevDirect | CandidateProvider::JevGateway => (
                AiAccessMode::ProviderApiKey,
                CandidateAuthentication::Verified,
                CandidateBilling::MeteredApi,
            ),
        };
        (
            CandidateDecisionPolicy {
                provider: Some(provider),
                cost: CandidateCostPolicy::ExplicitPaidApiAllowed,
            },
            CandidateProviderObservation {
                provider,
                readiness: CandidateReadiness::Ready,
                authentication,
                billing,
            },
            CandidatePolicyContext {
                access_mode,
                trigger: CandidateTrigger::UserRequest,
                permitted: true,
            },
        )
    }

    #[test]
    fn explicit_routes_have_positive_controls_and_no_cross_provider_fallback() {
        for provider in PROVIDERS {
            let (policy, observation, context) = ready(provider);
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                Ok(()),
                "{provider:?}"
            );
            for alternative in PROVIDERS {
                if alternative != provider {
                    let (_, alternative_observation, _) = ready(alternative);
                    assert_eq!(
                        policy.check_eligibility(alternative_observation, context, Instant::now()),
                        Err(CandidatePolicyDenial::ProviderMismatch)
                    );
                }
            }
        }
    }

    #[test]
    fn local_and_subscription_routes_need_no_paid_api_permission() {
        for provider in [
            CandidateProvider::LocalRules,
            CandidateProvider::LocalModel,
            CandidateProvider::CodexCli,
            CandidateProvider::ClaudeCli,
        ] {
            let (mut policy, observation, context) = ready(provider);
            policy.cost = CandidateCostPolicy::NoAdditionalApiCost;
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                Ok(()),
                "{provider:?}"
            );
        }
    }

    #[test]
    fn absent_configuration_is_off_and_explicit_deny_cannot_be_overridden() {
        let empty: CandidateDecisionPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, CandidateDecisionPolicy::default());
        assert_eq!(empty.cost, CandidateCostPolicy::NoAdditionalApiCost);
        for provider in PROVIDERS {
            let (policy, observation, mut context) = ready(provider);
            assert_eq!(
                empty.check_eligibility(observation, context, Instant::now()),
                Err(CandidatePolicyDenial::Off)
            );
            context.permitted = false;
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                Err(CandidatePolicyDenial::PolicyDenied)
            );
        }
    }

    #[test]
    fn access_modes_do_not_treat_cloud_cli_as_local() {
        for provider in PROVIDERS {
            let (policy, observation, mut context) = ready(provider);
            let configured_mode = context.access_mode;
            for mode in MODES {
                context.access_mode = mode;
                let expected =
                    if provider == CandidateProvider::LocalRules || mode == configured_mode {
                        Ok(())
                    } else {
                        Err(CandidatePolicyDenial::AccessModeMismatch)
                    };
                assert_eq!(
                    policy.check_eligibility(observation, context, Instant::now()),
                    expected,
                    "{provider:?} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn background_cli_is_denied_but_local_background_remains_possible() {
        for provider in PROVIDERS {
            let (policy, observation, mut context) = ready(provider);
            context.trigger = CandidateTrigger::Background;
            let expected = match provider {
                CandidateProvider::CodexCli | CandidateProvider::ClaudeCli => {
                    Err(CandidatePolicyDenial::UserRequestRequired)
                }
                _ => Ok(()),
            };
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                expected
            );
        }
    }

    #[test]
    fn unavailable_and_unverified_observations_stay_unavailable() {
        for provider in PROVIDERS {
            let (policy, observation, context) = ready(provider);
            let unavailable = CandidateProviderObservation {
                readiness: CandidateReadiness::Unavailable,
                ..observation
            };
            assert_eq!(
                policy.check_eligibility(unavailable, context, Instant::now()),
                Err(CandidatePolicyDenial::ProviderUnavailable)
            );
            for authentication in [
                CandidateAuthentication::NotRequired,
                CandidateAuthentication::Verified,
                CandidateAuthentication::Unknown,
                CandidateAuthentication::LoginRequired,
            ] {
                let changed = CandidateProviderObservation {
                    authentication,
                    ..observation
                };
                let expected = if authentication == observation.authentication {
                    Ok(())
                } else {
                    Err(CandidatePolicyDenial::AuthenticationUnverified)
                };
                assert_eq!(
                    policy.check_eligibility(changed, context, Instant::now()),
                    expected,
                    "{provider:?} {authentication:?}"
                );
            }
        }
    }

    #[test]
    fn authentication_does_not_prove_subscription_or_zero_cost() {
        for provider in PROVIDERS {
            let (policy, mut observation, context) = ready(provider);
            observation.billing = CandidateBilling::Unknown;
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                Err(CandidatePolicyDenial::UnknownCost)
            );
        }
        for provider in [CandidateProvider::CodexCli, CandidateProvider::ClaudeCli] {
            let (policy, mut observation, context) = ready(provider);
            for billing in [
                CandidateBilling::MeteredApi,
                CandidateBilling::Local,
                CandidateBilling::Promotion {
                    valid_until: Instant::now() + Duration::from_secs(60),
                },
            ] {
                observation.billing = billing;
                assert_eq!(
                    policy.check_eligibility(observation, context, Instant::now()),
                    Err(CandidatePolicyDenial::BillingModeMismatch)
                );
            }
        }
    }

    #[test]
    fn metered_jev_needs_explicit_cost_policy_without_silent_upgrade() {
        for provider in [CandidateProvider::JevDirect, CandidateProvider::JevGateway] {
            let (mut policy, observation, context) = ready(provider);
            policy.cost = CandidateCostPolicy::NoAdditionalApiCost;
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                Err(CandidatePolicyDenial::AdditionalApiCostDenied)
            );
            policy.cost = CandidateCostPolicy::ExplicitPaidApiAllowed;
            assert_eq!(
                policy.check_eligibility(observation, context, Instant::now()),
                Ok(())
            );
        }
    }

    #[test]
    fn promotion_expires_at_the_exact_boundary_even_with_paid_permission() {
        let deadline = Instant::now() + Duration::from_secs(60);
        for provider in [CandidateProvider::JevDirect, CandidateProvider::JevGateway] {
            let (mut policy, mut observation, context) = ready(provider);
            observation.billing = CandidateBilling::Promotion {
                valid_until: deadline,
            };
            for cost in [
                CandidateCostPolicy::NoAdditionalApiCost,
                CandidateCostPolicy::ExplicitPaidApiAllowed,
            ] {
                policy.cost = cost;
                assert_eq!(
                    policy.check_eligibility(
                        observation,
                        context,
                        deadline - Duration::from_nanos(1)
                    ),
                    Ok(())
                );
                assert_eq!(
                    policy.check_eligibility(observation, context, deadline),
                    Err(CandidatePolicyDenial::PromotionExpired)
                );
                assert_eq!(
                    policy.check_eligibility(
                        observation,
                        context,
                        deadline + Duration::from_nanos(1)
                    ),
                    Err(CandidatePolicyDenial::PromotionExpired)
                );
            }
        }
    }

    #[test]
    fn local_and_jev_routes_reject_incompatible_billing_observations() {
        let now = Instant::now();
        for provider in [CandidateProvider::LocalRules, CandidateProvider::LocalModel] {
            let (policy, mut observation, context) = ready(provider);
            for billing in [
                CandidateBilling::MeteredApi,
                CandidateBilling::IncludedSubscription,
                CandidateBilling::Promotion {
                    valid_until: now + Duration::from_secs(60),
                },
            ] {
                observation.billing = billing;
                assert_eq!(
                    policy.check_eligibility(observation, context, now),
                    Err(CandidatePolicyDenial::BillingModeMismatch)
                );
            }
        }
        for provider in [CandidateProvider::JevDirect, CandidateProvider::JevGateway] {
            let (policy, mut observation, context) = ready(provider);
            for billing in [
                CandidateBilling::Local,
                CandidateBilling::IncludedSubscription,
            ] {
                observation.billing = billing;
                assert_eq!(
                    policy.check_eligibility(observation, context, now),
                    Err(CandidatePolicyDenial::BillingModeMismatch)
                );
            }
        }
    }

    #[test]
    fn persisted_policy_rejects_unknown_provider_cost_and_fields() {
        for (document, expected) in [
            (r#"{"provider":"unknown"}"#, "unknown variant `unknown`"),
            (
                r#"{"provider":"codex_cli","cost":"free"}"#,
                "unknown variant `free`",
            ),
            (
                r#"{"provider":"jev_gateway","fallback":"jev_direct"}"#,
                "unknown field `fallback`",
            ),
        ] {
            let error = serde_json::from_str::<CandidateDecisionPolicy>(document)
                .expect_err("unknown policy values must fail parsing");
            assert_eq!(error.classify(), serde_json::error::Category::Data);
            assert!(error.to_string().contains(expected), "{error}");
        }
        for provider in PROVIDERS {
            let (policy, _, _) = ready(provider);
            let document = serde_json::to_string(&policy).unwrap();
            assert_eq!(
                serde_json::from_str::<CandidateDecisionPolicy>(&document).unwrap(),
                policy
            );
        }
        let policy: CandidateDecisionPolicy =
            serde_json::from_str(r#"{"provider":"codex_cli"}"#).unwrap();
        assert_eq!(policy.provider, Some(CandidateProvider::CodexCli));
        assert_eq!(policy.cost, CandidateCostPolicy::NoAdditionalApiCost);
    }
}
