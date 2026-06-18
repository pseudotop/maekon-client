// OOS-TBD: ADR-013 file split (cycle 35+) — LOC: 771
mod models;
mod token;

// ── Public re-exports (external API) ────────────────────────────────
pub use models::{AuditLevel, ExecutionPolicy, PolicyCache, ProcessOutput};

use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;

use crate::controller::AutomationCommand;
use crate::error::AutomationError;
use maekon_core::models::automation::CommandOrigin;

use token::{
    compute_command_scope_hash, is_valid_nonce, is_valid_signature, issue_command_token_for_policy,
    issue_policy_nonce, parse_policy_token, verify_policy_token_signature,
};

// PolicyClient

pub struct PolicyClient {
    policy_cache: RwLock<PolicyCache>,
    allowed_processes: RwLock<HashSet<String>>,
    validated_tokens: RwLock<HashMap<String, chrono::DateTime<Utc>>>,
}

impl PolicyClient {
    pub fn new() -> Self {
        Self {
            policy_cache: RwLock::new(PolicyCache::default()),
            allowed_processes: RwLock::new(HashSet::new()),
            validated_tokens: RwLock::new(HashMap::new()),
        }
    }

    pub async fn update_policies(&self, policies: Vec<ExecutionPolicy>) {
        let mut cache = self.policy_cache.write().await;
        let mut allowed = self.allowed_processes.write().await;
        let mut validated = self.validated_tokens.write().await;

        allowed.clear();
        for policy in &policies {
            allowed.insert(policy.process_name.clone());
        }

        cache.policies = policies;
        cache.last_updated = Utc::now();
        validated.clear();
    }

    pub async fn is_cache_valid(&self) -> bool {
        let cache = self.policy_cache.read().await;
        let elapsed = Utc::now()
            .signed_duration_since(cache.last_updated)
            .num_seconds() as u64;
        elapsed < cache.ttl_seconds
    }

    pub async fn validate_command(&self, cmd: &AutomationCommand) -> Result<bool, AutomationError> {
        // When TTL is configured to 0, immediately reject every cached token.
        // A configured ttl_seconds of 0 means "never cache" — every token is
        // treated as a replay to prevent stale token acceptance.
        {
            let cache = self.policy_cache.read().await;
            if cache.ttl_seconds == 0 {
                tracing::debug!("policy TTL is 0 — rejecting all cached tokens");
                return Ok(false);
            }
        }

        let now = Utc::now();
        let token = cmd.policy_token.trim();
        if token.is_empty() {
            return Ok(false);
        }

        let Some(parsed_token) = parse_policy_token(token) else {
            tracing::warn!(
                policy_token_fingerprint = %policy_token_fingerprint(token),
                "policy token error"
            );
            return Ok(false);
        };
        if !is_valid_nonce(parsed_token.nonce) {
            tracing::warn!(
                policy_token_fingerprint = %policy_token_fingerprint(token),
                "policy token nonce error"
            );
            return Ok(false);
        }

        let ttl_seconds = {
            let cache = self.policy_cache.read().await;
            let elapsed = now.signed_duration_since(cache.last_updated).num_seconds() as u64;
            if elapsed >= cache.ttl_seconds {
                0
            } else {
                cache.ttl_seconds
            }
        };

        if ttl_seconds == 0 {
            tracing::warn!("policy cache expired: server refresh required");
            return Ok(false);
        }

        let Some(policy) = self.get_policy_for_token(token).await else {
            tracing::warn!(
                policy_id = parsed_token.policy_id,
                "no matching policy found for policy token"
            );
            return Ok(false);
        };

        // #6333 A16 (hard invariant): External-origin commands must always be signed,
        // regardless of the per-policy flag. The `require_signed_token = false` opt-out
        // may only relax verification for Internal-origin commands — which bypass this
        // function entirely (gate.rs is_trusted_internal) — so a misconfigured policy can
        // never accept an unsigned external command (Saltzer & Schroeder fail-safe defaults).
        let require_signature =
            policy.require_signed_token || cmd.origin == CommandOrigin::External;
        if require_signature {
            let Some(signature) = parsed_token.signature else {
                tracing::warn!(
                    policy_id = parsed_token.policy_id,
                    "policy requires signature but policy token has no signature"
                );
                return Ok(false);
            };

            if !is_valid_signature(signature) {
                tracing::warn!(
                    policy_id = parsed_token.policy_id,
                    "invalid policy token signature format"
                );
                return Ok(false);
            }

            if !verify_policy_token_signature(
                parsed_token.policy_id,
                parsed_token.nonce,
                parsed_token.command_hash,
                signature,
            ) {
                tracing::warn!(
                    policy_id = parsed_token.policy_id,
                    "policy token signature validation failed"
                );
                return Ok(false);
            }
        }

        if let Some(token_command_hash) = parsed_token.command_hash {
            let expected_hash = compute_command_scope_hash(cmd)?;
            if !token_command_hash.eq_ignore_ascii_case(&expected_hash) {
                tracing::warn!(
                    policy_id = parsed_token.policy_id,
                    "policy token command hash mismatch"
                );
                return Ok(false);
            }
        }

        let mut validated = self.validated_tokens.write().await;
        validated.retain(|_, validated_at| {
            now.signed_duration_since(*validated_at).num_seconds() < ttl_seconds as i64
        });
        if validated.contains_key(token) {
            tracing::warn!(
                policy_token_fingerprint = %policy_token_fingerprint(token),
                "policy token detection"
            );
            return Ok(false);
        }
        validated.insert(token.to_string(), now);

        Ok(true)
    }

