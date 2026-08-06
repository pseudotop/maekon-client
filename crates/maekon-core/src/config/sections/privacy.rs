// Privacy/isolation config — PII filter level, automation sandbox, excluded-app list
use super::super::enums::{ConfirmationRequirement, PiiFilterLevel, SandboxProfile};
use super::reauth::ReauthConfig;
use serde::{Deserialize, Serialize};

/// Safe-by-default confidence floor for LLM-planned automation intents.
///
/// `0.0` remains a supported explicit opt-out value, but fresh installs and missing
/// config fields should reject low-confidence interpretations before auto-execution.
pub const DEFAULT_MIN_LLM_CONFIDENCE: f64 = 0.65;

// ── PrivacyConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub excluded_apps: Vec<String>,
    #[serde(default)]
    pub excluded_app_patterns: Vec<String>,
    #[serde(default)]
    pub excluded_title_patterns: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_exclude_sensitive: bool,
    #[serde(default)]
    pub pii_filter_level: PiiFilterLevel,
    /// Capture-history viewing re-authentication (biometric/PIN) settings.
    /// Enabled by default (#8044).
    #[serde(default)]
    pub reauth: ReauthConfig,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            excluded_apps: Vec::new(),
            excluded_app_patterns: Vec::new(),
            excluded_title_patterns: Vec::new(),
            auto_exclude_sensitive: true,
            pii_filter_level: PiiFilterLevel::Standard,
            reauth: ReauthConfig::default(),
        }
    }
}

// ── SandboxConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub profile: SandboxProfile,
    #[serde(default)]
    pub allowed_read_paths: Vec<String>,
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub max_memory_bytes: u64,
    #[serde(default)]
    pub max_cpu_time_ms: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: SandboxProfile::Standard,
            allowed_read_paths: Vec::new(),
            allowed_write_paths: Vec::new(),
            allow_network: false,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
        }
    }
}

// ── PermissionProfileV2 ────────────────────────────────────────────

pub const PERMISSION_PROFILE_V2_SCHEMA_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAccess {
    Denied,
    Read,
    Write,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionFileSystemRules {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default = "default_secret_deny_globs")]
    pub deny_globs: Vec<String>,
    // #9638: `workspace_roots` / `max_scan_depth` were declared here for two
    // years but never read by `access_for_path` (the sole enforcement point) —
    // an operator setting them changed nothing. Removed rather than silently
    // kept; serde ignores the stale keys in existing config files. Re-add only
    // together with actual enforcement.
}

impl PermissionFileSystemRules {
    pub fn access_for_path(&self, path: &str) -> PermissionAccess {
        if self.deny.iter().any(|rule| path_rule_matches(rule, path))
            || self
                .deny_globs
                .iter()
                .any(|pattern| glob_rule_matches(pattern, path))
        {
            return PermissionAccess::Denied;
        }

        if self.write.iter().any(|rule| path_rule_matches(rule, path)) {
            return PermissionAccess::Write;
        }

        if self.read.iter().any(|rule| path_rule_matches(rule, path)) {
            return PermissionAccess::Read;
        }

        PermissionAccess::Denied
    }
}

