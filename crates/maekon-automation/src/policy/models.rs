use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use maekon_core::config::SandboxProfile;

// AuditLevel — canonical definition in maekon-core, re-exported here for backward compat
pub use maekon_core::models::audit::AuditLevel;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub policy_id: String,
    pub process_name: String,
    pub process_hash: Option<String>,
    pub allowed_args: Vec<String>,
    pub requires_sudo: bool,
    pub max_execution_time_ms: u64,
    #[serde(default)]
    pub audit_level: AuditLevel,
    #[serde(default)]
    pub sandbox_profile: Option<SandboxProfile>,
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    #[serde(default)]
    pub allow_network: Option<bool>,
    #[serde(default = "default_require_signed_token")]
    pub require_signed_token: bool,
    #[serde(default)]
    pub confirmation: maekon_core::config::ConfirmationRequirement,
}

/// #6333 A16: policy tokens must be signed unless a policy explicitly opts out.
fn default_require_signed_token() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct PolicyCache {
    pub policies: Vec<ExecutionPolicy>,
    pub last_updated: DateTime<Utc>,
    pub ttl_seconds: u64,
}

impl Default for PolicyCache {
    fn default() -> Self {
        Self {
            policies: Vec::new(),
            last_updated: Utc::now(),
            ttl_seconds: 300, // 5 min
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[cfg(test)]
mod a16_tests {
    use super::ExecutionPolicy;

    #[test]
    fn execution_policy_requires_signed_token_by_default() {
        // #6333 A16: a policy that omits `require_signed_token` must be fail-closed
        // (signed tokens required), not open.
        let json = r#"{"policy_id":"p","process_name":"proc","process_hash":null,"allowed_args":[],"requires_sudo":false,"max_execution_time_ms":1000}"#;
        let policy: ExecutionPolicy = serde_json::from_str(json).expect("must deserialize");
        assert!(
            policy.require_signed_token,
            "omitted require_signed_token must default to true (#6333 A16)"
        );
    }
}
