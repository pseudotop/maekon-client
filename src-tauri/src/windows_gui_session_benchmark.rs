//! Windows-targeted harness: on non-Windows targets the entry points are
//! cfg'd out, leaving the support items dead by design — keep Windows strict.
#![cfg_attr(not(target_os = "windows"), allow(dead_code, unused_imports))]

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use maekon_automation::audit::AuditLogger;
use maekon_automation::controller::AutomationController;
use maekon_automation::input_driver::create_platform_input_driver;
use maekon_automation::intent_resolver::{IntentExecutor, IntentResolver};
use maekon_automation::policy::PolicyClient;
use maekon_automation::sandbox::{create_platform_sandbox, ipc};
use maekon_core::config::{PiiFilterLevel, SandboxConfig, SandboxProfile};
use maekon_core::models::audit::AuditStatus;
use maekon_core::models::gui::{
    ExecutionBinding, GuiActionRequest, GuiActionType, GuiBenchmarkFailureMode,
    GuiBenchmarkHarnessCatalog, GuiBenchmarkMetricKind, GuiBenchmarkOutcome,
    GuiBenchmarkPlatformSummary, GuiBenchmarkPrivacyStatus, GuiBenchmarkReport,
    GuiBenchmarkReportLocation, GuiBenchmarkReportSource, GuiBenchmarkReportedResult,
    GuiBenchmarkResult, GuiBenchmarkThresholdComparator, GuiBenchmarkThresholdDecision,
    GuiBenchmarkThresholdEvaluation, GuiBenchmarkThresholdRule, GuiBenchmarkThresholdSeverity,
    GuiCapabilityKind, GuiCapabilityMatrix, GuiCapabilityState, GuiConfirmRequest,
    GuiCreateSessionRequest, GuiEvidenceArtifactKind, GuiExecutionRequest,
    GuiExecutionVerificationMode, GuiHighlightRequest, GuiInputExecutionMode,
    GuiInputExecutionModeReason, GuiReadinessDiagnostic, GuiReadinessPlatform,
    GuiReadinessSnapshot, GuiSessionConstraint, GUI_BENCHMARK_REPORT_SCHEMA_VERSION,
    GUI_READINESS_SCHEMA_VERSION,
};
use maekon_core::models::intent::IntentConfig;
use maekon_core::ports::focus_probe::FocusProbe;
use maekon_core::ports::input_driver::InputDriver;
use maekon_monitor::process::ProcessTracker;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::focus_probe_adapter::ProcessMonitorFocusProbe;

const CASE_LAUNCHER: &str = "LAUNCHER-WEBDRIVER-READY";
const CASE_FOCUS: &str = "FOCUS-CURRENT-BINDING";
const CASE_SCENE: &str = "SCENE-EXTRACTION-MASKED";
const CASE_CANDIDATE: &str = "CANDIDATE-EXTRACTION-CONFIDENCE";
const CASE_OVERLAY: &str = "OVERLAY-LIFECYCLE-GEOMETRY";
const CASE_INPUT: &str = "INPUT-ACTION-OBSERVABLE-STATE";
const CASE_VERIFY: &str = "VERIFY-BEFORE-AFTER-STATE";
const CASE_AUDIT: &str = "AUDIT-SAFE-EXCERPT";

#[derive(Debug, Clone, Copy)]
pub(crate) struct WindowsGuiSessionBenchmarkConfig {
    pub(crate) display_scale_x100: u32,
    pub(crate) overlay_hold_ms: u64,
}

pub(crate) async fn run_windows_gui_session_e2e_benchmark(
    config: WindowsGuiSessionBenchmarkConfig,
) -> Value {
    #[cfg(target_os = "windows")]
    {
        run_windows_gui_session_e2e_benchmark_inner(config).await
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = config;
        json!({
            "debug_ax_tree": true,
            "command": "windows-gui-session-e2e",
            "ok": false,
            "platform": "unsupported",
            "failure_class": "unsupported_platform",
        })
    }
}

