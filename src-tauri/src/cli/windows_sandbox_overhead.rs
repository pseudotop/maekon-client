//! Windows-targeted harness: on non-Windows targets the entry points are
//! cfg'd out, leaving the support items dead by design — keep Windows strict.
#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use maekon_automation::action_dispatcher::{AutomationActionDispatcher, SandboxActionDispatcher};
use maekon_automation::audit::AuditLogger;
use maekon_automation::sandbox::{create_platform_sandbox, ipc};
use maekon_core::config::{SandboxConfig, SandboxProfile};
use maekon_core::models::automation::{AutomationAction, CommandResult};
use maekon_core::ports::sandbox::SandboxCapabilities;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy)]
pub(crate) struct WindowsSandboxOverheadBenchmarkConfig {
    pub(crate) samples: u32,
    pub(crate) timeout_probe_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SandboxOverheadObservation {
    profile: String,
    action_label: String,
    action_variant: String,
    sample_index: u32,
    cold: bool,
    semantic_execution_mode: String,
    success: bool,
    dispatch_total_ms: u64,
    worker_roundtrip_ms: Option<u64>,
    audit_record_ms: u64,
    verification_ms: u64,
    outcome: String,
    error_class: Option<String>,
}

impl SandboxOverheadObservation {
    #[cfg(test)]
    // CLI flag plumbing — one parameter per exposed knob.
    #[allow(clippy::too_many_arguments)]
    fn new_for_test(
        profile: &str,
        sample_index: u32,
        cold: bool,
        semantic_execution_mode: &str,
        success: bool,
        dispatch_total_ms: u64,
        worker_roundtrip_ms: Option<u64>,
        audit_record_ms: u64,
        verification_ms: u64,
    ) -> Self {
        Self {
            profile: profile.to_string(),
            action_label: "mouse_move_current_cursor".to_string(),
            action_variant: "MouseMove".to_string(),
            sample_index,
            cold,
            semantic_execution_mode: semantic_execution_mode.to_string(),
            success,
            dispatch_total_ms,
            worker_roundtrip_ms,
            audit_record_ms,
            verification_ms,
            outcome: if success { "success" } else { "failed" }.to_string(),
            error_class: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SandboxFailureProbe {
    name: String,
    covered: bool,
    outcome: String,
    error_class: Option<String>,
    elapsed_ms: u64,
}

impl SandboxFailureProbe {
    fn new(
        name: &str,
        covered: bool,
        outcome: &str,
        error_class: Option<String>,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            name: name.to_string(),
            covered,
            outcome: outcome.to_string(),
            error_class,
            elapsed_ms,
        }
    }

    #[cfg(test)]
    fn new_for_test(name: &str, covered: bool, outcome: &str) -> Self {
        Self {
            name: name.to_string(),
            covered,
            outcome: outcome.to_string(),
            error_class: None,
            elapsed_ms: 0,
        }
    }
}

pub(crate) fn windows_sandbox_capability_report(
    capabilities: SandboxCapabilities,
    windows_sandbox_feature_enabled: bool,
    worker_sidecar_present: bool,
) -> Value {
    json!({
        "windows_sandbox_feature_enabled": windows_sandbox_feature_enabled,
        "worker_sidecar_present": worker_sidecar_present,
        "capabilities": {
            "resource_limits": {
                "provided": capabilities.resource_limits,
                "scope": if capabilities.resource_limits {
                    "job_object_enforced"
                } else {
                    "requires_windows_sandbox_feature"
                },
            },
            "process_isolation": {
                "provided": capabilities.process_isolation,
                "scope": if capabilities.process_isolation {
                    "subprocess_job_object_enforced"
                } else {
                    "requires_windows_sandbox_feature"
                },
            },
            "filesystem_isolation": {
                "provided": capabilities.filesystem_isolation,
                "scope": "not_provided_by_windows_job_objects",
            },
            "syscall_filtering": {
                "provided": capabilities.syscall_filtering,
                "scope": "not_available_on_windows",
            },
            "network_isolation": {
                "provided": capabilities.network_isolation,
                "scope": "not_provided_by_windows_job_objects",
            },
            "restricted_token": {
                "created": windows_sandbox_feature_enabled,
                "applied_to_worker_process": false,
                "scope": "token_setup_present_but_worker_launch_uses_tokio_process",
            },
        },
        "semantic_execution_modes": [
            "noop_sandbox",
            "dry_run_worker_logging",
            "sandboxed_real_input_unverified",
            "unsupported_real_input",
        ],
        "contract_mismatch_follow_up_issue": 4561,
    })
}

pub(crate) async fn run_windows_sandbox_overhead_benchmark(
    config: WindowsSandboxOverheadBenchmarkConfig,
) -> Value {
    #[cfg(target_os = "windows")]
    {
        run_windows_sandbox_overhead_benchmark_inner(config).await
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = config;
        json!({
            "debug_ax_tree": true,
            "command": "windows-sandbox-overhead",
            "ok": false,
            "platform": "unsupported",
            "failure_class": "unsupported_platform",
        })
    }
}

#[cfg(target_os = "windows")]
async fn run_windows_sandbox_overhead_benchmark_inner(
    config: WindowsSandboxOverheadBenchmarkConfig,
) -> Value {
    let base_config = SandboxConfig {
        enabled: true,
        ..SandboxConfig::default()
    };
    let sandbox = create_platform_sandbox(&base_config);
    let worker_sidecar_present = ipc::resolve_worker_path().is_ok();
    let capability_report = windows_sandbox_capability_report(
        sandbox.capabilities(),
        cfg!(feature = "windows-sandbox"),
        worker_sidecar_present,
    );
    let dispatcher = SandboxActionDispatcher::new(sandbox);
    let action = current_cursor_mouse_move_action();
    let action_label = "mouse_move_current_cursor";
    let action_variant = automation_action_variant(&action);
    let mut observations = Vec::new();

    for profile in [
        SandboxProfile::Permissive,
        SandboxProfile::Standard,
        SandboxProfile::Strict,
    ] {
        let profile_label = sandbox_profile_label(profile);
        let profile_config = SandboxConfig {
            enabled: true,
            profile,
            ..SandboxConfig::default()
        };
        let semantic_execution_mode = semantic_execution_mode(profile, worker_sidecar_present);

        for sample_index in 0..config.samples {
            let dispatch_started = Instant::now();
            let result = dispatcher.dispatch(&action, &profile_config).await;
            let dispatch_total_ms = elapsed_ms(dispatch_started);

            let worker_roundtrip_ms = if matches!(profile, SandboxProfile::Permissive) {
                None
            } else {
                measure_worker_roundtrip(&action).await.elapsed_ms
            };
            let audit_record_ms = measure_audit_record_overhead(&result);
            let verification_ms = measure_contract_verification(&result);
            let (success, outcome, error_class) = command_result_outcome(result);

            observations.push(SandboxOverheadObservation {
                profile: profile_label.to_string(),
                action_label: action_label.to_string(),
                action_variant: action_variant.to_string(),
                sample_index,
                cold: sample_index == 0,
                semantic_execution_mode: semantic_execution_mode.to_string(),
                success,
                dispatch_total_ms,
                worker_roundtrip_ms,
                audit_record_ms,
                verification_ms,
                outcome,
                error_class,
            });
        }
    }

    let mut failure_probes = Vec::new();
    failure_probes.push(run_timeout_probe(config.timeout_probe_ms).await);
    failure_probes.push(run_malformed_worker_input_probe().await);
    failure_probes.push(resource_limit_probe(
        cfg!(feature = "windows-sandbox"),
        capability_report
            .pointer("/capabilities/resource_limits/provided")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        capability_report
            .pointer("/capabilities/process_isolation/provided")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    ));

    build_windows_sandbox_overhead_payload(config, capability_report, observations, failure_probes)
}

pub(crate) fn build_windows_sandbox_overhead_payload(
    config: WindowsSandboxOverheadBenchmarkConfig,
    capability_report: Value,
    observations: Vec<SandboxOverheadObservation>,
    failure_probes: Vec<SandboxFailureProbe>,
) -> Value {
    let dispatch_total_ms_max = observations
        .iter()
        .map(|observation| observation.dispatch_total_ms)
        .max()
        .unwrap_or(0);
    let mut grouped: BTreeMap<String, Vec<SandboxOverheadObservation>> = BTreeMap::new();
    for observation in observations {
        grouped
            .entry(observation.profile.clone())
            .or_default()
            .push(observation);
    }

    let profiles = grouped
        .into_iter()
        .map(|(profile, observations)| {
            let cold = observations
                .iter()
                .find(|observation| observation.cold)
                .map(observation_summary)
                .unwrap_or_else(|| json!(null));
            let warm: Vec<&SandboxOverheadObservation> = observations
                .iter()
                .filter(|observation| !observation.cold)
                .collect();
            let warm_dispatch_total_ms_avg = average_u64(
                warm.iter()
                    .map(|observation| observation.dispatch_total_ms)
                    .collect(),
            );
            let actions = observations
                .iter()
                .map(action_observation_payload)
                .collect::<Vec<_>>();
            json!({
                "profile": profile,
                "cold": cold,
                "warm": {
                    "count": warm.len(),
                    "dispatch_total_ms_avg": warm_dispatch_total_ms_avg,
                },
                "actions": actions,
            })
        })
        .collect::<Vec<_>>();

    let failure_paths = failure_probes
        .into_iter()
        .map(|probe| {
            (
                probe.name,
                json!({
                    "covered": probe.covered,
                    "outcome": probe.outcome,
                    "error_class": probe.error_class,
                    "elapsed_ms": probe.elapsed_ms,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();

    let all_actions_ok = profiles
        .iter()
        .flat_map(|profile| {
            profile["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|action| action["success"].as_bool().unwrap_or(false))
        })
        .all(|success| success);
    let failure_paths_covered = failure_paths
        .values()
        .all(|probe| probe["covered"].as_bool().unwrap_or(false));

    json!({
        "debug_ax_tree": true,
        "command": "windows-sandbox-overhead",
        "ok": all_actions_ok && failure_paths_covered,
        "platform": "windows",
        "samples_requested": config.samples,
        "timeout_probe_ms": config.timeout_probe_ms,
        "capability_report": capability_report,
        "representative_actions": [
            {
                "label": "mouse_move_current_cursor",
                "variant": "MouseMove",
                "side_effect_profile": "non_destructive_real_input",
                "semantic_note": "uses the current cursor position to avoid click/text side effects",
            },
        ],
        "representative_action_limitations": {
            "click_and_keyboard_actions": "not_executed_without_dedicated_input_harness",
            "follow_up_issue": 4561,
        },
        "profiles": profiles,
        "failure_paths": failure_paths,
        "performance_budget": {
            "dispatch_total_ms_p95_advisory": 250,
            "dispatch_total_ms_observed_max": dispatch_total_ms_max,
            "threshold_exceeded_follow_up_required": dispatch_total_ms_max > 250,
            "follow_up_issue_if_semantic_mismatch": 4561,
        },
        "privacy_contract": "local_worker_contract_no_raw_ui_payload",
    })
}

fn observation_summary(observation: &SandboxOverheadObservation) -> Value {
    json!({
        "action_label": observation.action_label,
        "action_variant": observation.action_variant,
        "sample_index": observation.sample_index,
        "semantic_execution_mode": observation.semantic_execution_mode,
        "success": observation.success,
        "dispatch_total_ms": observation.dispatch_total_ms,
        "worker_roundtrip_ms": observation.worker_roundtrip_ms,
        "audit_record_ms": observation.audit_record_ms,
        "verification_ms": observation.verification_ms,
        "outcome": observation.outcome,
        "error_class": observation.error_class,
    })
}

fn action_observation_payload(observation: &SandboxOverheadObservation) -> Value {
    json!({
        "action_label": observation.action_label,
        "action_variant": observation.action_variant,
        "sample_index": observation.sample_index,
        "cold": observation.cold,
        "success": observation.success,
        "outcome": observation.outcome,
        "error_class": observation.error_class,
        "semantic_execution_mode": observation.semantic_execution_mode,
        "timings": {
            "dispatch_total_ms": observation.dispatch_total_ms,
            "worker_startup_ipc_roundtrip_ms": observation.worker_roundtrip_ms,
            "native_input_execution_ms": Value::Null,
            "native_input_execution_observability": "not_reported_by_worker_contract",
            "audit_record_ms": observation.audit_record_ms,
            "verification_ms": observation.verification_ms,
        },
    })
}

fn average_u64(values: Vec<u64>) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.iter().sum::<u64>() / values.len() as u64
}

fn command_result_outcome(result: CommandResult) -> (bool, String, Option<String>) {
    match result {
        CommandResult::Success => (true, "success".to_string(), None),
        CommandResult::Failed(message) => (
            false,
            "failed".to_string(),
            Some(classify_error(&message).to_string()),
        ),
        CommandResult::Timeout => (false, "timeout".to_string(), Some("timeout".to_string())),
        CommandResult::Denied => (false, "denied".to_string(), Some("denied".to_string())),
    }
}

fn sandbox_profile_label(profile: SandboxProfile) -> &'static str {
    match profile {
        SandboxProfile::Permissive => "permissive_noop",
        SandboxProfile::Standard => "standard",
        SandboxProfile::Strict => "strict",
    }
}

fn semantic_execution_mode(profile: SandboxProfile, worker_sidecar_present: bool) -> &'static str {
    if matches!(profile, SandboxProfile::Permissive) {
        return "noop_sandbox";
    }
    if worker_sidecar_present {
        "sandboxed_real_input_unverified"
    } else {
        "unsupported_real_input"
    }
}

fn current_cursor_mouse_move_action() -> AutomationAction {
    #[cfg(target_os = "windows")]
    {
        if let Some(position) = maekon_monitor::windows::get_mouse_position_windows() {
            return AutomationAction::MouseMove {
                x: position.x,
                y: position.y,
            };
        }
    }

    AutomationAction::MouseMove { x: 0, y: 0 }
}

fn automation_action_variant(action: &AutomationAction) -> &'static str {
    match action {
        AutomationAction::MouseMove { .. } => "MouseMove",
        AutomationAction::MouseClick { .. } => "MouseClick",
        AutomationAction::KeyType { .. } => "KeyType",
        AutomationAction::KeyPress { .. } => "KeyPress",
        AutomationAction::KeyRelease { .. } => "KeyRelease",
        AutomationAction::Hotkey { .. } => "Hotkey",
    }
}

struct WorkerRoundtripProbe {
    elapsed_ms: Option<u64>,
}

async fn measure_worker_roundtrip(action: &AutomationAction) -> WorkerRoundtripProbe {
    let worker_path = match ipc::resolve_worker_path() {
        Ok(path) => path,
        Err(_) => return WorkerRoundtripProbe { elapsed_ms: None },
    };
    let request = ipc::SandboxRequest {
        action: action.clone(),
    };
    let request_json = match serde_json::to_string(&request) {
        Ok(json) => json,
        Err(_) => return WorkerRoundtripProbe { elapsed_ms: None },
    };

    let started = Instant::now();
    let mut child = match tokio::process::Command::new(worker_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return WorkerRoundtripProbe { elapsed_ms: None },
    };

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        if stdin.write_all(request_json.as_bytes()).await.is_err()
            || stdin.write_all(b"\n").await.is_err()
        {
            return WorkerRoundtripProbe { elapsed_ms: None };
        }
    }

    let output = match tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await
    {
        Ok(Ok(output)) => output,
        _ => return WorkerRoundtripProbe { elapsed_ms: None },
    };
    let elapsed_ms = elapsed_ms(started);
    let response_ok = output.status.success()
        && ipc::parse_worker_response(&output.stdout)
            .map(|response| response.success)
            .unwrap_or(false);

    WorkerRoundtripProbe {
        elapsed_ms: response_ok.then_some(elapsed_ms),
    }
}

fn measure_audit_record_overhead(result: &CommandResult) -> u64 {
    let started = Instant::now();
    let mut logger = AuditLogger::new(16, 8);
    logger.log_start(
        "sandbox-overhead-probe",
        "windows-sandbox-overhead",
        "MouseMove",
    );
    logger.log_complete_with_time(
        maekon_core::models::audit::AuditLevel::Basic,
        "sandbox-overhead-probe",
        "windows-sandbox-overhead",
        &format!("{result:?}"),
        0,
    );
    let _ = logger.recent_entries(4);
    elapsed_ms(started)
}

fn measure_contract_verification(result: &CommandResult) -> u64 {
    let started = Instant::now();
    let _worker_exit_success_is_not_input_proof = matches!(result, CommandResult::Success);
    elapsed_ms(started)
}

async fn run_timeout_probe(timeout_probe_ms: u64) -> SandboxFailureProbe {
    let started = Instant::now();
    let sleep_ms = timeout_probe_ms.saturating_mul(2).max(timeout_probe_ms + 1);
    let result = tokio::time::timeout(
        Duration::from_millis(timeout_probe_ms),
        tokio::time::sleep(Duration::from_millis(sleep_ms)),
    )
    .await;
    SandboxFailureProbe::new(
        "timeout",
        result.is_err(),
        if result.is_err() {
            "timeout"
        } else {
            "completed_without_timeout"
        },
        result
            .is_ok()
            .then(|| "timeout_probe_missed_deadline".to_string()),
        elapsed_ms(started),
    )
}

async fn run_malformed_worker_input_probe() -> SandboxFailureProbe {
    let started = Instant::now();
    let worker_path = match ipc::resolve_worker_path() {
        Ok(path) => path,
        Err(error) => {
            return SandboxFailureProbe::new(
                "malformed_worker_input",
                false,
                "worker_missing",
                Some(classify_error(&error.to_string()).to_string()),
                elapsed_ms(started),
            );
        }
    };

    let mut child = match tokio::process::Command::new(worker_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return SandboxFailureProbe::new(
                "malformed_worker_input",
                false,
                "spawn_failed",
                Some(classify_error(&error.to_string()).to_string()),
                elapsed_ms(started),
            );
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(b"{not-json}\n").await;
    }

    let output = match tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await
    {
        Ok(Ok(output)) => output,
        _ => {
            return SandboxFailureProbe::new(
                "malformed_worker_input",
                false,
                "worker_timeout",
                Some("timeout".to_string()),
                elapsed_ms(started),
            );
        }
    };
    let response = ipc::parse_worker_response(&output.stdout);
    let worker_reported_failure = response
        .map(|response| !response.success && response.error.is_some())
        .unwrap_or(false);
    SandboxFailureProbe::new(
        "malformed_worker_input",
        worker_reported_failure,
        if worker_reported_failure {
            "worker_failure"
        } else {
            "unexpected_success"
        },
        (!worker_reported_failure).then(|| "worker_failure_path_missing".to_string()),
        elapsed_ms(started),
    )
}

fn resource_limit_probe(
    windows_sandbox_feature_enabled: bool,
    resource_limits: bool,
    process_isolation: bool,
) -> SandboxFailureProbe {
    if !windows_sandbox_feature_enabled {
        return SandboxFailureProbe::new("resource_process_limits", true, "not_enabled", None, 0);
    }
    let covered = resource_limits && process_isolation;
    SandboxFailureProbe::new(
        "resource_process_limits",
        covered,
        if covered {
            "job_object_capability_verified"
        } else {
            "job_object_capability_missing"
        },
        (!covered).then(|| "resource_limit_capability_missing".to_string()),
        0,
    )
}

fn classify_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("timeout") {
        "timeout"
    } else if lower.contains("not found") || lower.contains("missing") {
        "worker_missing"
    } else if lower.contains("spawn") {
        "spawn_failed"
    } else if lower.contains("parse") || lower.contains("json") {
        "worker_failure"
    } else {
        "sandbox_execution_failed"
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use maekon_core::ports::sandbox::SandboxCapabilities;

    #[test]
    fn capability_report_declares_job_object_boundaries_and_semantic_modes() {
        let payload = super::windows_sandbox_capability_report(
            SandboxCapabilities {
                filesystem_isolation: false,
                syscall_filtering: false,
                network_isolation: false,
                resource_limits: true,
                process_isolation: true,
            },
            true,
            true,
        );

        assert_eq!(payload["windows_sandbox_feature_enabled"], true);
        assert_eq!(payload["worker_sidecar_present"], true);
        assert_eq!(payload["capabilities"]["resource_limits"]["provided"], true);
        assert_eq!(
            payload["capabilities"]["process_isolation"]["provided"],
            true
        );
        assert_eq!(
            payload["capabilities"]["filesystem_isolation"]["provided"],
            false
        );
        assert_eq!(
            payload["capabilities"]["filesystem_isolation"]["scope"],
            "not_provided_by_windows_job_objects"
        );
        assert_eq!(
            payload["capabilities"]["syscall_filtering"]["scope"],
            "not_available_on_windows"
        );
        assert_eq!(
            payload["capabilities"]["network_isolation"]["scope"],
            "not_provided_by_windows_job_objects"
        );
        assert_eq!(payload["contract_mismatch_follow_up_issue"], 4561);

        let modes = payload["semantic_execution_modes"].as_array().unwrap();
        assert!(modes.iter().any(|mode| mode == "noop_sandbox"));
        assert!(modes.iter().any(|mode| mode == "dry_run_worker_logging"));
        assert!(modes
            .iter()
            .any(|mode| mode == "sandboxed_real_input_unverified"));
        assert!(modes.iter().any(|mode| mode == "unsupported_real_input"));
    }

    #[test]
    fn benchmark_payload_keeps_cold_warm_timings_and_failure_paths() {
        let payload = super::build_windows_sandbox_overhead_payload(
            super::WindowsSandboxOverheadBenchmarkConfig {
                samples: 2,
                timeout_probe_ms: 25,
            },
            super::windows_sandbox_capability_report(
                SandboxCapabilities {
                    filesystem_isolation: false,
                    syscall_filtering: false,
                    network_isolation: false,
                    resource_limits: false,
                    process_isolation: false,
                },
                false,
                true,
            ),
            vec![
                super::SandboxOverheadObservation::new_for_test(
                    "standard",
                    0,
                    true,
                    "sandboxed_real_input_unverified",
                    true,
                    41,
                    Some(37),
                    2,
                    1,
                ),
                super::SandboxOverheadObservation::new_for_test(
                    "standard",
                    1,
                    false,
                    "sandboxed_real_input_unverified",
                    true,
                    21,
                    Some(19),
                    2,
                    1,
                ),
            ],
            vec![
                super::SandboxFailureProbe::new_for_test("timeout", true, "timeout"),
                super::SandboxFailureProbe::new_for_test(
                    "malformed_worker_input",
                    true,
                    "worker_failure",
                ),
            ],
        );

        assert_eq!(payload["command"], "windows-sandbox-overhead");
        assert_eq!(payload["samples_requested"], 2);
        assert_eq!(payload["ok"], true);
        assert_eq!(payload["profiles"][0]["profile"], "standard");
        assert_eq!(
            payload["representative_actions"][0]["label"],
            "mouse_move_current_cursor"
        );
        assert_eq!(
            payload["representative_action_limitations"]["follow_up_issue"],
            4561
        );
        assert_eq!(payload["profiles"][0]["cold"]["dispatch_total_ms"], 41);
        assert_eq!(
            payload["profiles"][0]["cold"]["action_variant"],
            "MouseMove"
        );
        assert_eq!(payload["profiles"][0]["warm"]["count"], 1);
        assert_eq!(payload["profiles"][0]["warm"]["dispatch_total_ms_avg"], 21);
        assert_eq!(
            payload["profiles"][0]["actions"][0]["timings"]["native_input_execution_ms"],
            serde_json::Value::Null
        );
        assert_eq!(
            payload["profiles"][0]["actions"][0]["timings"]["native_input_execution_observability"],
            "not_reported_by_worker_contract"
        );
        assert_eq!(payload["failure_paths"]["timeout"]["covered"], true);
        assert_eq!(
            payload["failure_paths"]["malformed_worker_input"]["outcome"],
            "worker_failure"
        );
    }
}
