//! Cfg-free Windows sandbox resource-limit + token-restriction policy.
//!
//! The Win32 Job Object / restricted-token *enforcement* lives in `super::windows`
//! (target-gated FFI), but deciding the limits — how much memory/CPU/processes a
//! profile allows, and which token privileges to strip — is pure
//! `SandboxConfig` → struct logic. Extracting it here (no `#[cfg]`, no FFI) makes
//! that security policy unit-testable on any OS (#5138), so the per-profile
//! limits and the custom-override precedence are verified by the ordinary
//! ubuntu/macOS test run instead of only on a (billing-frozen, #4826) Windows
//! runner. Mirrors `super::sbpl` (the macOS SBPL seam).

// Consumed by the Windows sandbox adapter (`super::windows`) plus the tests
// below; it has no non-test caller on other targets.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use maekon_core::config::{SandboxConfig, SandboxProfile};

/// Resource limits applied to the child via a Win32 Job Object.
#[derive(Debug, Clone)]
pub(crate) struct JobObjectLimits {
    pub(crate) max_memory_bytes: u64,
    pub(crate) max_cpu_time_ms: u64,
    pub(crate) max_processes: u32,
}

/// Restricted-token policy (which SIDs/privileges to drop).
#[derive(Debug, Clone)]
pub(crate) struct TokenRestrictions {
    pub(crate) disable_admin_sid: bool,
    #[allow(dead_code)] // Reserved for future SID-level restrictions
    pub(crate) disable_most_sids: bool,
    pub(crate) remove_privileges: bool,
}

/// Resolve the Job Object limits for `config`: a per-profile default, with the
/// caller's explicit non-zero `max_memory_bytes` / `max_cpu_time_ms` overriding
/// it. `max_processes` is always the profile default (not user-overridable).
pub(crate) fn build_job_limits(config: &SandboxConfig) -> JobObjectLimits {
    let (default_memory, default_cpu_ms, default_max_processes) = match config.profile {
        SandboxProfile::Permissive => (0, 0, 0),
        SandboxProfile::Standard => (512 * 1024 * 1024, 30_000, 10), // 512MB, 30s, 10 processes
        SandboxProfile::Strict => (256 * 1024 * 1024, 10_000, 3),    // 256MB, 10s, 3 processes
    };

    JobObjectLimits {
        max_memory_bytes: if config.max_memory_bytes > 0 {
            config.max_memory_bytes
        } else {
            default_memory
        },
        max_cpu_time_ms: if config.max_cpu_time_ms > 0 {
            config.max_cpu_time_ms
        } else {
            default_cpu_ms
        },
        max_processes: default_max_processes,
    }
}

/// Resolve the restricted-token policy for `config`'s profile. `disable_admin_sid`
/// is always set (admin SID dropped on every tier); Standard and Strict
/// additionally restrict most SIDs and strip privileges.
pub(crate) fn build_token_restrictions(config: &SandboxConfig) -> TokenRestrictions {
    match config.profile {
        SandboxProfile::Permissive => TokenRestrictions {
            disable_admin_sid: true,
            disable_most_sids: false,
            remove_privileges: false,
        },
        SandboxProfile::Standard => TokenRestrictions {
            disable_admin_sid: true,
            disable_most_sids: true,
            remove_privileges: true,
        },
        SandboxProfile::Strict => TokenRestrictions {
            disable_admin_sid: true,
            disable_most_sids: true,
            remove_privileges: true,
        },
    }
}