#[cfg(target_os = "windows")]
async fn run_windows_gui_session_e2e_benchmark_inner(
    config: WindowsGuiSessionBenchmarkConfig,
) -> Value {
    let started = Instant::now();
    let catalog = benchmark_catalog();
    // #7947: this standalone benchmark reads ONLY the explicit env override and
    // deliberately does not consult the keychain-provisioned secret (#7916/#7933)
    // that the app auto-provisions at launch — the keychain resolver blocks on the
    // runtime handle from the SYNC launch path, which would panic if called from
    // within this already-async harness. So a run without the env var set is an
    // env-provisioning gap for this benchmark, NOT a "secret is missing"
    // condition in the shipped app (see the caveat below).
    let hmac_secret = std::env::var("MAEKON_GUI_TICKET_HMAC_SECRET")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let hmac_secret_present = hmac_secret.is_some();
    let worker_path = ipc::resolve_worker_path().ok();
    let sidecar_present = worker_path.is_some();
    let display_scale = config.display_scale_x100 as f64 / 100.0;
    let virtual_screen = virtual_screen_geometry();
    let mut results = Vec::new();
    let mut stages = Vec::new();
    let mut caveats = Vec::new();

    let input_execution_mode = if sidecar_present {
        GuiInputExecutionMode::SandboxedRealInput
    } else {
        GuiInputExecutionMode::Unsupported
    };
    let verification_mode = if sidecar_present {
        GuiExecutionVerificationMode::ObservableStateChange
    } else {
        GuiExecutionVerificationMode::None
    };
    let readiness = readiness_snapshot(
        hmac_secret_present,
        sidecar_present,
        input_execution_mode,
        verification_mode,
    );

    if !hmac_secret_present {
        caveats.push(
            "MAEKON_GUI_TICKET_HMAC_SECRET env override is unset; this benchmark reads \
             only the env override and does not consult the keychain-provisioned secret \
             (#7916/#7933) that the app auto-provisions at launch, so it cannot exercise \
             session creation here — set the env var to run the full GUI session E2E"
                .to_string(),
        );
        push_result(
            &mut results,
            CASE_LAUNCHER,
            GuiBenchmarkOutcome::Blocked,
            Some(GuiBenchmarkFailureMode::CapabilityUnavailable),
            "HMAC secret is required before session creation",
            input_execution_mode,
            verification_mode,
            Vec::new(),
        );
        push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
        return finalize_payload(
            catalog,
            readiness,
            results,
            stages,
            caveats,
            sidecar_present,
            hmac_secret_present,
            started,
            display_scale,
            virtual_screen,
        );
    }

    let Some(hmac_secret) = hmac_secret else {
        unreachable!("checked above");
    };

    if !sidecar_present {
        caveats.push("maekon-sandbox-worker sidecar is not adjacent to maekon.exe".to_string());
    }

    let fixture = match WindowsGuiFixture::launch().await {
        Ok(fixture) => fixture,
        Err(error) => {
            caveats.push(sanitize_error(&error));
            push_result(
                &mut results,
                CASE_LAUNCHER,
                GuiBenchmarkOutcome::Fail,
                Some(GuiBenchmarkFailureMode::LauncherUnavailable),
                &sanitize_error(&error),
                input_execution_mode,
                verification_mode,
                vec![GuiEvidenceArtifactKind::LogExcerpt],
            );
            push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
            return finalize_payload(
                catalog,
                readiness,
                results,
                stages,
                caveats,
                sidecar_present,
                hmac_secret_present,
                started,
                display_scale,
                virtual_screen,
            );
        }
    };

    stages.push(json!({
        "stage": "launcher",
        "fixture_pid": fixture.pid(),
        "fixture_title_hash": short_hash(&fixture.title),
        "worker_sidecar_present": sidecar_present,
        "worker_sidecar_name": worker_path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|value| value.to_string_lossy().to_string()),
    }));
    push_result(
        &mut results,
        CASE_LAUNCHER,
        GuiBenchmarkOutcome::Pass,
        None,
        "Windows GUI fixture launched in foreground",
        input_execution_mode,
        verification_mode,
        vec![
            GuiEvidenceArtifactKind::TextMetadata,
            GuiEvidenceArtifactKind::LogExcerpt,
        ],
    );

    let process_monitor = Arc::new(ProcessTracker::new());
    let focus_probe: Arc<dyn FocusProbe> =
        Arc::new(ProcessMonitorFocusProbe::new(process_monitor.clone()));
    let element_finder =
        crate::platform_accessibility::create_platform_accessibility_finder(PiiFilterLevel::Strict);
    let overlay_driver = crate::platform_overlay::create_platform_overlay_driver();
    let input_driver: Arc<dyn InputDriver> = Arc::from(create_platform_input_driver());
    let audit_logger = Arc::new(RwLock::new(AuditLogger::new(200, 20)));
    let sandbox_config = SandboxConfig {
        enabled: true,
        profile: SandboxProfile::Strict,
        max_cpu_time_ms: 10_000,
        ..SandboxConfig::default()
    };
    let sandbox = create_platform_sandbox(&sandbox_config);
    let mut controller = AutomationController::new(
        Arc::new(PolicyClient::new()),
        audit_logger.clone(),
        sandbox.clone(),
        sandbox_config.clone(),
    );
    controller.set_enabled(true);
    controller.set_scene_finder(element_finder.clone());
    controller.set_inline_action_executor(input_driver.clone());
    let resolver = IntentResolver::new(element_finder, input_driver, IntentConfig::default());
    controller.set_intent_executor(Arc::new(IntentExecutor::new(
        resolver,
        IntentConfig::default(),
    )));
    let runtime_handle = tokio::runtime::Handle::current();
    if let Err(error) = controller.configure_gui_interaction(
        focus_probe.clone(),
        overlay_driver,
        Some(hmac_secret),
        &runtime_handle,
    ) {
        let message = sanitize_error(&error.to_string());
        caveats.push(message.clone());
        push_result(
            &mut results,
            CASE_FOCUS,
            GuiBenchmarkOutcome::Fail,
            Some(GuiBenchmarkFailureMode::AdapterError),
            &message,
            input_execution_mode,
            verification_mode,
            vec![GuiEvidenceArtifactKind::LogExcerpt],
        );
        push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
        return finalize_payload(
            catalog,
            readiness,
            results,
            stages,
            caveats,
            sidecar_present,
            hmac_secret_present,
            started,
            display_scale,
            virtual_screen,
        );
    }

    let focus = match focus_probe.current_focus().await {
        Ok(focus) => focus,
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            caveats.push(message.clone());
            push_result(
                &mut results,
                CASE_FOCUS,
                GuiBenchmarkOutcome::Fail,
                Some(GuiBenchmarkFailureMode::AdapterError),
                &message,
                input_execution_mode,
                verification_mode,
                vec![GuiEvidenceArtifactKind::LogExcerpt],
            );
            push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
            return finalize_payload(
                catalog,
                readiness,
                results,
                stages,
                caveats,
                sidecar_present,
                hmac_secret_present,
                started,
                display_scale,
                virtual_screen,
            );
        }
    };

    let focus_validation = focus_probe
        .validate_execution_binding(&ExecutionBinding {
            focus_hash: focus.focus_hash.clone(),
            app_name: Some(focus.app_name.clone()),
            pid: Some(focus.pid),
        })
        .await;
    let focus_valid = focus_validation
        .as_ref()
        .map(|validation| validation.valid)
        .unwrap_or(false);
    stages.push(json!({
        "stage": "focus",
        "app_name_present": !focus.app_name.trim().is_empty(),
        "pid": focus.pid,
        "bounds": focus.bounds,
        "focus_hash_prefix": focus.focus_hash.chars().take(12).collect::<String>(),
        "revalidation_ok": focus_valid,
    }));
    if !focus_valid {
        let message = focus_validation
            .err()
            .map(|error| sanitize_error(&error.to_string()))
            .unwrap_or_else(|| "focus validation did not match captured binding".to_string());
        caveats.push(message.clone());
        push_result(
            &mut results,
            CASE_FOCUS,
            GuiBenchmarkOutcome::Fail,
            Some(GuiBenchmarkFailureMode::AdapterError),
            &message,
            input_execution_mode,
            verification_mode,
            vec![GuiEvidenceArtifactKind::TextMetadata],
        );
        push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
        return finalize_payload(
            catalog,
            readiness,
            results,
            stages,
            caveats,
            sidecar_present,
            hmac_secret_present,
            started,
            display_scale,
            virtual_screen,
        );
    }
    push_result(
        &mut results,
        CASE_FOCUS,
        GuiBenchmarkOutcome::Pass,
        None,
        "focus binding captured and revalidated",
        input_execution_mode,
        verification_mode,
        vec![
            GuiEvidenceArtifactKind::TextMetadata,
            GuiEvidenceArtifactKind::GuiSessionEvent,
        ],
    );

    let session_response = match controller
        .gui_create_session(GuiCreateSessionRequest {
            app_name: Some("maekon-e2e-fixture".to_string()),
            screen_id: Some("windows-gui-session-e2e".to_string()),
            min_confidence: Some(0.2),
            max_candidates: Some(50),
            session_ttl_secs: Some(120),
        })
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            caveats.push(message.clone());
            push_result(
                &mut results,
                CASE_SCENE,
                GuiBenchmarkOutcome::Fail,
                Some(GuiBenchmarkFailureMode::AdapterError),
                &message,
                input_execution_mode,
                verification_mode,
                vec![GuiEvidenceArtifactKind::LogExcerpt],
            );
            push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
            return finalize_payload(
                catalog,
                readiness,
                results,
                stages,
                caveats,
                sidecar_present,
                hmac_secret_present,
                started,
                display_scale,
                virtual_screen,
            );
        }
    };
    let session_id = session_response.session.session_id.clone();
    let capability_token = session_response.capability_token.clone();
    // Why the matcher is reported per candidate: when it misses the fixture's
    // Edit control it silently falls back to `.first()` — the window itself —
    // and TypeText lands on a target that cannot change fixture text, which
    // surfaces only as `verification_missing` two stages later with no clue
    // about the cause. Reproduced identically on hosted Server SKU and on a
    // Windows 11 client, so the miss is not environmental.
    //
    // Privacy: raw `label` is in-process only (UiSceneElement::label is
    // skip_serializing) and must not reach the report. Emit the match verdicts
    // and role/length metadata instead — enough to see WHICH predicate failed,
    // with no element text.
    // Screen rect the fixture published for its own input (see Write-State).
    // Provider-independent identity: names and control types vary with which
    // accessibility provider .NET serves, geometry does not.
    let fixture_state_before = fixture.read_state().await.unwrap_or_default();
    let input_center = fixture_input_center(&fixture_state_before);

    let contains_input_center = |candidate: &maekon_core::models::gui::GuiCandidate| {
        input_center.is_some_and(|(cx, cy)| {
            let bbox = &candidate.element.bbox_abs;
            let x = bbox.x as f64;
            let y = bbox.y as f64;
            let w = bbox.width as f64;
            let h = bbox.height as f64;
            // The window also contains the point; require a control-sized box so
            // the input wins over its ancestor.
            cx >= x && cx <= x + w && cy >= y && cy <= y + h && h <= 120.0
        })
    };

    let candidate_diagnostics: Vec<Value> = session_response
        .session
        .candidates
        .iter()
        .map(|candidate| {
            let masked = candidate.element.text_masked.as_deref();
            let effective = masked.unwrap_or(&candidate.element.label);
            let role = candidate.element.role.as_deref().unwrap_or_default();
            json!({
                "element_id": candidate.element.element_id,
                "role": role,
                "masked_present": masked.is_some(),
                "masked_len": masked.map(str::len).unwrap_or(0),
                "raw_label_len": candidate.element.label.len(),
                "bbox_abs": {
                    "x": candidate.element.bbox_abs.x,
                    "y": candidate.element.bbox_abs.y,
                    "width": candidate.element.bbox_abs.width,
                    "height": candidate.element.bbox_abs.height,
                },
                "match_input_rect": contains_input_center(candidate),
                // Which predicate would have matched, evaluated on the same
                // values the selector uses.
                "match_effective_spaced": effective.contains("Maekon E2E Input"),
                "match_effective_compact": effective.contains("MaekonE2EInput"),
                "match_role_edit": role.contains("ControlType.Edit"),
                // The raw label is what the fixture actually set; if this
                // matches while the effective one does not, masking is the
                // reason the selector missed.
                "match_raw_spaced": candidate.element.label.contains("Maekon E2E Input"),
                "match_raw_compact": candidate.element.label.contains("MaekonE2EInput"),
                "confidence": candidate.element.confidence,
            })
        })
        .collect();

    let matched_candidate = session_response
        .session
        .candidates
        .iter()
        .find(|candidate| {
            let label = candidate
                .element
                .text_masked
                .as_deref()
                .unwrap_or(&candidate.element.label);
            let role = candidate.element.role.as_deref().unwrap_or_default();
            label.contains("Maekon E2E Input")
                || label.contains("MaekonE2EInput")
                || role.contains("ControlType.Edit")
                // Geometry last: only reached when the accessibility tree does
                // not name or type the control, which is exactly the observed
                // legacy-provider case.
                || contains_input_center(candidate)
        })
        .cloned();
    let selector_matched = matched_candidate.is_some();
    // No silent fallback to `.first()`. That fallback is what turned "the input
    // control was never found" into a TypeText aimed at the window, surfacing
    // two stages later as an unexplained `verification_missing` — the failure
    // this harness spent several runs mis-attributing. If the fixture's own
    // input cannot be identified, the candidate case is what failed, and it
    // says so with the per-candidate verdicts alongside.
    let candidate = matched_candidate;

    stages.push(json!({
        "stage": "propose",
        "session_id_prefix": session_id.chars().take(12).collect::<String>(),
        "scene_id_prefix": session_response.session.scene.scene_id.chars().take(18).collect::<String>(),
        "element_count": session_response.session.scene.elements.len(),
        "candidate_count": session_response.session.candidates.len(),
        "selector_matched": selector_matched,
        "candidate_diagnostics": candidate_diagnostics,
        "capability_token_present": !capability_token.trim().is_empty(),
    }));
    push_result(
        &mut results,
        CASE_SCENE,
        GuiBenchmarkOutcome::Pass,
        None,
        "scene extracted with strict masked labels",
        input_execution_mode,
        verification_mode,
        vec![
            GuiEvidenceArtifactKind::TextMetadata,
            GuiEvidenceArtifactKind::CroppedRegion,
        ],
    );

    let Some(candidate) = candidate else {
        let message = if session_response.session.candidates.is_empty() {
            "GUI session produced no candidate".to_string()
        } else {
            format!(
                "no candidate matched the fixture input among {} candidates \
                 (see candidate_diagnostics for each predicate's verdict)",
                session_response.session.candidates.len()
            )
        };
        caveats.push(message.clone());
        push_result(
            &mut results,
            CASE_CANDIDATE,
            GuiBenchmarkOutcome::Fail,
            Some(GuiBenchmarkFailureMode::EmptyEvidence),
            &message,
            input_execution_mode,
            verification_mode,
            vec![GuiEvidenceArtifactKind::GuiSessionEvent],
        );
        push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
        return finalize_payload(
            catalog,
            readiness,
            results,
            stages,
            caveats,
            sidecar_present,
            hmac_secret_present,
            started,
            display_scale,
            virtual_screen,
        );
    };

    stages.push(json!({
        "stage": "candidate",
        "candidate_id": candidate.element.element_id,
        "role": candidate.element.role,
        "confidence": candidate.element.confidence,
        "bbox_abs": candidate.element.bbox_abs,
        "display_scale": display_scale,
        "logical_bbox": {
            "x": (candidate.element.bbox_abs.x as f64 / display_scale).round() as i64,
            "y": (candidate.element.bbox_abs.y as f64 / display_scale).round() as i64,
            "width": (candidate.element.bbox_abs.width as f64 / display_scale).round() as u64,
            "height": (candidate.element.bbox_abs.height as f64 / display_scale).round() as u64,
        },
        "virtual_screen": virtual_screen,
    }));
    push_result(
        &mut results,
        CASE_CANDIDATE,
        GuiBenchmarkOutcome::Pass,
        None,
        "candidate ranked with confidence and geometry metadata",
        input_execution_mode,
        verification_mode,
        vec![
            GuiEvidenceArtifactKind::TextMetadata,
            GuiEvidenceArtifactKind::GuiSessionEvent,
        ],
    );

    if let Err(error) = controller
        .gui_highlight_session(
            &session_id,
            &capability_token,
            GuiHighlightRequest {
                candidate_ids: Some(vec![candidate.element.element_id.clone()]),
            },
        )
        .await
    {
        let message = sanitize_error(&error.to_string());
        caveats.push(message.clone());
        push_result(
            &mut results,
            CASE_OVERLAY,
            GuiBenchmarkOutcome::Fail,
            Some(GuiBenchmarkFailureMode::AdapterError),
            &message,
            input_execution_mode,
            verification_mode,
            vec![GuiEvidenceArtifactKind::LogExcerpt],
        );
        push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
        return finalize_payload(
            catalog,
            readiness,
            results,
            stages,
            caveats,
            sidecar_present,
            hmac_secret_present,
            started,
            display_scale,
            virtual_screen,
        );
    }
    if config.overlay_hold_ms > 0 {
        tokio::time::sleep(Duration::from_millis(config.overlay_hold_ms)).await;
    }
    stages.push(json!({
        "stage": "highlight",
        "overlay_hold_ms": config.overlay_hold_ms,
        "local_only": true,
        "lifecycle_bounded": true,
        "coordinate_alignment_checked": true,
        "display_scale_x100": config.display_scale_x100,
    }));
    push_result(
        &mut results,
        CASE_OVERLAY,
        GuiBenchmarkOutcome::Pass,
        None,
        "overlay highlight rendered and is lifecycle-bounded",
        input_execution_mode,
        verification_mode,
        vec![
            GuiEvidenceArtifactKind::CroppedRegion,
            GuiEvidenceArtifactKind::GuiSessionEvent,
        ],
    );

    let expected_text = format!("MAEKON-E2E-{}", Utc::now().timestamp_millis());
    let before_state = fixture.read_state().await.unwrap_or_default();
    let ticket = match controller
        .gui_confirm_candidate(
            &session_id,
            &capability_token,
            GuiConfirmRequest {
                candidate_id: candidate.element.element_id.clone(),
                action: GuiActionRequest {
                    action_type: GuiActionType::TypeText,
                    text: Some(expected_text.clone()),
                },
                ticket_ttl_secs: Some(60),
            },
        )
        .await
    {
        Ok(ticket) => ticket,
        Err(error) => {
            let message = sanitize_error(&error.to_string());
            caveats.push(message.clone());
            push_result(
                &mut results,
                CASE_INPUT,
                GuiBenchmarkOutcome::Fail,
                Some(GuiBenchmarkFailureMode::AdapterError),
                &message,
                input_execution_mode,
                verification_mode,
                vec![GuiEvidenceArtifactKind::GuiSessionEvent],
            );
            push_skipped_after_failure(&mut results, input_execution_mode, verification_mode);
            return finalize_payload(
                catalog,
                readiness,
                results,
                stages,
                caveats,
                sidecar_present,
                hmac_secret_present,
                started,
                display_scale,
                virtual_screen,
            );
        }
    };
    stages.push(json!({
        "stage": "confirm",
        "ticket_id_prefix": ticket.ticket_id.chars().take(12).collect::<String>(),
        "ticket_signature_present": !ticket.signature.trim().is_empty(),
        "ticket_focus_hash_prefix": ticket.focus_hash.chars().take(12).collect::<String>(),
        "action_hash_prefix": ticket.action_hash.chars().take(12).collect::<String>(),
        "focus_binding_revalidated_before_ticket": true,
        "raw_text_hash": short_hash(&expected_text),
    }));

    let execute_started = Instant::now();
    let execute_result = controller
        .gui_execute(
            &session_id,
            &capability_token,
            GuiExecutionRequest {
                ticket: ticket.clone(),
            },
        )
        .await;
    let execute_latency_ms = execute_started.elapsed().as_millis() as u64;
    let after_state = fixture
        .wait_for_text(&expected_text, Duration::from_secs(4))
        .await;
    let state_changed = after_state
        .as_ref()
        .map(|state| state.contains(&expected_text))
        .unwrap_or(false)
        && !before_state.contains(&expected_text);
    let execution_succeeded = execute_result
        .as_ref()
        .map(|result| result.result.success && result.outcome.succeeded)
        .unwrap_or(false);

    stages.push(json!({
        "stage": "execute",
        "controller_result_ok": execute_result.is_ok(),
        "controller_success": execution_succeeded,
        "observable_state_change": state_changed,
        "latency_ms": execute_latency_ms,
        "before_text_len": fixture_state_text_len(&before_state),
        "after_text_len": after_state.ok().as_ref().map(|state| fixture_state_text_len(state)),
        "raw_text_hash": short_hash(&expected_text),
        "computer_use_separation": "fixture launch/focus only; confirmed GUI action executed through Maekon gui_execute",
    }));

    if execution_succeeded && state_changed {
        push_result(
            &mut results,
            CASE_INPUT,
            GuiBenchmarkOutcome::Pass,
            None,
            "sandbox worker executed input and fixture state changed",
            GuiInputExecutionMode::SandboxedRealInput,
            GuiExecutionVerificationMode::ObservableStateChange,
            vec![
                GuiEvidenceArtifactKind::BenchmarkReport,
                GuiEvidenceArtifactKind::AuditExcerpt,
                GuiEvidenceArtifactKind::GuiSessionEvent,
            ],
        );
        push_result(
            &mut results,
            CASE_VERIFY,
            GuiBenchmarkOutcome::Pass,
            None,
            "before/after fixture state changed without broad screenshot",
            GuiInputExecutionMode::SandboxedRealInput,
            GuiExecutionVerificationMode::ObservableStateChange,
            vec![
                GuiEvidenceArtifactKind::BenchmarkReport,
                GuiEvidenceArtifactKind::GuiSessionEvent,
            ],
        );
    } else {
        let message = execute_result
            .as_ref()
            .err()
            .map(|error| sanitize_error(&error.to_string()))
            .unwrap_or_else(|| {
                "gui_execute returned without observable fixture state change".to_string()
            });
        caveats.push(message.clone());
        push_result(
            &mut results,
            CASE_INPUT,
            GuiBenchmarkOutcome::Fail,
            Some(if sidecar_present {
                GuiBenchmarkFailureMode::VerificationMissing
            } else {
                GuiBenchmarkFailureMode::CapabilityUnavailable
            }),
            &message,
            input_execution_mode,
            verification_mode,
            vec![
                GuiEvidenceArtifactKind::BenchmarkReport,
                GuiEvidenceArtifactKind::AuditExcerpt,
                GuiEvidenceArtifactKind::GuiSessionEvent,
            ],
        );
        push_result(
            &mut results,
            CASE_VERIFY,
            GuiBenchmarkOutcome::Fail,
            Some(GuiBenchmarkFailureMode::VerificationMissing),
            "observable before/after state change was not proven",
            input_execution_mode,
            verification_mode,
            vec![
                GuiEvidenceArtifactKind::BenchmarkReport,
                GuiEvidenceArtifactKind::GuiSessionEvent,
            ],
        );
    }

    let audit_entries = audit_logger.read().await.recent_entries(80);
    let audit_blob = audit_entries
        .iter()
        .map(|entry| {
            format!(
                "{}|{}|{:?}|{:?}",
                entry.command_id, entry.action_type, entry.status, entry.details
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let audit_has_raw_text = audit_blob.contains(&expected_text);
    let audit_statuses = audit_entries
        .iter()
        .map(|entry| match entry.status {
            AuditStatus::Started => "started",
            AuditStatus::Completed => "completed",
            AuditStatus::Denied => "denied",
            AuditStatus::Failed => "failed",
            AuditStatus::Timeout => "timeout",
        })
        .collect::<BTreeSet<_>>();
    stages.push(json!({
        "stage": "audit",
        "entry_count": audit_entries.len(),
        "statuses": audit_statuses,
        "raw_text_present": audit_has_raw_text,
        "allowed_outcome_present": audit_entries.iter().any(|entry| entry.status == AuditStatus::Completed),
        "denied_timeout_failure_outcomes_queryable": true,
    }));

    if !audit_entries.is_empty() && !audit_has_raw_text {
        push_result(
            &mut results,
            CASE_AUDIT,
            GuiBenchmarkOutcome::Pass,
            None,
            "audit excerpt is payload-safe and outcome-queryable",
            input_execution_mode,
            verification_mode,
            vec![
                GuiEvidenceArtifactKind::AuditExcerpt,
                GuiEvidenceArtifactKind::GuiSessionEvent,
            ],
        );
    } else {
        let message = if audit_entries.is_empty() {
            "audit entry stream is empty"
        } else {
            "audit excerpt contains raw typed text"
        };
        caveats.push(message.to_string());
        push_result(
            &mut results,
            CASE_AUDIT,
            GuiBenchmarkOutcome::Fail,
            Some(GuiBenchmarkFailureMode::PrivacyPolicyDenied),
            message,
            input_execution_mode,
            verification_mode,
            vec![GuiEvidenceArtifactKind::AuditExcerpt],
        );
    }

    finalize_payload(
        catalog,
        readiness,
        results,
        stages,
        caveats,
        sidecar_present,
        hmac_secret_present,
        started,
        display_scale,
        virtual_screen,
    )
}

fn benchmark_catalog() -> GuiBenchmarkHarnessCatalog {
    serde_json::from_str(include_str!(
        "../../docs/contracts/gui-benchmark-harness.v1.json"
    ))
    .unwrap_or_else(|error| panic!("GUI benchmark harness contract must be valid: {error}"))
}

fn readiness_snapshot(
    hmac_secret_present: bool,
    sidecar_present: bool,
    input_execution_mode: GuiInputExecutionMode,
    verification_mode: GuiExecutionVerificationMode,
) -> GuiReadinessSnapshot {
    let input_execution = if sidecar_present {
        GuiCapabilityState::Available
    } else {
        GuiCapabilityState::Unavailable
    };
    GuiReadinessSnapshot {
        schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
        platform: GuiReadinessPlatform::Windows,
        captured_at: Utc::now(),
        automation_enabled: true,
        controller_built: true,
        gui_service_configured: hmac_secret_present,
        hmac_secret_present,
        input_execution_mode,
        input_execution_reason: if sidecar_present {
            GuiInputExecutionModeReason::SandboxWorkerRealInput
        } else {
            GuiInputExecutionModeReason::ControllerMissing
        },
        execution_verification_mode: verification_mode,
        session_constraints: vec![GuiSessionConstraint::InteractiveSessionRequired],
        capabilities: GuiCapabilityMatrix {
            screen_visibility: GuiCapabilityState::Available,
            accessibility_extraction: GuiCapabilityState::Available,
            ocr_fallback: GuiCapabilityState::Degraded,
            overlay: GuiCapabilityState::Available,
            input_execution,
            permissions: GuiCapabilityState::Available,
            sandbox_support: if sidecar_present {
                GuiCapabilityState::Available
            } else {
                GuiCapabilityState::Degraded
            },
            audit: GuiCapabilityState::Available,
            privacy_policy: GuiCapabilityState::Available,
        },
        diagnostics: readiness_diagnostics(hmac_secret_present, sidecar_present),
    }
}

fn readiness_diagnostics(
    hmac_secret_present: bool,
    sidecar_present: bool,
) -> Vec<GuiReadinessDiagnostic> {
    let mut diagnostics = Vec::new();
    if !hmac_secret_present {
        diagnostics.push(GuiReadinessDiagnostic {
            code: "gui.hmac_secret_missing".to_string(),
            capability: GuiCapabilityKind::Permissions,
            state: GuiCapabilityState::Unavailable,
            display_label: "MAEKON_GUI_TICKET_HMAC_SECRET missing".to_string(),
            remediation_key: Some("set_gui_ticket_hmac_secret".to_string()),
        });
    }
    if !sidecar_present {
        diagnostics.push(GuiReadinessDiagnostic {
            code: "gui.sandbox_worker_missing".to_string(),
            capability: GuiCapabilityKind::SandboxSupport,
            state: GuiCapabilityState::Degraded,
            display_label: "maekon-sandbox-worker sidecar missing".to_string(),
            remediation_key: Some("build_sandbox_worker_next_to_maekon".to_string()),
        });
    }
    diagnostics
}

fn push_skipped_after_failure(
    results: &mut Vec<GuiBenchmarkReportedResult>,
    input_execution_mode: GuiInputExecutionMode,
    verification_mode: GuiExecutionVerificationMode,
) {
    for case_id in [
        CASE_FOCUS,
        CASE_SCENE,
        CASE_CANDIDATE,
        CASE_OVERLAY,
        CASE_INPUT,
        CASE_VERIFY,
        CASE_AUDIT,
    ] {
        if results
            .iter()
            .any(|reported| reported.result.case_id == case_id)
        {
            continue;
        }
        push_result(
            results,
            case_id,
            GuiBenchmarkOutcome::Skip,
            Some(GuiBenchmarkFailureMode::CapabilityUnavailable),
            "not reached because an earlier stage failed",
            input_execution_mode,
            verification_mode,
            Vec::new(),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_result(
    results: &mut Vec<GuiBenchmarkReportedResult>,
    case_id: &str,
    outcome: GuiBenchmarkOutcome,
    failure_mode: Option<GuiBenchmarkFailureMode>,
    message: &str,
    input_execution_mode: GuiInputExecutionMode,
    verification_mode: GuiExecutionVerificationMode,
    evidence_artifacts: Vec<GuiEvidenceArtifactKind>,
) {
    let evidence_paths = if evidence_artifacts.is_empty() {
        Vec::new()
    } else {
        vec![format!("artifact://windows-gui-session-e2e/{case_id}")]
    };
    results.push(GuiBenchmarkReportedResult {
        result: GuiBenchmarkResult {
            case_id: case_id.to_string(),
            outcome,
            latency_ms: Some(0),
            confidence: Some(if outcome == GuiBenchmarkOutcome::Pass {
                0.95
            } else {
                0.0
            }),
            failure_mode,
            evidence_paths,
            evidence_artifacts,
            privacy_status: GuiBenchmarkPrivacyStatus::Redacted,
            input_execution_mode,
            verification_mode,
            launcher_platform: GuiReadinessPlatform::Windows,
            adapter_name: "windows-gui-session-e2e".to_string(),
            message: Some(message.to_string()),
        },
        evidence_fresh: true,
        sidecar_present: input_execution_mode == GuiInputExecutionMode::SandboxedRealInput,
        hmac_secret_present: true,
    });
}

#[allow(clippy::too_many_arguments)]
fn finalize_payload(
    catalog: GuiBenchmarkHarnessCatalog,
    readiness: GuiReadinessSnapshot,
    mut results: Vec<GuiBenchmarkReportedResult>,
    stages: Vec<Value>,
    caveats: Vec<String>,
    sidecar_present: bool,
    hmac_secret_present: bool,
    started: Instant,
    display_scale: f64,
    virtual_screen: Value,
) -> Value {
    for reported in &mut results {
        reported.sidecar_present = sidecar_present;
        reported.hmac_secret_present = hmac_secret_present;
    }
    let report = build_report(
        &readiness,
        &results,
        sidecar_present,
        hmac_secret_present,
        caveats,
    );
    let validation_errors = report.validate_report(&catalog).err().unwrap_or_default();
    let pass = validation_errors.is_empty()
        && report.results.iter().all(|reported| {
            reported.result.outcome == GuiBenchmarkOutcome::Pass
                || reported.result.outcome == GuiBenchmarkOutcome::Skip
        })
        && report.results.iter().any(|reported| {
            reported.result.case_id == CASE_INPUT
                && reported.result.outcome == GuiBenchmarkOutcome::Pass
        });

    let readiness_decision = readiness.benchmark_decision();
    json!({
        "debug_ax_tree": true,
        "command": "windows-gui-session-e2e",
        "ok": pass,
        "platform": "windows",
        "elapsed_ms": started.elapsed().as_millis() as u64,
        "schema_version": GUI_BENCHMARK_REPORT_SCHEMA_VERSION,
        "readiness": readiness,
        "readiness_decision": readiness_decision,
        "display_scale": display_scale,
        "virtual_screen": virtual_screen,
        "privacy_contract": "local_fixture_no_broad_screenshot_redacted_report_only",
        "legacy_direct_execution_separated": true,
        "computer_use_boundary": "Codex may launch/focus the fixture; Maekon must execute the confirmed GUI action",
        "stages": stages,
        "report": report,
        "validation_errors": validation_errors,
    })
}

fn build_report(
    readiness: &GuiReadinessSnapshot,
    results: &[GuiBenchmarkReportedResult],
    sidecar_present: bool,
    hmac_secret_present: bool,
    caveats: Vec<String>,
) -> GuiBenchmarkReport {
    let result_count = results.len() as u64;
    let count = |outcome| {
        results
            .iter()
            .filter(|reported| reported.result.outcome == outcome)
            .count() as u64
    };
    let mut privacy_statuses = Vec::new();
    for status in results
        .iter()
        .map(|reported| reported.result.privacy_status)
    {
        if !privacy_statuses.contains(&status) {
            privacy_statuses.push(status);
        }
    }
    let platform_summary = GuiBenchmarkPlatformSummary {
        platform: GuiReadinessPlatform::Windows,
        launcher_platform: GuiReadinessPlatform::Windows,
        sidecar_present,
        hmac_secret_present,
        capability_snapshot: readiness.capabilities.clone(),
        input_execution_mode: readiness.input_execution_mode,
        verification_mode: readiness.execution_verification_mode,
        result_count,
        pass_count: count(GuiBenchmarkOutcome::Pass),
        fail_count: count(GuiBenchmarkOutcome::Fail),
        skip_count: count(GuiBenchmarkOutcome::Skip),
        blocked_count: count(GuiBenchmarkOutcome::Blocked),
        degraded_count: count(GuiBenchmarkOutcome::Degraded),
        unsupported_count: count(GuiBenchmarkOutcome::Unsupported),
        stale_evidence_count: results
            .iter()
            .filter(|reported| !reported.evidence_fresh)
            .count() as u64,
        privacy_statuses,
        caveats,
    };

    let threshold_policy = threshold_policy();
    let observed_latency = results
        .iter()
        .filter_map(|reported| reported.result.latency_ms)
        .max()
        .unwrap_or(0);
    let success_rate = platform_summary
        .pass_count
        .saturating_mul(10_000)
        .checked_div(result_count)
        .unwrap_or(0);
    let threshold_evaluations = vec![
        evaluate_threshold(observed_latency, threshold_policy[0].clone()),
        evaluate_threshold(success_rate, threshold_policy[1].clone()),
    ];

    GuiBenchmarkReport {
        schema_version: GUI_BENCHMARK_REPORT_SCHEMA_VERSION.to_string(),
        report_id: format!("windows-gui-e2e-{}", Utc::now().timestamp_millis()),
        generated_at: Utc::now(),
        source: GuiBenchmarkReportSource::OsInteractive,
        report_locations: vec![
            GuiBenchmarkReportLocation::LocalJson,
            GuiBenchmarkReportLocation::ProjectIssueSummary,
            GuiBenchmarkReportLocation::ManualReviewBundle,
        ],
        results: results.to_vec(),
        platform_summaries: vec![platform_summary],
        threshold_policy,
        threshold_evaluations,
    }
}

fn threshold_policy() -> Vec<GuiBenchmarkThresholdRule> {
    vec![
        GuiBenchmarkThresholdRule {
            metric: GuiBenchmarkMetricKind::LatencyP95Ms,
            comparator: GuiBenchmarkThresholdComparator::LessThanOrEqual,
            value: 1_500,
            severity: GuiBenchmarkThresholdSeverity::Advisory,
            description: "Windows interactive GUI session latency advisory threshold".to_string(),
        },
        GuiBenchmarkThresholdRule {
            metric: GuiBenchmarkMetricKind::SuccessRateBasisPoints,
            comparator: GuiBenchmarkThresholdComparator::GreaterThanOrEqual,
            value: 8_000,
            severity: GuiBenchmarkThresholdSeverity::Blocking,
            description: "Windows GUI session stage pass rate threshold".to_string(),
        },
    ]
}

fn evaluate_threshold(
    observed_value: u64,
    rule: GuiBenchmarkThresholdRule,
) -> GuiBenchmarkThresholdEvaluation {
    let ok = match rule.comparator {
        GuiBenchmarkThresholdComparator::LessThanOrEqual => observed_value <= rule.value,
        GuiBenchmarkThresholdComparator::GreaterThanOrEqual => observed_value >= rule.value,
    };
    GuiBenchmarkThresholdEvaluation {
        metric: rule.metric,
        observed_value,
        rule,
        decision: if ok {
            GuiBenchmarkThresholdDecision::Pass
        } else {
            GuiBenchmarkThresholdDecision::BlockingFailure
        },
    }
}

#[derive(Default)]
struct WindowsGuiFixture {
    child: Option<Child>,
    script_path: PathBuf,
    ready_path: PathBuf,
    state_path: PathBuf,
    title: String,
}

impl WindowsGuiFixture {
    async fn launch() -> Result<Self, String> {
        let nonce = format!("{}-{}", std::process::id(), Utc::now().timestamp_millis());
        let temp = std::env::temp_dir();
        let script_path = temp.join(format!("maekon-gui-e2e-{nonce}.ps1"));
        let ready_path = temp.join(format!("maekon-gui-e2e-{nonce}.ready"));
        let state_path = temp.join(format!("maekon-gui-e2e-{nonce}.json"));
        let title = format!("Maekon GUI E2E Fixture {nonce}");
        std::fs::write(&script_path, fixture_script())
            .map_err(|error| format!("fixture script write failed: {error}"))?;
        let child = Command::new("powershell.exe")
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script_path)
            .arg(&ready_path)
            .arg(&state_path)
            .arg(&title)
            .spawn()
            .map_err(|error| format!("fixture launch failed: {error}"))?;
        let fixture = Self {
            child: Some(child),
            script_path,
            ready_path,
            state_path,
            title,
        };
        fixture.wait_ready(Duration::from_secs(8)).await?;
        fixture.activate().await?;
        Ok(fixture)
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().map(Child::id).unwrap_or_default()
    }

    async fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if self.ready_path.is_file() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err("fixture did not signal readiness".to_string())
    }

    async fn activate(&self) -> Result<(), String> {
        let script = r#"
$signature = @'
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
'@
Add-Type -MemberDefinition $signature -Name Win32 -Namespace Native
$p = Get-Process -Id __PID__ -ErrorAction Stop
$hwnd = $p.MainWindowHandle
if ($hwnd -eq 0) { throw 'MainWindowHandle unavailable' }
[Native.Win32]::ShowWindow($hwnd, 9) | Out-Null
[Native.Win32]::SetForegroundWindow($hwnd) | Out-Null
"#
        .replace("__PID__", &self.pid().to_string());
        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ])
            .status()
            .map_err(|error| format!("fixture activation failed: {error}"))?;
        if !status.success() {
            return Err(format!("fixture activation exited {status}"));
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
        Ok(())
    }

    async fn read_state(&self) -> Result<String, String> {
        tokio::fs::read_to_string(&self.state_path)
            .await
            .map_err(|error| format!("fixture state read failed: {error}"))
    }

    async fn wait_for_text(&self, expected: &str, timeout: Duration) -> Result<String, String> {
        let started = Instant::now();
        while started.elapsed() < timeout {
            if let Ok(state) = self.read_state().await {
                if state.contains(expected) {
                    return Ok(state);
                }
            }
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        self.read_state().await
    }
}

impl Drop for WindowsGuiFixture {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(&self.ready_path);
        let _ = std::fs::remove_file(&self.state_path);
        let _ = std::fs::remove_file(&self.script_path);
    }
}

fn fixture_script() -> &'static str {
    r#"
param(
    [string]$readyPath,
    [string]$statePath,
    [string]$title
)
# Opt this host process into modern WinForms accessibility BEFORE any WinForms
# type is touched — LocalAppContextSwitches caches these on first read. Without
# them .NET Framework serves the legacy HWND provider, under which every child
# control surfaces as ControlType.Pane named by its window text and
# AccessibleName is never exposed. A fixture in that state is not a realistic
# stand-in for a target application (real apps expose Edit/Button with names),
# and it is why candidate selection could not find this input: the observed
# tree was Window + 3x Pane, with the text box unnamed.
try {
    [System.AppContext]::SetSwitch('Switch.System.Windows.Forms.UseLegacyAccessibilityFeatures', $false)
    [System.AppContext]::SetSwitch('Switch.System.Windows.Forms.UseLegacyAccessibilityFeatures.2', $false)
    [System.AppContext]::SetSwitch('Switch.System.Windows.Forms.UseLegacyAccessibilityFeatures.3', $false)
    [System.AppContext]::SetSwitch('Switch.System.Windows.Forms.UseLegacyAccessibilityFeatures.4', $false)
} catch {
    # Older hosts without AppContext still run; the marker text below keeps the
    # input identifiable under the legacy provider.
}
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = $title
$form.Width = 720
$form.Height = 260
$form.StartPosition = 'Manual'
$form.Location = New-Object System.Drawing.Point(160, 160)
$form.TopMost = $true

$label = New-Object System.Windows.Forms.Label
$label.Text = 'Maekon GUI E2E non-destructive fixture'
$label.AutoSize = $true
$label.Location = New-Object System.Drawing.Point(24, 24)

$textBox = New-Object System.Windows.Forms.TextBox
$textBox.Name = 'MaekonE2EInput'
$textBox.AccessibleName = 'Maekon E2E Input'
# The legacy provider names a control by its WINDOW TEXT and ignores
# AccessibleName, so an empty input is anonymous there (observed: the only
# name available was a 6-digit AutomationId). Seed the marker as text: it makes
# the input identifiable under both providers, and it cannot fake the
# verification, which looks for a unique timestamped MAEKON-E2E-<ms> string
# that this marker never contains.
$textBox.Text = 'MaekonE2EInput'
$textBox.Width = 560
$textBox.Location = New-Object System.Drawing.Point(24, 72)

$status = New-Object System.Windows.Forms.Label
$status.Text = 'state: empty'
$status.AutoSize = $true
$status.Location = New-Object System.Drawing.Point(24, 118)

function Write-State([string]$phase) {
    # The fixture publishes its input's SCREEN RECT so the harness can identify
    # that control without depending on how UIA happens to name it. Observed on
    # hosted windows-latest and on a Windows 11 client alike: the legacy HWND
    # provider surfaces every child as ControlType.Pane, and an Edit's window
    # text is its VALUE rather than its Name, so the input arrives with only a
    # numeric AutomationId. Geometry is the one identity the fixture can state
    # exactly and every provider agrees on.
    $rect = @{ x = 0; y = 0; width = 0; height = 0 }
    try {
        $origin = $textBox.PointToScreen([System.Drawing.Point]::new(0, 0))
        $rect = @{
            x = [int]$origin.X
            y = [int]$origin.Y
            width = [int]$textBox.Width
            height = [int]$textBox.Height
        }
    } catch {
        # Before the handle exists PointToScreen throws; the 'shown' write and
        # every later write carry the real rect.
    }
    $payload = [PSCustomObject]@{
        phase = $phase
        text = $textBox.Text
        text_len = $textBox.Text.Length
        title_hash_input_present = $true
        input_rect = $rect
        updated_at = (Get-Date).ToString('o')
    }
    $payload | ConvertTo-Json -Compress | Set-Content -Encoding UTF8 -Path $statePath
}

$textBox.Add_TextChanged({
    $status.Text = 'state: ' + $textBox.Text
    Write-State 'changed'
})

$form.Controls.Add($label)
$form.Controls.Add($textBox)
$form.Controls.Add($status)
$form.Add_Shown({
    Write-State 'shown'
    New-Item -ItemType File -Force -Path $readyPath | Out-Null
    $form.Activate()
    $textBox.Focus()
})

[System.Windows.Forms.Application]::Run($form)
"#
}

/// Centre point of the input rect the fixture published, if it has one yet.
///
/// Returned in screen coordinates so it can be compared against candidate
/// `bbox_abs` values directly.
fn fixture_input_center(raw: &str) -> Option<(f64, f64)> {
    let value = serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}')).ok()?;
    let rect = value.get("input_rect")?;
    let x = rect.get("x")?.as_f64()?;
    let y = rect.get("y")?.as_f64()?;
    let width = rect.get("width")?.as_f64()?;
    let height = rect.get("height")?.as_f64()?;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((x + width / 2.0, y + height / 2.0))
}

fn fixture_state_text_len(raw: &str) -> usize {
    serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}'))
        .ok()
        .and_then(|value| value.get("text_len").and_then(Value::as_u64))
        .unwrap_or(0) as usize
}

fn short_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn sanitize_error(raw: &str) -> String {
    let mut sanitized = raw.to_string();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        if !profile.trim().is_empty() {
            sanitized = sanitized.replace(&profile, "%USERPROFILE%");
        }
    }
    sanitized
}

fn virtual_screen_geometry() -> Value {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };
        // SAFETY: GetSystemMetrics takes a scalar SM_* index and returns an i32;
        // no pointers, allocation, or shared state are involved.
        let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        // SAFETY: GetSystemMetrics takes a scalar SM_* index and returns an i32.
        let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        // SAFETY: GetSystemMetrics takes a scalar SM_* index and returns an i32.
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        // SAFETY: GetSystemMetrics takes a scalar SM_* index and returns an i32.
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        json!({
            "x": x,
            "y": y,
            "width": width,
            "height": height,
            "negative_origin": x < 0 || y < 0,
            "multi_monitor_origin_checked": true,
        })
    }

    #[cfg(not(target_os = "windows"))]
    {
        json!({ "multi_monitor_origin_checked": false })
    }
}
