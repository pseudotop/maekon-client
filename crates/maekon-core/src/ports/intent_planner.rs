//! Intent planner port — contract for translating natural-language
//! intent hints into structured automation intents.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::intent::AutomationIntent;

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
}