/// Return the missing containment capabilities required by a Windows sandbox
/// profile. `Standard`/`Strict` must not silently degrade to Job Object resource
/// limits because callers interpret those profile names as filesystem, syscall,
/// and network containment.
pub(crate) fn missing_required_containment_for_profile(
    profile: SandboxProfile,
    filesystem_isolation: bool,
    syscall_filtering: bool,
    network_isolation: bool,
    privilege_restriction: bool,
) -> Option<String> {
    if matches!(profile, SandboxProfile::Permissive) {
        return None;
    }

    let mut missing = Vec::new();
    if !filesystem_isolation {
        missing.push("filesystem_isolation");
    }
    if !syscall_filtering {
        missing.push("syscall_filtering");
    }
    if !network_isolation {
        missing.push("network_isolation");
    }
    if !privilege_restriction {
        missing.push("privilege_restriction");
    }

    if missing.is_empty() {
        None
    } else {
        Some(missing.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(profile: SandboxProfile) -> SandboxConfig {
        SandboxConfig {
            profile,
            ..Default::default()
        }
    }

    #[test]
    fn job_limits_profile_defaults() {
        let standard = build_job_limits(&config(SandboxProfile::Standard));
        assert_eq!(standard.max_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(standard.max_cpu_time_ms, 30_000);
        assert_eq!(standard.max_processes, 10);

        let strict = build_job_limits(&config(SandboxProfile::Strict));
        assert_eq!(strict.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(strict.max_cpu_time_ms, 10_000);
        assert_eq!(strict.max_processes, 3);
    }

    #[test]
    fn job_limits_strict_is_tighter_than_standard() {
        // Security ordering: Strict must never grant MORE than Standard.
        let s = build_job_limits(&config(SandboxProfile::Standard));
        let x = build_job_limits(&config(SandboxProfile::Strict));
        assert!(x.max_memory_bytes < s.max_memory_bytes);
        assert!(x.max_cpu_time_ms < s.max_cpu_time_ms);
        assert!(x.max_processes < s.max_processes);
    }

    #[test]
    fn job_limits_permissive_is_unlimited() {
        let p = build_job_limits(&config(SandboxProfile::Permissive));
        assert_eq!(p.max_memory_bytes, 0);
        assert_eq!(p.max_cpu_time_ms, 0);
        assert_eq!(p.max_processes, 0);
    }

    #[test]
    fn job_limits_custom_overrides_memory_and_cpu_only() {
        let limits = build_job_limits(&SandboxConfig {
            profile: SandboxProfile::Strict,
            max_memory_bytes: 1024 * 1024 * 1024,
            max_cpu_time_ms: 60_000,
            ..Default::default()
        });
        assert_eq!(limits.max_memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_cpu_time_ms, 60_000);
        // max_processes is NOT user-overridable — stays the Strict default.
        assert_eq!(limits.max_processes, 3);
    }

    #[test]
    fn token_restrictions_per_profile() {
        let permissive = build_token_restrictions(&config(SandboxProfile::Permissive));
        assert!(
            permissive.disable_admin_sid,
            "admin SID is dropped on every tier"
        );
        assert!(!permissive.disable_most_sids);
        assert!(!permissive.remove_privileges);

        for profile in [SandboxProfile::Standard, SandboxProfile::Strict] {
            let r = build_token_restrictions(&config(profile));
            assert!(r.disable_admin_sid);
            assert!(r.disable_most_sids, "{profile:?} restricts most SIDs");
            assert!(r.remove_privileges, "{profile:?} strips privileges");
        }
    }

    #[test]
    fn standard_and_strict_require_windows_containment_capabilities() {
        for profile in [SandboxProfile::Standard, SandboxProfile::Strict] {
            let missing =
                missing_required_containment_for_profile(profile, false, false, false, false)
                    .expect("Standard/Strict must require containment capabilities");
            assert_eq!(
                missing,
                "filesystem_isolation, syscall_filtering, network_isolation, privilege_restriction"
            );
        }
    }

    #[test]
    fn standard_and_strict_accept_windows_when_all_required_capabilities_exist() {
        for profile in [SandboxProfile::Standard, SandboxProfile::Strict] {
            let missing = missing_required_containment_for_profile(profile, true, true, true, true);
            assert!(
                missing.is_none(),
                "complete containment capability set must satisfy {profile:?}"
            );
        }
    }

    #[test]
    fn permissive_does_not_require_windows_containment_capabilities() {
        let missing = missing_required_containment_for_profile(
            SandboxProfile::Permissive,
            false,
            false,
            false,
            false,
        );
        assert!(
            missing.is_none(),
            "Permissive may run resource-limited without containment"
        );
    }
}
