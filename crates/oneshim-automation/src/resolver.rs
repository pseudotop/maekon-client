use crate::policy::{AuditLevel, ExecutionPolicy};
use oneshim_core::config::{SandboxConfig, SandboxProfile};

pub fn resolve_sandbox_profile(policy: &ExecutionPolicy) -> SandboxProfile {
    if let Some(profile) = policy.sandbox_profile {
        return profile;
    }

    let base_profile = match policy.audit_level {
        AuditLevel::None => SandboxProfile::Permissive,
        AuditLevel::Basic => SandboxProfile::Standard,
        AuditLevel::Detailed => SandboxProfile::Strict,
        AuditLevel::Full => SandboxProfile::Strict,
    };

    if policy.requires_sudo && matches!(base_profile, SandboxProfile::Permissive) {
        return SandboxProfile::Standard;
    }

    base_profile
}

pub fn resolve_sandbox_config(
    policy: &ExecutionPolicy,
    base_config: &SandboxConfig,
) -> SandboxConfig {
    let profile = resolve_sandbox_profile(policy);

    let allow_network = policy
        .allow_network
        .unwrap_or(matches!(profile, SandboxProfile::Permissive));

    let mut allowed_read_paths = base_config.allowed_read_paths.clone();
    for path in &policy.allowed_paths {
        if !allowed_read_paths.contains(path) {
            allowed_read_paths.push(path.clone());
        }
    }

    let max_cpu_time_ms = if policy.max_execution_time_ms > 0 {
        policy.max_execution_time_ms
    } else {
        base_config.max_cpu_time_ms
    };

    SandboxConfig {
        enabled: base_config.enabled,
        profile,
        allowed_read_paths,
        allowed_write_paths: base_config.allowed_write_paths.clone(),
        allow_network,
        max_memory_bytes: base_config.max_memory_bytes,
        max_cpu_time_ms,
    }
}

pub fn default_strict_config(base_config: &SandboxConfig) -> SandboxConfig {
    SandboxConfig {
        enabled: base_config.enabled,
        profile: SandboxProfile::Strict,
        allowed_read_paths: base_config.allowed_read_paths.clone(),
        allowed_write_paths: Vec::new(),
        allow_network: false,
        max_memory_bytes: base_config.max_memory_bytes,
        max_cpu_time_ms: base_config.max_cpu_time_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy(audit: AuditLevel, sudo: bool) -> ExecutionPolicy {
        ExecutionPolicy {
            policy_id: "test".to_string(),
            process_name: "test".to_string(),
            process_hash: None,
            allowed_args: vec![],
            requires_sudo: sudo,
            max_execution_time_ms: 0,
            audit_level: audit,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: None,
            require_signed_token: false,
            confirmation: Default::default(),
        }
    }

    #[test]
    fn audit_none_maps_to_permissive() {
        let policy = make_policy(AuditLevel::None, false);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Permissive
        ));
    }

    #[test]
    fn audit_basic_maps_to_standard() {
        let policy = make_policy(AuditLevel::Basic, false);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Standard
        ));
    }

    #[test]
    fn audit_detailed_maps_to_strict() {
        let policy = make_policy(AuditLevel::Detailed, false);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Strict
        ));
    }

    #[test]
    fn audit_full_maps_to_strict() {
        let policy = make_policy(AuditLevel::Full, false);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Strict
        ));
    }

    #[test]
    fn sudo_escalates_permissive_to_standard() {
        let policy = make_policy(AuditLevel::None, true);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Standard
        ));
    }

    #[test]
    fn sudo_does_not_escalate_strict() {
        let policy = make_policy(AuditLevel::Detailed, true);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Strict
        ));
    }

    #[test]
    fn server_override_takes_priority() {
        let mut policy = make_policy(AuditLevel::Full, true);
        policy.sandbox_profile = Some(SandboxProfile::Permissive);
        assert!(matches!(
            resolve_sandbox_profile(&policy),
            SandboxProfile::Permissive
        ));
    }

    #[test]
    fn config_merges_allowed_paths() {
        let mut policy = make_policy(AuditLevel::Basic, false);
        policy.allowed_paths = vec!["/tmp/extra".to_string()];

        let base = SandboxConfig {
            allowed_read_paths: vec!["/usr/lib".to_string()],
            ..Default::default()
        };

        let resolved = resolve_sandbox_config(&policy, &base);
        assert_eq!(resolved.allowed_read_paths.len(), 2);
        assert!(resolved
            .allowed_read_paths
            .contains(&"/usr/lib".to_string()));
        assert!(resolved
            .allowed_read_paths
            .contains(&"/tmp/extra".to_string()));
    }

    #[test]
    fn config_network_override() {
        let mut policy = make_policy(AuditLevel::Detailed, false);
        policy.allow_network = Some(true);

        let resolved = resolve_sandbox_config(&policy, &SandboxConfig::default());
        assert!(resolved.allow_network);
    }

    #[test]
    fn config_max_cpu_time_from_policy() {
        let mut policy = make_policy(AuditLevel::Basic, false);
        policy.max_execution_time_ms = 3000;

        let resolved = resolve_sandbox_config(&policy, &SandboxConfig::default());
        assert_eq!(resolved.max_cpu_time_ms, 3000);
    }

    #[test]
    fn default_strict_blocks_write_and_network() {
        let base = SandboxConfig {
            allowed_write_paths: vec!["/tmp".to_string()],
            allow_network: true,
            ..Default::default()
        };

        let strict = default_strict_config(&base);
        assert!(matches!(strict.profile, SandboxProfile::Strict));
        assert!(strict.allowed_write_paths.is_empty());
        assert!(!strict.allow_network);
    }
}
