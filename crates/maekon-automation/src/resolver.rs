use crate::policy::{AuditLevel, ExecutionPolicy};
use maekon_core::config::{
    PermissionNetworkDecision, PermissionNetworkMode, PermissionProfileV2, SandboxConfig,
    SandboxProfile,
};
use maekon_core::ports::sandbox::SandboxCapabilities;

/// Resolve the effective sandbox profile for an execution policy.
///
/// # Trust boundary (#8047 E8)
///
/// An explicit server-issued `policy.sandbox_profile` takes priority over the
/// profile that would otherwise be derived from `audit_level` and the
/// `requires_sudo` escalation below (pinned by the `server_override_takes_priority`
/// test). This is by design: the server is trusted control-plane data. The
/// consequence is that a malicious or misconfigured server can hand down a weaker
/// profile (e.g. `Permissive`) and thereby weaken the client's sandboxing. That
/// risk is out of this crate's blast radius by design — the client trusts its
/// paired server for policy. If the server-trust assumption is ever relaxed, this
/// override must be reconsidered (e.g. clamping the server value to a
/// locally-derived floor).
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

pub fn resolve_permission_profile_v2(
    policy: &ExecutionPolicy,
    base_config: &SandboxConfig,
) -> PermissionProfileV2 {
    let resolved = resolve_sandbox_config(policy, base_config);
    PermissionProfileV2::from_legacy_sandbox(&resolved)
}

/// Fail closed when a V2 permission profile contains rules the legacy
/// `SandboxConfig` runtime cannot faithfully enforce yet.
pub fn validate_permission_profile_v2_runtime_support(
    profile: &PermissionProfileV2,
) -> Result<(), String> {
    let filesystem_has_allow_rules =
        !profile.filesystem.read.is_empty() || !profile.filesystem.write.is_empty();
    let filesystem_has_deny_rules =
        !profile.filesystem.deny.is_empty() || !profile.filesystem.deny_globs.is_empty();
    if filesystem_has_allow_rules && filesystem_has_deny_rules {
        return Err(
            "deny_globs/deny filesystem rules cannot be represented by the legacy SandboxConfig runtime"
                .to_string(),
        );
    }

    if profile.network.enabled {
        for (target, mode) in [
            ("127.0.0.1:0", PermissionNetworkMode::Bind),
            ("127.0.0.1:11434", PermissionNetworkMode::Connect),
            ("192.168.1.10:8080", PermissionNetworkMode::Connect),
        ] {
            if profile.network.decision_for_target(target, mode)
                == PermissionNetworkDecision::Denied
            {
                return Err(
                    "network local/private target or bind rules cannot be represented by the legacy SandboxConfig runtime"
                        .to_string(),
                );
            }
        }
    }

    if !profile.unix_sockets.allow.is_empty()
        || !profile.unix_sockets.deny.is_empty()
        || profile.unix_sockets.audit_enabled
    {
        return Err(
            "unix socket allow/deny/audit rules cannot be represented by the legacy SandboxConfig runtime"
                .to_string(),
        );
    }

    Ok(())
}

pub fn validate_sandbox_config_permission_profile_v2_runtime_support(
    config: &SandboxConfig,
) -> Result<(), String> {
    let profile = PermissionProfileV2::from_legacy_sandbox(config);
    validate_permission_profile_v2_runtime_support(&profile)
}

/// The strongest profile the wired sandbox can actually enforce.
///
/// `Standard` and `Strict` both promise filesystem, syscall, network and
/// privilege containment (see `missing_required_containment_for_profile`), so a
/// platform missing any of them can honor only `Permissive`. Windows is that
/// platform: it enforces a restricted token and Job Object limits but no
/// filesystem, syscall or network isolation.
///
/// This exists because naming an unenforceable profile does not produce more
/// safety — the adapter refuses, and the user-confirmed action simply does not
/// run (#10665). Naming the strongest ENFORCEABLE profile keeps the audit
/// honest, because the profile recorded against the action is the one that was
/// applied, while leaving the feature reachable on the platform.
pub fn strongest_enforceable_profile(capabilities: &SandboxCapabilities) -> SandboxProfile {
    if capabilities.filesystem_isolation
        && capabilities.syscall_filtering
        && capabilities.network_isolation
        && capabilities.privilege_restriction
    {
        return SandboxProfile::Strict;
    }

    // Clamping down is only safe when the sandbox still contains the child in
    // the ways Permissive relies on: a separate process running with reduced
    // privilege. A sandbox offering neither (NoOpSandbox, FailClosedSandbox —
    // every capability false) would turn Permissive into in-process execution
    // with no containment at all, so those keep naming Strict and keep failing
    // closed, which is the correct answer for "we cannot contain this".
    if capabilities.process_isolation && capabilities.privilege_restriction {
        SandboxProfile::Permissive
    } else {
        SandboxProfile::Strict
    }
}

