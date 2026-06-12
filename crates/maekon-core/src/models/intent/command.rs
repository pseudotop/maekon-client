//! Intent command types: `IntentCommand`, `IntentConfig`, `IntentResult`,
//! `VerificationResult`.

use serde::{Deserialize, Serialize};

use super::automation::AutomationIntent;
use super::elements::UiElement;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentCommand {
    pub command_id: String,
    pub session_id: String,
    pub intent: AutomationIntent,
    pub config: Option<IntentConfig>,
    pub timeout_ms: Option<u64>,
    pub policy_token: String,
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