    /// - unsigned: `{policy_id}:{nonce}`
    /// - signed: `{policy_id}:{nonce}:{sha256(policy_id:nonce:secret)}`
    pub async fn issue_command_token(&self, policy_id: &str) -> Result<String, AutomationError> {
        let policy = {
            let cache = self.policy_cache.read().await;
            cache
                .policies
                .iter()
                .find(|p| p.policy_id == policy_id)
                .cloned()
        }
        .ok_or_else(|| AutomationError::PolicyDenied(format!("Unknown policy ID: {policy_id}")))?;

        let nonce = issue_policy_nonce();
        issue_command_token_for_policy(&policy, &nonce, None)
    }

    pub async fn issue_command_token_for_command(
        &self,
        policy_id: &str,
        cmd: &AutomationCommand,
    ) -> Result<String, AutomationError> {
        let policy = {
            let cache = self.policy_cache.read().await;
            cache
                .policies
                .iter()
                .find(|p| p.policy_id == policy_id)
                .cloned()
        }
        .ok_or_else(|| AutomationError::PolicyDenied(format!("Unknown policy ID: {policy_id}")))?;

        let nonce = issue_policy_nonce();
        let command_hash = compute_command_scope_hash(cmd)?;
        issue_command_token_for_policy(&policy, &nonce, Some(command_hash.as_str()))
    }

    pub async fn get_policy_for_process(&self, process_name: &str) -> Option<ExecutionPolicy> {
        let cache = self.policy_cache.read().await;
        cache
            .policies
            .iter()
            .find(|p| p.process_name == process_name)
            .cloned()
    }

    pub async fn is_process_allowed(&self, process_name: &str) -> bool {
        let allowed = self.allowed_processes.read().await;
        allowed.contains(process_name)
    }

    pub async fn get_policy_for_token(&self, policy_token: &str) -> Option<ExecutionPolicy> {
        let cache = self.policy_cache.read().await;
        let policy_id = parse_policy_token(policy_token)
            .map(|token| token.policy_id)
            .unwrap_or(policy_token);
        cache
            .policies
            .iter()
            .find(|p| p.policy_id == policy_id)
            .cloned()
    }

    /// Add or replace a single execution policy by `policy_id`.
    pub async fn add_policy(&self, policy: ExecutionPolicy) {
        let mut cache = self.policy_cache.write().await;
        cache.policies.retain(|p| p.policy_id != policy.policy_id);
        let name = policy.process_name.clone();
        cache.policies.push(policy);
        cache.last_updated = Utc::now();
        let mut allowed = self.allowed_processes.write().await;
        allowed.insert(name);
    }

    /// Remove a policy by `policy_id`. Returns `true` if a policy was removed.
    pub async fn remove_policy(&self, policy_id: &str) -> bool {
        let mut cache = self.policy_cache.write().await;
        let before = cache.policies.len();
        cache.policies.retain(|p| p.policy_id != policy_id);
        if cache.policies.len() < before {
            cache.last_updated = Utc::now();
            let mut allowed = self.allowed_processes.write().await;
            *allowed = cache
                .policies
                .iter()
                .map(|p| p.process_name.clone())
                .collect();
            true
        } else {
            false
        }
    }