/// `default_strict_config` clamped to what the platform can enforce.
///
/// Used for trusted internal commands, where refusing outright means the
/// confirmed GUI action never executes at all.
pub fn enforceable_strict_config(
    base_config: &SandboxConfig,
    capabilities: &SandboxCapabilities,
) -> SandboxConfig {
    let profile = strongest_enforceable_profile(capabilities);
    if !matches!(profile, SandboxProfile::Strict) {
        tracing::warn!(
            applied_profile = ?profile,
            filesystem_isolation = capabilities.filesystem_isolation,
            syscall_filtering = capabilities.syscall_filtering,
            network_isolation = capabilities.network_isolation,
            privilege_restriction = capabilities.privilege_restriction,
            "platform cannot enforce Strict; applying the strongest enforceable profile \
             and auditing the action under it"
        );
    }
    SandboxConfig {
        profile,
        ..default_strict_config(base_config)
    }
}

pub fn default_strict_config(base_config: &SandboxConfig) -> SandboxConfig {
    SandboxConfig {
        // Preserve the operator's sandbox switch, mirroring `resolve_sandbox_config`
        // (`enabled: base_config.enabled` above). Forcing `enabled: true` here routed
        // every action to whatever sandbox was wired at construction — a `NoOpSandbox`
        // when the operator disabled the sandbox (the default state) — which silently
        // dropped the action yet reported `CommandResult::Success` and durably audited
        // it as `Completed`, corrupting the audit trail (#7476). When disabled, the
        // dispatcher's `!enabled` branch instead runs the action via the explicit
        // inline input driver, or fails closed when none is wired — never a silent
        // no-op success.
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
    use maekon_core::config::{PermissionAccess, PermissionNetworkDecision, PermissionNetworkMode};

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
            enabled: true,
            allowed_write_paths: vec!["/tmp".to_string()],
            allow_network: true,
            ..Default::default()
        };

        let strict = default_strict_config(&base);
        assert!(strict.enabled);
        assert!(matches!(strict.profile, SandboxProfile::Strict));
        assert!(strict.allowed_write_paths.is_empty());
        assert!(!strict.allow_network);
    }

    #[test]
    fn default_strict_preserves_operator_sandbox_switch() {
        // #7476: `default_strict_config` must mirror `resolve_sandbox_config` and
        // preserve `base_config.enabled`. Forcing `enabled: true` when the operator
        // disabled the sandbox routed the action to the wired `NoOpSandbox`, which
        // silently dropped it yet reported success. A disabled base must yield a
        // disabled strict config so the dispatcher's `!enabled` branch runs the
        // action inline (or fails closed) instead of silently no-op'ing.
        let disabled_base = SandboxConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!default_strict_config(&disabled_base).enabled);

        let enabled_base = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(default_strict_config(&enabled_base).enabled);
    }

    #[test]
    fn resolves_permission_profile_v2_from_effective_sandbox_config() {
        let mut policy = make_policy(AuditLevel::Basic, false);
        policy.allowed_paths = vec!["/tmp/extra".to_string()];
        policy.max_execution_time_ms = 3000;

        let base = SandboxConfig {
            allowed_read_paths: vec!["/usr/lib".to_string()],
            allowed_write_paths: vec!["/tmp/out".to_string()],
            max_memory_bytes: 1024,
            ..Default::default()
        };

        let profile = resolve_permission_profile_v2(&policy, &base);

        assert_eq!(
            profile.filesystem.access_for_path("/tmp/extra/readme.md"),
            PermissionAccess::Read
        );
        assert_eq!(
            profile.filesystem.access_for_path("/tmp/out/result.txt"),
            PermissionAccess::Write
        );
        assert_eq!(profile.max_memory_bytes, 1024);
        assert_eq!(profile.max_cpu_time_ms, 3000);
    }

    #[test]
    fn resolved_permission_profile_v2_keeps_secret_denies_after_policy_path_merge() {
        let mut policy = make_policy(AuditLevel::Basic, false);
        policy.allowed_paths = vec!["/workspace".to_string()];

        let profile = resolve_permission_profile_v2(&policy, &SandboxConfig::default());

        assert_eq!(
            profile.filesystem.access_for_path("/workspace/README.md"),
            PermissionAccess::Read
        );
        assert_eq!(
            profile.filesystem.access_for_path("/workspace/.env"),
            PermissionAccess::Denied
        );
    }

    #[test]
    fn resolved_permission_profile_v2_network_stays_denied_until_policy_enables_it() {
        let policy = make_policy(AuditLevel::Detailed, false);
        let profile = resolve_permission_profile_v2(&policy, &SandboxConfig::default());

        assert_eq!(
            profile
                .network
                .decision_for_target("api.openai.com", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Denied
        );

        let mut network_policy = make_policy(AuditLevel::Detailed, false);
        network_policy.allow_network = Some(true);
        let network_profile =
            resolve_permission_profile_v2(&network_policy, &SandboxConfig::default());

        assert_eq!(
            network_profile
                .network
                .decision_for_target("api.openai.com", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Allowed
        );
        assert_eq!(
            network_profile
                .network
                .decision_for_target("localhost:11434", PermissionNetworkMode::Connect),
            PermissionNetworkDecision::Denied
        );
    }

    #[test]
    fn permission_profile_v2_runtime_guard_rejects_secret_denies_legacy_cannot_enforce() {
        let mut policy = make_policy(AuditLevel::Basic, false);
        policy.allowed_paths = vec!["/workspace".to_string()];

        let profile = resolve_permission_profile_v2(&policy, &SandboxConfig::default());
        let err = validate_permission_profile_v2_runtime_support(&profile)
            .expect_err("legacy runtime cannot express allow path plus secret deny globs");

        assert!(err.contains("deny_globs"));
    }

    #[test]
    fn permission_profile_v2_runtime_guard_rejects_network_target_rules_legacy_cannot_enforce() {
        let mut policy = make_policy(AuditLevel::Detailed, false);
        policy.allow_network = Some(true);

        let profile = resolve_permission_profile_v2(&policy, &SandboxConfig::default());
        let err = validate_permission_profile_v2_runtime_support(&profile)
            .expect_err("legacy runtime cannot express V2 local/private network target rules");

        assert!(err.contains("network"));
    }

    fn capabilities(isolation: bool) -> SandboxCapabilities {
        SandboxCapabilities {
            filesystem_isolation: isolation,
            syscall_filtering: isolation,
            network_isolation: isolation,
            resource_limits: true,
            process_isolation: true,
            privilege_restriction: true,
        }
    }

    #[test]
    fn a_sandbox_that_contains_nothing_keeps_failing_closed() {
        // NoOpSandbox/FailClosedSandbox report every capability false. Clamping
        // those to Permissive would run the action in-process with no
        // containment, so they must keep naming Strict and refusing.
        let nothing = SandboxCapabilities {
            filesystem_isolation: false,
            syscall_filtering: false,
            network_isolation: false,
            resource_limits: false,
            process_isolation: false,
            privilege_restriction: false,
        };
        assert!(matches!(
            strongest_enforceable_profile(&nothing),
            SandboxProfile::Strict
        ));
        let config = enforceable_strict_config(&SandboxConfig::default(), &nothing);
        assert!(matches!(config.profile, SandboxProfile::Strict));
    }

    #[test]
    fn full_containment_still_yields_strict() {
        assert!(matches!(
            strongest_enforceable_profile(&capabilities(true)),
            SandboxProfile::Strict
        ));
        let config = enforceable_strict_config(&SandboxConfig::default(), &capabilities(true));
        assert!(matches!(config.profile, SandboxProfile::Strict));
    }

    #[test]
    fn windows_shaped_capabilities_yield_the_enforceable_profile() {
        // Windows: restricted token + Job Object, but no filesystem, syscall or
        // network isolation. Naming Strict there means the adapter refuses and
        // the user-confirmed action never runs (#10665).
        let windows = SandboxCapabilities {
            filesystem_isolation: false,
            syscall_filtering: false,
            network_isolation: false,
            resource_limits: true,
            process_isolation: true,
            privilege_restriction: true,
        };
        assert!(matches!(
            strongest_enforceable_profile(&windows),
            SandboxProfile::Permissive
        ));
        let config = enforceable_strict_config(&SandboxConfig::default(), &windows);
        assert!(matches!(config.profile, SandboxProfile::Permissive));
    }

    #[test]
    fn clamping_changes_only_the_profile() {
        // Everything else must still come from default_strict_config, so the
        // clamp cannot quietly widen writes, network or the operator's switch.
        let base = SandboxConfig {
            enabled: true,
            max_cpu_time_ms: 4_242,
            ..SandboxConfig::default()
        };
        let strict = default_strict_config(&base);
        let clamped = enforceable_strict_config(&base, &capabilities(false));

        assert!(matches!(clamped.profile, SandboxProfile::Permissive));
        assert_eq!(clamped.enabled, strict.enabled);
        assert_eq!(clamped.allowed_write_paths, strict.allowed_write_paths);
        assert!(clamped.allowed_write_paths.is_empty());
        assert_eq!(clamped.allow_network, strict.allow_network);
        assert!(!clamped.allow_network);
        assert_eq!(clamped.max_cpu_time_ms, strict.max_cpu_time_ms);
        assert_eq!(clamped.allowed_read_paths, strict.allowed_read_paths);
    }

    #[test]
    fn a_disabled_sandbox_switch_survives_clamping() {
        // The operator's off switch is preserved exactly as default_strict_config
        // preserves it (#7476): clamping must not turn a disabled sandbox into an
        // enabled permissive one.
        let base = SandboxConfig {
            enabled: false,
            ..SandboxConfig::default()
        };
        let clamped = enforceable_strict_config(&base, &capabilities(false));
        assert!(!clamped.enabled);
    }
}
