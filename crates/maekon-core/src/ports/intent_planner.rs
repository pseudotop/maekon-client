//! Intent planner port — contract for translating natural-language
//! intent hints into structured automation intents.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::intent::AutomationIntent;
use crate::models::intent_execution::{
    IntentExecutionObserver, IntentPlanningOutcome, IntentSafeReason,
};

/// Translates natural-language intent hints into structured automation intents.
///
/// # Errors
/// - `CoreError::Analysis` (wire: `provider.analysis_failed`) for
///   LLM-side planning failures: empty output, non-parseable plan,
///   schema-violating intent fields.
/// - HTTP-layer failures follow the canonical semantic status mapping:
///   `CoreError::Auth` (401/403), `CoreError::RequestTimeout` (408/504),
///   `CoreError::RateLimit` (429), `CoreError::ServiceUnavailable`
///   (502/503). See `docs/guides/http-status-error-mapping.md`.
/// - `CoreError::Network` (wire: `network.generic`) for pre-response
///   transport failures (DNS, connection refused).
/// - Caller-side gate: "IntentPlanner is not configured" surfaces as
///   `CoreError::Config { code: ConfigCode::Missing }` from the
///   dispatch layer (iter-100), not inside this port's impls.
#[async_trait]
pub trait IntentPlanner: Send + Sync {
    async fn plan(&self, intent_hint: &str) -> Result<AutomationIntent, CoreError>;

    /// Legacy planners preserve their behavior without inventing provider facts.
    async fn plan_observed(
        &self,
        intent_hint: &str,
        observer: &IntentExecutionObserver,
    ) -> Result<AutomationIntent, CoreError> {
        observer.mark_legacy();
        let result = self.plan(intent_hint).await;
        match &result {
            Ok(_) => observer.planning_finished(
                IntentPlanningOutcome::Unknown,
                Some(IntentSafeReason::LegacyUnknown),
            ),
            Err(error) => observer.planning_finished(
                IntentPlanningOutcome::Error,
                Some(IntentSafeReason::from_error(error)),
            ),
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::intent_execution::{IntentObservation, IntentSubmission, OpaqueIntentId};

    struct LegacyPlanner;

    #[async_trait]
    impl IntentPlanner for LegacyPlanner {
        async fn plan(&self, hint: &str) -> Result<AutomationIntent, CoreError> {
            if hint == "unavailable" {
                Err(CoreError::Network {
                    code: crate::error_codes::NetworkCode::Generic,
                    message: "private-error-canary".into(),
                })
            } else {
                Ok(AutomationIntent::ActivateApp {
                    app_name: hint.into(),
                })
            }
        }
    }

    #[tokio::test]
    async fn legacy_planner_preserves_the_result_without_fabricating_provider_observations() {
        for hint in ["app-canary", "unavailable"] {
            let observer = IntentExecutionObserver::new(IntentSubmission {
                submission_id: Some(OpaqueIntentId::new()),
                retry_of: None,
            });
            let result = LegacyPlanner.plan_observed(hint, &observer).await;
            assert_eq!(observer.snapshot().observation, IntentObservation::Partial);
            observer.finish(
                crate::models::intent_execution::IntentActionOutcome::NotStarted,
                None,
            );
            let receipt = observer.snapshot();
            assert_eq!(receipt.observation, IntentObservation::LegacyUnknown);
            assert!(receipt.attempts.is_empty());
            if hint == "app-canary" {
                assert!(
                    matches!(result.unwrap(), AutomationIntent::ActivateApp { app_name } if app_name == hint)
                );
                assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Unknown);
                assert_eq!(
                    receipt.observation_reason,
                    Some(IntentSafeReason::LegacyUnknown)
                );
            } else {
                assert_eq!(result.unwrap_err().code(), "network.generic");
                assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Error);
                assert_eq!(
                    receipt.observation_reason,
                    Some(IntentSafeReason::Unavailable)
                );
            }
            let wire = serde_json::to_string(&receipt).unwrap();
            assert!(!wire.contains("app-canary"));
            assert!(!wire.contains("private-error-canary"));
        }
    }
}
