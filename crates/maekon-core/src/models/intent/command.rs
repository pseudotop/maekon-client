//! Intent command types: `IntentCommand`, `IntentConfig`, `IntentResult`,
//! `VerificationResult`.

use serde::{Deserialize, Serialize};

use super::automation::AutomationIntent;
use super::elements::UiElement;
use crate::models::automation::CommandOrigin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentResult {
    pub success: bool,
    pub element: Option<UiElement>,
    pub verification: Option<VerificationResult>,
    pub retry_count: u32,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    pub screen_changed: bool,
    pub changed_regions: usize,
    pub text_found: Option<bool>,
}

// NOTE: Debug is hand-written (not derived) to mask `policy_token` (#7600).
// This is a signed capability token (see `maekon-automation::policy`, which
// already refuses to log it raw — `policy_token_fingerprint` is the sanctioned
// log surface); a derived Debug would emit it verbatim under any `{:?}`, so a
// single error-path `?command` could leak it to a file/OTel log sink.
#[derive(Clone, Serialize, Deserialize)]
pub struct IntentCommand {
    pub command_id: String,
    pub session_id: String,
    pub intent: AutomationIntent,
    pub config: Option<IntentConfig>,
    pub timeout_ms: Option<u64>,
    pub policy_token: String,
    /// #6333 A20: provenance marker; not serialized (deserialized commands are External).
    #[serde(skip)]
    pub origin: CommandOrigin,
}

impl std::fmt::Debug for IntentCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IntentCommand")
            .field("command_id", &self.command_id)
            .field("session_id", &self.session_id)
            .field("intent", &self.intent)
            .field("config", &self.config)
            .field("timeout_ms", &self.timeout_ms)
            .field("policy_token", &"[REDACTED]")
            .field("origin", &self.origin)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentConfig {
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_retry_interval_ms")]
    pub retry_interval_ms: u64,
    #[serde(default = "default_verify")]
    pub verify_after_action: bool,
    #[serde(default = "default_verify_delay_ms")]
    pub verify_delay_ms: u64,
}

impl Default for IntentConfig {
    fn default() -> Self {
        Self {
            min_confidence: default_min_confidence(),
            max_retries: default_max_retries(),
            retry_interval_ms: default_retry_interval_ms(),
            verify_after_action: default_verify(),
            verify_delay_ms: default_verify_delay_ms(),
        }
    }
}

fn default_min_confidence() -> f64 {
    0.7
}
fn default_max_retries() -> u32 {
    3
}
fn default_retry_interval_ms() -> u64 {
    500
}
fn default_verify() -> bool {
    true
}
fn default_verify_delay_ms() -> u64 {
    1000
}