    /// Return a snapshot of all execution policies.
    pub async fn list_policies(&self) -> Vec<ExecutionPolicy> {
        self.policy_cache.read().await.policies.clone()
    }

    pub fn validate_args(policy: &ExecutionPolicy, args: &[String]) -> bool {
        if policy.allowed_args.is_empty() {
            return true; // unrestricted
        }

        args.iter().all(|arg| {
            policy.allowed_args.iter().any(|pattern| {
                if pattern.contains('*') {
                    let parts: Vec<&str> = pattern.split('*').collect();
                    if parts.len() == 2 {
                        arg.starts_with(parts[0]) && arg.ends_with(parts[1])
                    } else {
                        arg == pattern
                    }
                } else {
                    arg == pattern
                }
            })
        })
    }
}

impl Default for PolicyClient {
    fn default() -> Self {
        Self::new()
    }
}

fn policy_token_fingerprint(token: &str) -> String {
    let digest = Sha256::digest(token.trim().as_bytes());
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use token::{compute_policy_token_signature, POLICY_TOKEN_SIGNING_SECRET_ENV};
    use tokio::sync::Mutex;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn execution_policy_serde() {
        let policy = ExecutionPolicy {
            policy_id: "pol-001".to_string(),
            process_name: "ls".to_string(),
            process_hash: None,
            allowed_args: vec!["-la".to_string()],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        };
        let json = serde_json::to_string(&policy).unwrap();
        let deser: ExecutionPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.policy_id, "pol-001");
        assert_eq!(deser.audit_level, AuditLevel::Basic);
        assert!(deser.sandbox_profile.is_none());
        assert!(deser.allowed_paths.is_empty());
        assert!(deser.allow_network.is_none());
        assert!(!deser.require_signed_token);
    }

    #[test]
    fn validate_args_empty_allows_all() {
        let policy = ExecutionPolicy {
            policy_id: "p".to_string(),
            process_name: "ls".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::None,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        };
        assert!(PolicyClient::validate_args(
            &policy,
            &["anything".to_string()]
        ));
    }

    #[test]
    fn validate_args_pattern_match() {
        let policy = ExecutionPolicy {
            policy_id: "p".to_string(),
            process_name: "git".to_string(),
            process_hash: None,
            allowed_args: vec!["--*.txt".to_string(), "status".to_string()],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::None,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        };
        assert!(PolicyClient::validate_args(
            &policy,
            &["status".to_string()]
        ));
        assert!(!PolicyClient::validate_args(&policy, &["push".to_string()]));
    }

    #[test]
    fn policy_token_fingerprint_is_stable_and_non_reversible() {
        let token = "pol-1:nonce_1234";
        let fingerprint = policy_token_fingerprint(token);

        assert_eq!(fingerprint, policy_token_fingerprint(token));
        assert_ne!(fingerprint, token);
        assert_eq!(fingerprint.len(), 16);
    }

    #[test]
    fn policy_logging_source_never_uses_raw_policy_token_field() {
        let source = include_str!("mod.rs");
        let forbidden = ["policy_token", " = ", "token", ","].concat();

        assert!(!source.contains(&forbidden));
        assert!(source.contains("policy_token_fingerprint"));
    }

    #[tokio::test]
    async fn update_and_check_policies() {
        let client = PolicyClient::new();
        let policies = vec![ExecutionPolicy {
            policy_id: "p1".to_string(),
            process_name: "ls".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        }];

        client.update_policies(policies).await;
        assert!(client.is_process_allowed("ls").await);
        assert!(!client.is_process_allowed("rm").await);
    }

    #[tokio::test]
    async fn get_policy_for_token_found() {
        let client = PolicyClient::new();
        let policies = vec![ExecutionPolicy {
            policy_id: "pol-42".to_string(),
            process_name: "git".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 10000,
            audit_level: AuditLevel::Detailed,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        }];
        client.update_policies(policies).await;

        let found = client.get_policy_for_token("pol-42:abc123").await;
        assert!(found.is_some());
        assert_eq!(found.unwrap().process_name, "git");
    }

    #[tokio::test]
    async fn get_policy_for_token_not_found() {
        let client = PolicyClient::new();
        let found = client.get_policy_for_token("nonexistent:xyz").await;
        assert!(found.is_none());
    }

    fn make_policy(policy_id: &str) -> ExecutionPolicy {
        ExecutionPolicy {
            policy_id: policy_id.to_string(),
            process_name: "git".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: false,
            max_execution_time_ms: 10000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        }
    }

    fn make_command(policy_token: &str) -> AutomationCommand {
        AutomationCommand {
            command_id: "cmd-1".to_string(),
            session_id: "sess-1".to_string(),
            action: crate::controller::AutomationAction::MouseMove { x: 0, y: 0 },
            timeout_ms: None,
            policy_token: policy_token.to_string(),
            origin: maekon_core::models::automation::CommandOrigin::External,
        }
    }

    #[tokio::test]
    async fn validate_command_rejects_all_when_ttl_is_zero() {
        // PolicyCache.ttl_seconds == 0 means "never cache" — every token must
        // be rejected regardless of policy or nonce validity.
        let client = PolicyClient::new();
        // Inject a policy whose cache has ttl_seconds = 0.
        {
            let mut cache = client.policy_cache.write().await;
            cache.policies.push(make_policy("pol-1"));
            cache.ttl_seconds = 0;
        }
        {
            let mut allowed = client.allowed_processes.write().await;
            allowed.insert("git".to_string());
        }

        let cmd = make_command("pol-1:nonce_1234");
        assert!(
            !client.validate_command(&cmd).await.unwrap(),
            "ttl_seconds=0 must reject all tokens"
        );
    }

    #[tokio::test]
    async fn validate_command_requires_existing_policy_and_valid_nonce() {
        let env_guard = env_lock().lock().await;
        std::env::set_var(POLICY_TOKEN_SIGNING_SECRET_ENV, "test-secret");
        let client = PolicyClient::new();
        client.update_policies(vec![make_policy("pol-1")]).await;

        let invalid_format = make_command("pol-1");
        assert!(!client.validate_command(&invalid_format).await.unwrap());

        let invalid_nonce = make_command("pol-1:short");
        assert!(!client.validate_command(&invalid_nonce).await.unwrap());

        let unknown_policy = make_command("pol-x:nonce_1234");
        assert!(!client.validate_command(&unknown_policy).await.unwrap());

        // #6333 A16: make_command defaults to External origin, which now requires a
        // signature regardless of the policy flag — sign a valid token to exercise accept.
        let nonce = issue_policy_nonce();
        let sig = compute_policy_token_signature("pol-1", &nonce, None, "test-secret");
        let valid = make_command(&format!("pol-1:{nonce}:{sig}"));
        assert!(client.validate_command(&valid).await.unwrap());
        drop(env_guard);
    }

    #[tokio::test]
    async fn validate_command_rejects_replayed_policy_token() {
        let env_guard = env_lock().lock().await;
        std::env::set_var(POLICY_TOKEN_SIGNING_SECRET_ENV, "test-secret");
        let client = PolicyClient::new();
        client.update_policies(vec![make_policy("pol-1")]).await;

        // #6333 A16: external-origin command must be signed to reach the replay check.
        let nonce = issue_policy_nonce();
        let sig = compute_policy_token_signature("pol-1", &nonce, None, "test-secret");
        let cmd = make_command(&format!("pol-1:{nonce}:{sig}"));
        assert!(client.validate_command(&cmd).await.unwrap());
        assert!(!client.validate_command(&cmd).await.unwrap());
        drop(env_guard);
    }

    #[tokio::test]
    async fn validate_command_requires_signature_for_external_origin_even_when_policy_allows_unsigned(
    ) {
        // #6333 A16 hard invariant: an External-origin command whose policy does NOT
        // require signed tokens must still be rejected when unsigned. Closes the
        // "misconfigured policy fails open" gap the default-flip alone leaves.
        let client = PolicyClient::new();
        client.update_policies(vec![make_policy("pol-1")]).await; // require_signed_token: false
        let cmd = make_command("pol-1:nonce_aaaaaaaa"); // unsigned; make_command → External origin
        assert!(!client.validate_command(&cmd).await.unwrap());
    }

    #[test]
    fn parse_policy_token_supports_optional_signature() {
        let without_signature = parse_policy_token("pol-1:nonce_1234").unwrap();
        assert_eq!(without_signature.policy_id, "pol-1");
        assert_eq!(without_signature.nonce, "nonce_1234");
        assert!(without_signature.command_hash.is_none());
        assert!(without_signature.signature.is_none());

        let with_command_hash = parse_policy_token(
            "pol-1:nonce_1234:h14bf0b43befc58f56d4e4bcc9c8942d44f8d3af1321a96bea6f89fa44f4f5329",
        )
        .unwrap();
        assert_eq!(with_command_hash.policy_id, "pol-1");
        assert_eq!(with_command_hash.nonce, "nonce_1234");
        assert_eq!(
            with_command_hash.command_hash,
            Some("14bf0b43befc58f56d4e4bcc9c8942d44f8d3af1321a96bea6f89fa44f4f5329")
        );
        assert!(with_command_hash.signature.is_none());

        let with_signature = parse_policy_token(
            "pol-1:nonce_1234:14bf0b43befc58f56d4e4bcc9c8942d44f8d3af1321a96bea6f89fa44f4f5329",
        )
        .unwrap();
        assert_eq!(with_signature.policy_id, "pol-1");
        assert_eq!(with_signature.nonce, "nonce_1234");
        assert!(with_signature.command_hash.is_none());
        assert!(with_signature.signature.is_some());
    }

    #[test]
    fn parse_policy_token_rejects_too_many_segments() {
        assert!(parse_policy_token("pol-1:nonce:hdeadbeef:signature:extra").is_none());
    }

    fn fixture_signing_secret() -> String {
        String::from_utf8(vec![115, 101, 99, 114, 101, 116]).unwrap()
    }

    #[test]
    fn compute_policy_signature_is_deterministic() {
        let secret = fixture_signing_secret();
        let nonce = issue_policy_nonce();
        let signature = compute_policy_token_signature("pol-1", &nonce, None, &secret);
        assert_eq!(
            signature,
            compute_policy_token_signature("pol-1", &nonce, None, &secret)
        );
        assert!(is_valid_signature(&signature));
    }

    #[tokio::test]
    async fn validate_command_rejects_missing_signature_when_policy_requires_it() {
        let client = PolicyClient::new();
        let mut policy = make_policy("pol-1");
        policy.require_signed_token = true;
        client.update_policies(vec![policy]).await;

        let cmd = make_command("pol-1:nonce_1234");
        assert!(!client.validate_command(&cmd).await.unwrap());
    }

    #[tokio::test]
    async fn issue_command_token_rejects_unknown_policy() {
        let client = PolicyClient::new();
        let result = client.issue_command_token("unknown").await;
        assert!(matches!(result, Err(AutomationError::PolicyDenied(_))));
    }

    #[tokio::test]
    async fn issue_command_token_for_unsigned_policy() {
        let client = PolicyClient::new();
        client.update_policies(vec![make_policy("pol-1")]).await;

        let token = client
            .issue_command_token("pol-1")
            .await
            .expect("Failed to issue token");
        let parsed = parse_policy_token(&token).expect("Failed to parse issued token");
        assert_eq!(parsed.policy_id, "pol-1");
        assert!(parsed.command_hash.is_none());
        assert!(parsed.signature.is_none());
        assert!(is_valid_nonce(parsed.nonce));
    }

    #[tokio::test]
    async fn issue_command_token_for_signed_policy_requires_secret() {
        let env_guard = env_lock().lock().await;
        std::env::remove_var(POLICY_TOKEN_SIGNING_SECRET_ENV);

        let client = PolicyClient::new();
        let mut policy = make_policy("pol-1");
        policy.require_signed_token = true;
        client.update_policies(vec![policy]).await;

        let result = client.issue_command_token("pol-1").await;
        // Iter-100: signing-secret env var missing now routes to
        // AutomationError::Core(Config{Missing}) → wire config.missing.
        let err = result.unwrap_err();
        let core: maekon_core::error::CoreError = err.into();
        assert_eq!(core.code(), "config.missing");

        drop(env_guard);
    }

    #[tokio::test]
    async fn issue_command_token_for_signed_policy_generates_verifiable_token() {
        let env_guard = env_lock().lock().await;
        std::env::set_var(POLICY_TOKEN_SIGNING_SECRET_ENV, "signing-secret");

        let client = PolicyClient::new();
        let mut policy = make_policy("pol-1");
        policy.require_signed_token = true;
        client.update_policies(vec![policy]).await;

        let token = client
            .issue_command_token("pol-1")
            .await
            .expect("Failed to issue token");

        let parsed = parse_policy_token(&token).expect("Failed to parse issued token");
        assert!(parsed.command_hash.is_none());
        let signature = parsed.signature.expect("Missing signature");
        assert!(is_valid_signature(signature));
        assert!(verify_policy_token_signature(
            parsed.policy_id,
            parsed.nonce,
            parsed.command_hash,
            signature
        ));

        let cmd = make_command(&token);
        assert!(client.validate_command(&cmd).await.unwrap());

        std::env::remove_var(POLICY_TOKEN_SIGNING_SECRET_ENV);
        drop(env_guard);
    }

    #[tokio::test]
    async fn issue_command_token_for_command_binds_command_scope() {
        let client = PolicyClient::new();
        client.update_policies(vec![make_policy("pol-1")]).await;

        let mut cmd = make_command("unused");
        cmd.command_id = "cmd-bound".to_string();
        cmd.session_id = "sess-bound".to_string();
        // #6333 A16: this exercises command-scope BINDING, orthogonal to the signature
        // requirement; Internal origin isolates the command_hash check from signing.
        cmd.origin = CommandOrigin::Internal;
        let token = client
            .issue_command_token_for_command("pol-1", &cmd)
            .await
            .expect("Failed to issue token");
        cmd.policy_token = token;
        assert!(client.validate_command(&cmd).await.unwrap());
    }

    #[tokio::test]
    async fn validate_command_rejects_when_bound_command_scope_mismatches() {
        let client = PolicyClient::new();
        client.update_policies(vec![make_policy("pol-1")]).await;

        let mut source_cmd = make_command("unused");
        source_cmd.command_id = "cmd-source".to_string();
        source_cmd.session_id = "sess-source".to_string();
        let token = client
            .issue_command_token_for_command("pol-1", &source_cmd)
            .await
            .expect("Failed to issue token");

        let mut different_cmd = make_command(&token);
        different_cmd.command_id = "cmd-other".to_string();
        // #6333 A16: Internal origin so this rejects for command-hash MISMATCH (the
        // behavior under test), not for a missing signature.
        different_cmd.origin = CommandOrigin::Internal;
        assert!(!client.validate_command(&different_cmd).await.unwrap());
    }

    #[tokio::test]
    async fn add_policy_inserts_new_policy() {
        let client = PolicyClient::new();
        let policy = make_policy("pol-add");
        client.add_policy(policy).await;
        let list = client.list_policies().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].policy_id, "pol-add");
        assert!(client.is_process_allowed("git").await);
    }

    #[tokio::test]
    async fn add_policy_replaces_existing_by_id() {
        let client = PolicyClient::new();
        let mut p1 = make_policy("pol-1");
        p1.process_name = "ls".to_string();
        client.add_policy(p1).await;
        assert_eq!(client.list_policies().await.len(), 1);

        let mut p1_updated = make_policy("pol-1");
        p1_updated.process_name = "cat".to_string();
        client.add_policy(p1_updated).await;

        let list = client.list_policies().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].process_name, "cat");
    }

    #[tokio::test]
    async fn remove_policy_returns_false_for_unknown() {
        let client = PolicyClient::new();
        assert!(!client.remove_policy("nonexistent").await);
    }

    #[tokio::test]
    async fn remove_policy_removes_and_rebuilds_allowed() {
        let client = PolicyClient::new();
        let mut p1 = make_policy("pol-1");
        p1.process_name = "git".to_string();
        let mut p2 = make_policy("pol-2");
        p2.process_name = "ls".to_string();
        client.add_policy(p1).await;
        client.add_policy(p2).await;
        assert_eq!(client.list_policies().await.len(), 2);

        assert!(client.remove_policy("pol-1").await);
        assert_eq!(client.list_policies().await.len(), 1);
        assert!(!client.is_process_allowed("git").await);
        assert!(client.is_process_allowed("ls").await);
    }

    #[tokio::test]
    async fn list_policies_empty_by_default() {
        let client = PolicyClient::new();
        assert!(client.list_policies().await.is_empty());
    }
}