impl Default for PermissionFileSystemRules {
    fn default() -> Self {
        Self {
            read: Vec::new(),
            write: Vec::new(),
            deny: Vec::new(),
            deny_globs: default_secret_deny_globs(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionNetworkMode {
    Connect,
    Bind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionNetworkDecision {
    Denied,
    Allowed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionNetworkRules {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow_domains: Vec<String>,
    #[serde(default)]
    pub deny_domains: Vec<String>,
    #[serde(default)]
    pub allow_local_targets: Vec<String>,
    #[serde(default)]
    pub allow_local_binding: bool,
}

impl PermissionNetworkRules {
    pub fn decision_for_target(
        &self,
        target: &str,
        mode: PermissionNetworkMode,
    ) -> PermissionNetworkDecision {
        if !self.enabled {
            return PermissionNetworkDecision::Denied;
        }

        if matches!(mode, PermissionNetworkMode::Bind) {
            return if self.allow_local_binding {
                PermissionNetworkDecision::Allowed
            } else {
                PermissionNetworkDecision::Denied
            };
        }

        let normalized_target = normalize_network_target(target);
        let host = network_host(&normalized_target);

        if self
            .deny_domains
            .iter()
            .any(|pattern| network_pattern_matches(pattern, &normalized_target, &host))
        {
            return PermissionNetworkDecision::Denied;
        }

        if is_local_network_host(&host) || is_private_network_host(&host) {
            return if self
                .allow_local_targets
                .iter()
                .any(|pattern| network_pattern_matches(pattern, &normalized_target, &host))
            {
                PermissionNetworkDecision::Allowed
            } else {
                PermissionNetworkDecision::Denied
            };
        }

        if self.allow_domains.is_empty()
            || self
                .allow_domains
                .iter()
                .any(|pattern| network_pattern_matches(pattern, &normalized_target, &host))
        {
            return PermissionNetworkDecision::Allowed;
        }

        PermissionNetworkDecision::Denied
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionUnixSocketDecision {
    Denied,
    AllowedAudited,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionUnixSocketRules {
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
    #[serde(default)]
    pub audit_enabled: bool,
}

impl PermissionUnixSocketRules {
    pub fn decision_for_path(&self, path: &str) -> PermissionUnixSocketDecision {
        if self.deny.iter().any(|rule| path_rule_matches(rule, path)) {
            return PermissionUnixSocketDecision::Denied;
        }

        if self.allow.iter().any(|rule| path_rule_matches(rule, path)) && self.audit_enabled {
            return PermissionUnixSocketDecision::AllowedAudited;
        }

        PermissionUnixSocketDecision::Denied
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PermissionProfileV2 {
    #[serde(default = "default_permission_schema_version")]
    pub schema_version: u8,
    #[serde(default)]
    pub source_profile: SandboxProfile,
    #[serde(default)]
    pub filesystem: PermissionFileSystemRules,
    #[serde(default)]
    pub network: PermissionNetworkRules,
    #[serde(default)]
    pub unix_sockets: PermissionUnixSocketRules,
    #[serde(default)]
    pub max_memory_bytes: u64,
    #[serde(default)]
    pub max_cpu_time_ms: u64,
}

impl PermissionProfileV2 {
    pub fn built_in(profile: SandboxProfile) -> Self {
        let mut permission_profile = Self {
            schema_version: PERMISSION_PROFILE_V2_SCHEMA_VERSION,
            source_profile: profile,
            filesystem: PermissionFileSystemRules::default(),
            network: PermissionNetworkRules::default(),
            unix_sockets: PermissionUnixSocketRules::default(),
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
        };

        if matches!(profile, SandboxProfile::Permissive) {
            permission_profile.filesystem.read.push("*".to_string());
            permission_profile.filesystem.write.push("*".to_string());
            permission_profile.network.enabled = true;
            permission_profile
                .network
                .allow_domains
                .push("*".to_string());
        }

        permission_profile
    }

    pub fn from_legacy_sandbox(config: &SandboxConfig) -> Self {
        let mut profile = Self::built_in(config.profile);
        profile.filesystem.read = config.allowed_read_paths.clone();
        profile.filesystem.write = config.allowed_write_paths.clone();
        profile.network.enabled = config.allow_network;
        profile.max_memory_bytes = config.max_memory_bytes;
        profile.max_cpu_time_ms = config.max_cpu_time_ms;
        profile
    }

    pub fn with_unix_socket_allow(mut self, path: impl Into<String>) -> Self {
        self.unix_sockets.allow.push(path.into());
        self.unix_sockets.audit_enabled = true;
        self
    }
}

impl Default for PermissionProfileV2 {
    fn default() -> Self {
        Self::built_in(SandboxProfile::Standard)
    }
}

pub fn default_secret_deny_globs() -> Vec<String> {
    [
        ".env",
        "**/.env",
        "**/.env.*",
        "*.pem",
        "**/*.pem",
        "*.key",
        "**/*.key",
        "**/id_rsa",
        "**/id_ed25519",
        "**/*credential*.json",
        "**/*credentials*.json",
        "**/*token*",
        "**/.aws/**",
        "**/.config/gh/**",
        "**/.docker/config.json",
        "**/.npmrc",
        "**/.pypirc",
        "**/.netrc",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_permission_schema_version() -> u8 {
    PERMISSION_PROFILE_V2_SCHEMA_VERSION
}

fn normalize_permission_path(value: &str) -> String {
    let normalized = value.trim().replace('\\', "/");
    if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_string()
    } else {
        normalized
    }
}

fn path_rule_matches(rule: &str, path: &str) -> bool {
    let rule = normalize_permission_path(rule);
    let path = normalize_permission_path(path);

    if rule == "*" || rule == "**" {
        return true;
    }

    if let Some(prefix) = rule.strip_suffix("/**") {
        let prefix = normalize_permission_path(prefix);
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }

    path == rule || path.starts_with(&format!("{rule}/"))
}

fn glob_rule_matches(pattern: &str, path: &str) -> bool {
    let pattern = normalize_permission_path(pattern);
    let path = normalize_permission_path(path);
    let filename = path.rsplit('/').next().unwrap_or(path.as_str());

    if !pattern.contains('*') && !pattern.contains('/') {
        return filename == pattern || path == pattern;
    }

    wildcard_matches(&pattern, &path)
        || (!pattern.contains('/') && wildcard_matches(&pattern, filename))
        || pattern
            .strip_prefix("**/")
            .map(|suffix| wildcard_matches(suffix, &path) || wildcard_matches(suffix, filename))
            .unwrap_or(false)
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" || pattern == "**" {
        return true;
    }

    if !pattern.contains('*') {
        return path_rule_matches(pattern, value);
    }

    let parts: Vec<&str> = pattern.split('*').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return true;
    }

    let mut remainder = value;
    if !pattern.starts_with('*') {
        let first = parts[0];
        let Some(stripped) = remainder.strip_prefix(first) else {
            return false;
        };
        remainder = stripped;
    }

    let start_idx = usize::from(!pattern.starts_with('*'));
    for part in parts.iter().skip(start_idx) {
        let Some(index) = remainder.find(part) else {
            return false;
        };
        remainder = &remainder[index + part.len()..];
    }

    pattern.ends_with('*') || remainder.is_empty()
}

fn normalize_network_target(target: &str) -> String {
    target.trim().trim_end_matches('/').to_ascii_lowercase()
}

fn network_host(target: &str) -> String {
    let after_scheme = target
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(target);
    let without_path = after_scheme.split('/').next().unwrap_or(after_scheme);
    let without_user = without_path
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(without_path);

    if let Some(stripped) = without_user.strip_prefix('[') {
        return stripped
            .split_once(']')
            .map(|(host, _)| host.to_string())
            .unwrap_or_else(|| stripped.to_string());
    }

    if without_user.matches(':').count() == 1 {
        without_user
            .split_once(':')
            .map(|(host, _)| host.to_string())
            .unwrap_or_else(|| without_user.to_string())
    } else {
        without_user.to_string()
    }
}

fn network_pattern_matches(pattern: &str, target: &str, host: &str) -> bool {
    let pattern = normalize_network_target(pattern);
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host.ends_with(&format!(".{suffix}")) || host == suffix;
    }

    wildcard_matches(&pattern, target) || wildcard_matches(&pattern, host)
}

// #7723: the bit-math below (0.0.0.0/8, fc00::/7, fe80::/10) is shared with the
// SSRF-guard call sites in `feature_capabilities.rs` / `ai_model_catalog_endpoint.rs`
// via `crate::net_policy`'s primitives — see that module's doc comment for why the
// three sites keep their own accept/reject combinations instead of one shared policy.
fn is_local_network_host(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || parse_ip(host)
            .map(|ip| match ip {
                std::net::IpAddr::V4(ip) => {
                    ip.is_loopback() || crate::net_policy::is_ipv4_this_network(&ip)
                }
                std::net::IpAddr::V6(ip) => ip
                    .to_ipv4_mapped()
                    .map(|mapped| {
                        mapped.is_loopback() || crate::net_policy::is_ipv4_this_network(&mapped)
                    })
                    .unwrap_or_else(|| ip.is_loopback()),
            })
            .unwrap_or(false)
}

fn is_private_network_host(host: &str) -> bool {
    parse_ip(host)
        .map(|ip| match ip {
            std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
            std::net::IpAddr::V6(ip) => {
                if let Some(mapped) = ip.to_ipv4_mapped() {
                    mapped.is_private() || mapped.is_link_local()
                } else {
                    crate::net_policy::is_unique_local_ipv6(&ip)
                        || crate::net_policy::is_link_local_ipv6(&ip)
                }
            }
        })
        .unwrap_or(false)
}

fn parse_ip(host: &str) -> Option<std::net::IpAddr> {
    host.parse().ok()
}

// ── AutomationConfig ───────────────────────────────────────────────

/// Configuration for the automation (RPA) subsystem.
///
/// `confirmation_policy` uses [`ConfirmationRequirement`], which is the
/// canonical runtime gate enum (Auto / Confirm / Block).  The former
/// `AutomationConfirmPolicy` (AlwaysConfirm / TrustedOnly / NeverConfirm)
/// has been retired and unified here (F-RC-C24-02).  Existing config files
/// that stored `ALWAYS_CONFIRM` or `TRUSTED_ONLY` should be migrated to
/// `CONFIRM`; `NEVER_CONFIRM` maps to `AUTO`.
///
/// The knob's default is `Auto` (D2-② product sign-off: intent-hint runs
/// immediately under strict sandbox, matching the "Runs immediately under
/// strict sandbox" caption shown in the UI).  Set `confirmation_policy =
/// "CONFIRM"` to require user approval on every intent-hint execution, or
/// `"BLOCK"` to disable it.
///
/// Note: this `Auto` default is applied at the FIELD level. The enum-level
/// `ConfirmationRequirement::default()` stays `Confirm` (fail-safe) because it
/// is also the `#[serde(default)]` for `ExecutionPolicy.confirmation` — a
/// security gate that must not default open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub custom_presets: Vec<crate::models::intent::WorkflowPreset>,
    #[serde(default = "default_confirmation_policy")]
    pub confirmation_policy: ConfirmationRequirement,
    /// #6333 A10: minimum LLM self-reported interpretation confidence (0.0-1.0)
    /// required to auto-execute an LLM-planned intent. Below this floor the plan is
    /// rejected rather than executed, so a hallucinated/prompt-injected low-confidence
    /// interpretation cannot auto-run. Set 0.0 explicitly to disable this gate.
    #[serde(default = "default_min_llm_confidence")]
    pub min_llm_confidence: f64,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: SandboxConfig::default(),
            custom_presets: Vec::new(),
            confirmation_policy: default_confirmation_policy(),
            min_llm_confidence: default_min_llm_confidence(),
        }
    }
}

/// D2-② sign-off default for the intent-hint confirmation knob.
fn default_confirmation_policy() -> ConfirmationRequirement {
    ConfirmationRequirement::Auto
}

/// #6333 A10 / E42.2: safe default for LLM-planned automation intents.
fn default_min_llm_confidence() -> f64 {
    DEFAULT_MIN_LLM_CONFIDENCE
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_config_confirmation_policy_serde_round_trip() {
        // AutomationConfig.confirmation_policy must serialise/deserialise using
        // SCREAMING_SNAKE_CASE tokens (inherited from ConfirmationRequirement serde attr).
        let config = AutomationConfig {
            enabled: false,
            confirmation_policy: ConfirmationRequirement::Confirm,
            ..AutomationConfig::default()
        };
        let json = serde_json::to_string(&config).expect("must serialise");
        assert!(
            json.contains("\"CONFIRM\""),
            "confirmation_policy must serialise as SCREAMING_SNAKE_CASE; got: {json}"
        );
        let restored: AutomationConfig = serde_json::from_str(&json).expect("must deserialise");
        assert_eq!(
            restored.confirmation_policy,
            ConfirmationRequirement::Confirm
        );
    }

    #[test]
    fn automation_config_default_confirmation_policy_is_auto() {
        // Default is Auto (D2-② product sign-off: immediate-run under strict sandbox).
        // F-RC-C24-02: users opt into Confirm/Block via config.
        assert_eq!(
            AutomationConfig::default().confirmation_policy,
            ConfirmationRequirement::Auto
        );
    }

    #[test]
    fn automation_config_default_min_llm_confidence_is_safe_floor() {
        assert!(
            AutomationConfig::default().min_llm_confidence >= 0.6,
            "fresh installs must reject low-confidence LLM-planned intents by default"
        );
    }

    #[test]
    fn automation_config_serde_default_min_llm_confidence_is_safe_floor() {
        let restored: AutomationConfig = serde_json::from_str("{}").expect("must deserialise");
        assert!(
            restored.min_llm_confidence >= 0.6,
            "missing config field must deserialize to the safe LLM confidence floor"
        );
    }

    #[test]
    fn permission_profile_v2_migration_keeps_legacy_default_closed() {
        let profile = PermissionProfileV2::from_legacy_sandbox(&SandboxConfig::default());

        assert!(!profile.network.enabled);
        assert_eq!(
            profile.filesystem.access_for_path("/tmp/output.txt"),
            PermissionAccess::Denied
        );
        assert!(profile.unix_sockets.allow.is_empty());
    }

    #[test]
    fn permission_profile_v2_deny_precedence_over_write_and_read() {
        let profile = PermissionProfileV2 {
            filesystem: PermissionFileSystemRules {
                read: vec!["/workspace".to_string()],
                write: vec!["/workspace/out".to_string()],
                deny: vec!["/workspace/out/private".to_string()],
                deny_globs: Vec::new(),
            },
            ..PermissionProfileV2::default()
        };

        assert_eq!(
            profile
                .filesystem
                .access_for_path("/workspace/out/private/secret.txt"),
            PermissionAccess::Denied
        );
        assert_eq!(
            profile
                .filesystem
                .access_for_path("/workspace/out/report.md"),
            PermissionAccess::Write
        );
        assert_eq!(
            profile.filesystem.access_for_path("/workspace/src/main.rs"),
            PermissionAccess::Read
        );
    }

    #[test]
    fn permission_profile_v2_secret_deny_globs_override_legacy_allowed_paths() {
        let legacy = SandboxConfig {
            allowed_read_paths: vec!["/workspace".to_string()],
            allowed_write_paths: vec!["/workspace/out".to_string()],
            ..SandboxConfig::default()
        };
        let profile = PermissionProfileV2::from_legacy_sandbox(&legacy);

        assert_eq!(
            profile
                .filesystem
                .access_for_path("/workspace/out/report.md"),
            PermissionAccess::Write
        );
        for secret_path in [
            "/workspace/.env",
            "/workspace/out/key.pem",
            "/workspace/out/deploy.key",
            "/workspace/id_rsa",
            "/workspace/id_ed25519",
            "/workspace/service-account-credentials.json",
            "/workspace/.config/gh/hosts.yml",
        ] {
            assert_eq!(
                profile.filesystem.access_for_path(secret_path),
                PermissionAccess::Denied,
                "{secret_path} must be denied even when the parent path is allowed"
            );
        }
    }

    #[test]
    fn permission_profile_v2_network_local_guard_requires_explicit_allow() {
        let mut network = PermissionNetworkRules {
            enabled: true,
            allow_domains: vec!["*".to_string()],
            ..PermissionNetworkRules::default()
        };

        assert_eq!(
            network.decision_for_target("api.openai.com", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Allowed
        );
        assert_eq!(
            network.decision_for_target("localhost:11434", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Denied
        );

        network
            .allow_local_targets
            .push("localhost:11434".to_string());
        assert_eq!(
            network.decision_for_target("localhost:11434", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Allowed
        );
        assert_eq!(
            network.decision_for_target("192.168.1.10:8080", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Denied
        );
        network
            .allow_local_targets
            .push("192.168.1.10:8080".to_string());
        assert_eq!(
            network.decision_for_target("192.168.1.10:8080", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Allowed
        );
        assert_eq!(
            network.decision_for_target("127.0.0.1:0", PermissionNetworkMode::Bind),
            PermissionNetworkDecision::Denied
        );

        network.allow_local_binding = true;
        assert_eq!(
            network.decision_for_target("127.0.0.1:0", PermissionNetworkMode::Bind),
            PermissionNetworkDecision::Allowed
        );
    }

    #[test]
    fn permission_profile_v2_network_guard_treats_ipv4_mapped_ipv6_as_local_or_private() {
        let network = PermissionNetworkRules {
            enabled: true,
            allow_domains: vec!["*".to_string()],
            ..PermissionNetworkRules::default()
        };

        for target in [
            "http://[::ffff:127.0.0.1]:11434",
            "http://[::ffff:192.168.1.10]:8080",
        ] {
            assert_eq!(
                network.decision_for_target(target, PermissionNetworkMode::Connect),
                PermissionNetworkDecision::Denied,
                "{target} must not be classified as public network egress"
            );
        }
    }

    #[test]
    fn permission_profile_v2_unix_socket_access_is_absent_by_default_and_audited_when_enabled() {
        let default_profile = PermissionProfileV2::built_in(SandboxProfile::Strict);
        assert_eq!(
            default_profile
                .unix_sockets
                .decision_for_path("/var/run/docker.sock"),
            PermissionUnixSocketDecision::Denied
        );

        let socket_profile = default_profile.with_unix_socket_allow("/var/run/docker.sock");
        assert_eq!(
            socket_profile
                .unix_sockets
                .decision_for_path("/var/run/docker.sock"),
            PermissionUnixSocketDecision::AllowedAudited
        );
    }

    #[test]
    fn legacy_sandbox_config_deserializes_and_migrates_to_v2() {
        let json = r#"{
            "enabled": true,
            "profile": "Strict",
            "allowed_read_paths": ["/workspace"],
            "allowed_write_paths": ["/workspace/out"],
            "allow_network": false,
            "max_memory_bytes": 4096,
            "max_cpu_time_ms": 1000
        }"#;
        let legacy: SandboxConfig = serde_json::from_str(json).expect("must deserialise");
        let migrated = PermissionProfileV2::from_legacy_sandbox(&legacy);

        assert_eq!(migrated.source_profile, SandboxProfile::Strict);
        assert_eq!(migrated.max_memory_bytes, 4096);
        assert_eq!(migrated.max_cpu_time_ms, 1000);
        assert_eq!(
            migrated
                .filesystem
                .access_for_path("/workspace/out/result.txt"),
            PermissionAccess::Write
        );
        assert!(!migrated.network.enabled);
    }
}

/// #10131: mutation guards for the permission matchers and network classifiers.
///
/// `cargo-mutants` found 23 surviving mutants in this file. These functions
/// decide which paths a permission rule covers and whether a host is
/// local/private, so a surviving `delete !` or `|| → &&` silently changes which
/// rule a path is matched against.
///
/// Each test isolates one arm: the input is chosen so that exactly one disjunct
/// or negation can produce the result, which is what makes flipping it
/// observable.
#[cfg(test)]
mod mutation_guard_tests {
    use super::*;

    // ---- decision_for_path: allow-match AND audit are both required ------

    #[test]
    fn unix_socket_allow_requires_audit_to_be_enabled() {
        let rules = PermissionUnixSocketRules {
            allow: vec!["/run/app.sock".to_string()],
            deny: vec![],
            audit_enabled: true,
        };
        assert_eq!(
            rules.decision_for_path("/run/app.sock"),
            PermissionUnixSocketDecision::AllowedAudited
        );

        // Same allow-list match, audit off — must NOT be allowed.
        let no_audit = PermissionUnixSocketRules {
            audit_enabled: false,
            ..rules.clone()
        };
        assert_eq!(
            no_audit.decision_for_path("/run/app.sock"),
            PermissionUnixSocketDecision::Denied
        );

        // Audit on, but the path is not on the allow list.
        assert_eq!(
            rules.decision_for_path("/run/other.sock"),
            PermissionUnixSocketDecision::Denied
        );
    }

    // ---- defaults: each default value is asserted ------------------------

    #[test]
    fn permission_schema_version_default_is_the_v2_constant() {
        assert_eq!(
            default_permission_schema_version(),
            PERMISSION_PROFILE_V2_SCHEMA_VERSION
        );
        assert_eq!(default_permission_schema_version(), 2);
    }

    #[test]
    fn min_llm_confidence_default_is_the_documented_threshold() {
        assert_eq!(default_min_llm_confidence(), DEFAULT_MIN_LLM_CONFIDENCE);
        assert!(
            (default_min_llm_confidence() - 0.65).abs() < f64::EPSILON,
            "a default of 1.0 would demand perfect confidence and silently \
             disable every LLM-planned intent"
        );
    }

    #[test]
    fn default_true_is_true() {
        assert!(default_true());
    }

    // ---- normalize_permission_path: the length guard is strict ----------

    #[test]
    fn normalize_permission_path_keeps_a_bare_root_slash() {
        // len == 1 must take the else branch: trimming here would turn the root
        // path into an empty string and collapse every rule that targets it.
        assert_eq!(normalize_permission_path("/"), "/");
        // len > 1 must trim the trailing separator.
        assert_eq!(normalize_permission_path("/a/"), "/a");
        assert_eq!(normalize_permission_path("/a/b/"), "/a/b");
        // Backslashes normalize, surrounding whitespace is trimmed.
        assert_eq!(normalize_permission_path("  C:\\x\\  "), "C:/x");
    }

    // ---- path_rule_matches: each wildcard spelling stands alone ----------

    #[test]
    fn path_rule_star_and_double_star_each_match_everything() {
        assert!(path_rule_matches("*", "/anything/here"));
        assert!(path_rule_matches("**", "/anything/here"));
    }

    #[test]
    fn path_rule_recursive_suffix_matches_the_prefix_itself_and_its_children() {
        // The prefix ITSELF matches — this is the `path == prefix` arm.
        assert!(path_rule_matches("/a/**", "/a"));
        // A child matches — this is the `starts_with(prefix/)` arm.
        assert!(path_rule_matches("/a/**", "/a/b"));
        // A sibling that merely shares a textual prefix must NOT match.
        assert!(!path_rule_matches("/a/**", "/ab"));
    }

    // ---- glob_rule_matches: each disjunct is independently sufficient ----

    #[test]
    fn glob_literal_pattern_matches_filename_or_whole_path() {
        assert!(glob_rule_matches("a.txt", "/dir/a.txt"));
        assert!(glob_rule_matches("a.txt", "a.txt"));
        assert!(!glob_rule_matches("a.txt", "/dir/b.txt"));
    }

    #[test]
    fn glob_literal_pattern_does_not_fall_through_to_prefix_matching() {
        // A literal (no `*`, no `/`) takes the early-return branch, which
        // compares against the filename and the whole path only. Falling
        // through to `wildcard_matches` would additionally treat the pattern as
        // a directory prefix and match `logs/x`, widening every literal rule.
        assert!(!glob_rule_matches("logs", "logs/x"));
    }

    #[test]
    fn glob_full_path_pattern_matches_via_the_path_disjunct_alone() {
        // Contains `/`, so the filename disjunct is disabled; no `**/` prefix,
        // so the third disjunct is disabled. Only the path disjunct can match.
        assert!(glob_rule_matches("/dir/*.txt", "/dir/a.txt"));
        assert!(!glob_rule_matches("/dir/*.txt", "/other/a.txt"));
    }

    #[test]
    fn glob_bare_pattern_matches_via_the_filename_disjunct_alone() {
        // `a*.txt` anchored at the start cannot match the full path
        // `/dir/ab.txt` (which begins with `/`), so only the filename disjunct
        // can produce a match here.
        assert!(glob_rule_matches("a*.txt", "/dir/ab.txt"));
    }

    #[test]
    fn glob_double_star_prefix_matches_via_the_strip_prefix_disjunct_alone() {
        // `**/a.txt` against a bare `a.txt`: the path disjunct fails (the
        // pattern's literal `/a.txt` is absent) and the filename disjunct is
        // disabled because the pattern contains `/`. Only the `**/` strip can
        // match.
        assert!(glob_rule_matches("**/a.txt", "a.txt"));
    }

    #[test]
    fn glob_double_star_suffix_matches_the_path_independently_of_the_filename() {
        // Isolates the INNER `||` of the `**/` branch. With a multi-segment
        // suffix and a path that carries no leading slash:
        //   - the outer path disjunct fails (the pattern's literal `/dir/a.txt`
        //     is not a substring of `dir/a.txt`),
        //   - the filename disjunct is disabled (the pattern contains `/`),
        //   - the suffix matches the PATH but NOT the filename (`a.txt`).
        // So only `wildcard_matches(suffix, path)` can carry the result.
        assert!(glob_rule_matches("**/dir/a.txt", "dir/a.txt"));
    }

    // ---- wildcard_matches: both bare wildcards stand alone ---------------

    #[test]
    fn wildcard_star_and_double_star_each_match_everything() {
        assert!(wildcard_matches("*", "/any/path"));
        assert!(wildcard_matches("**", "/any/path"));
    }

    // ---- network_pattern_matches: suffix and exact host both count -------

    #[test]
    fn network_wildcard_suffix_matches_subdomains_and_the_apex() {
        // Subdomain — the `ends_with(".suffix")` arm.
        assert!(network_pattern_matches(
            "*.example.com",
            "api.example.com",
            "api.example.com"
        ));
        // The apex itself — the `host == suffix` arm.
        assert!(network_pattern_matches(
            "*.example.com",
            "example.com",
            "example.com"
        ));
        // A domain that merely ends with the same letters must NOT match.
        assert!(!network_pattern_matches(
            "*.example.com",
            "notexample.com",
            "notexample.com"
        ));
    }

    // ---- is_local_network_host: each recognition arm stands alone --------

    #[test]
    fn local_host_each_form_is_independently_recognised() {
        assert!(is_local_network_host("localhost"));
        assert!(is_local_network_host("app.localhost"));
        assert!(is_local_network_host("127.0.0.1"));
        assert!(is_local_network_host("::1"));
        assert!(!is_local_network_host("example.com"));
        assert!(!is_local_network_host("8.8.8.8"));
    }

    // ---- is_private_network_host: private and link-local both count ------

    #[test]
    fn private_host_private_and_link_local_are_each_sufficient() {
        // RFC1918 private, not link-local.
        assert!(is_private_network_host("192.168.1.1"));
        assert!(is_private_network_host("10.0.0.1"));
        // Link-local, not RFC1918 private.
        assert!(is_private_network_host("169.254.1.1"));
        // Public address is neither.
        assert!(!is_private_network_host("8.8.8.8"));
    }

    #[test]
    fn private_host_native_ipv6_unique_local_and_link_local_are_each_sufficient() {
        // The native (non-v4-mapped) IPv6 branch is a SEPARATE arm from the v4
        // one above — testing only IPv4 left it uncovered.
        // Unique-local (fc00::/7), not link-local.
        assert!(is_private_network_host("fd00::1"));
        // Link-local (fe80::/10), not unique-local.
        assert!(is_private_network_host("fe80::1"));
        // Public IPv6 is neither.
        assert!(!is_private_network_host("2001:4860:4860::8888"));
    }

    #[test]
    fn private_host_v4_mapped_ipv6_reuses_the_v4_classification() {
        assert!(is_private_network_host("::ffff:192.168.1.1"));
        assert!(is_private_network_host("::ffff:169.254.1.1"));
        assert!(!is_private_network_host("::ffff:8.8.8.8"));
    }
}
