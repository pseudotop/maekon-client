//! Debug CLI command runners (debug_assertions only).
//!
//! Each `run_debug_*` function executes a command and returns an exit code.
//! Platform-specific macOS code is gated with `#[cfg(target_os = "macos")]`.

use super::output::{
    append_debug_notification_audit_jsonl, debug_notification_audit_event_payload,
    debug_notification_cli_activation_output_path_from,
    debug_notification_cli_diagnostic_jsonl_path_from, emit_debug_ax_tree_cli_json,
    emit_debug_notification_cli_json, emit_debug_notification_cli_marker_json,
    emit_debug_permissions_cli_json, hold_debug_notification_cli_if_requested,
    hold_debug_permissions_cli_if_requested,
};
use super::parsers::{debug_notification_activation_route_from, debug_notification_backend_from};
use super::types::{
    DebugAutostartCliCommand, DebugAxTreeCliCommand, DebugNotificationBackend,
    DebugNotificationCliCommand, DebugPermissionsCliCommand, DebugPermissionsRuntimeCliCommand,
};

#[cfg(debug_assertions)]
pub(crate) fn run_debug_autostart_cli_command(command: DebugAutostartCliCommand) -> i32 {
    match command {
        DebugAutostartCliCommand::Status => match crate::autostart::is_autostart_enabled() {
            Ok(enabled) => {
                let capabilities = crate::autostart::detect_capabilities();
                println!(
                    "{{\"debug_autostart\":true,\"command\":\"status\",\"enabled\":{},\"supported\":{},\"environment\":\"{:?}\"}}",
                    enabled, capabilities.supported, capabilities.environment
                );
                0
            }
            Err(error) => {
                eprintln!("debug-autostart status failed: {error}");
                1
            }
        },
        DebugAutostartCliCommand::Enable => match crate::autostart::enable_autostart() {
            Ok(()) => {
                println!("{{\"debug_autostart\":true,\"command\":\"enable\",\"ok\":true}}");
                0
            }
            Err(error) => {
                eprintln!("debug-autostart enable failed: {error}");
                1
            }
        },
        DebugAutostartCliCommand::Disable => match crate::autostart::disable_autostart() {
            Ok(()) => {
                println!("{{\"debug_autostart\":true,\"command\":\"disable\",\"ok\":true}}");
                0
            }
            Err(error) => {
                eprintln!("debug-autostart disable failed: {error}");
                1
            }
        },
    }
}

#[cfg(debug_assertions)]
pub(crate) fn run_debug_notification_cli_command<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    command: DebugNotificationCliCommand,
) -> i32 {
    let payload = match command {
        DebugNotificationCliCommand::Status => {
            let snapshot = crate::desktop_permissions::get_desktop_permission_snapshot(app_handle);
            serde_json::json!({
                "debug_notification": true,
                "command": "status",
                "permissions": snapshot,
            })
        }
        DebugNotificationCliCommand::Request => {
            let started_payload = serde_json::json!({
                "debug_notification": true,
                "command": "request",
                "phase": "started",
            });
            let started_payload = serde_json::to_string(&started_payload).unwrap_or_else(|_| {
                "{\"debug_notification\":true,\"command\":\"request\",\"phase\":\"started\"}"
                    .to_string()
            });
            let marker_exit_code = emit_debug_notification_cli_marker_json(&started_payload);
            if marker_exit_code != 0 {
                return marker_exit_code;
            }

            match crate::desktop_permissions::request_desktop_notification_permission(app_handle) {
                Ok(snapshot) => serde_json::json!({
                    "debug_notification": true,
                    "command": "request",
                    "ok": true,
                    "permissions": snapshot,
                }),
                Err(error) => serde_json::json!({
                    "debug_notification": true,
                    "command": "request",
                    "ok": false,
                    "error": error,
                }),
            }
        }
        DebugNotificationCliCommand::Send => {
            let backend = debug_notification_backend_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_BACKEND")
                    .ok()
                    .as_deref(),
            );
            let activation_route = debug_notification_activation_route_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_ACTIVATION_ROUTE")
                    .ok()
                    .as_deref(),
            );
            let _activation_output_path = debug_notification_cli_activation_output_path_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_ACTIVATION_OUTPUT")
                    .ok()
                    .as_deref(),
            );
            let _diagnostic_jsonl_path = debug_notification_cli_diagnostic_jsonl_path_from(
                std::env::var("MAEKON_DEBUG_NOTIFICATION_DIAGNOSTIC_JSONL")
                    .ok()
                    .as_deref(),
            );
            let title = std::env::var("MAEKON_DEBUG_NOTIFICATION_TITLE")
                .unwrap_or_else(|_| "Maekon Debug".to_string());
            let body = std::env::var("MAEKON_DEBUG_NOTIFICATION_BODY")
                .unwrap_or_else(|_| "Debug notification body".to_string());

            let (ok, error_message) = match backend {
                DebugNotificationBackend::TauriPlugin => {
                    let notification =
                        tauri_plugin_notification::NotificationExt::notification(app_handle)
                            .builder()
                            .title(&title)
                            .body(&body);
                    match notification.show() {
                        Ok(()) => (true, None),
                        Err(error) => (false, Some(error.to_string())),
                    }
                }
                #[cfg(target_os = "macos")]
                DebugNotificationBackend::MacosUnuser => {
                    match super::macos_notify::show_debug_macos_unuser_notification(
                        app_handle,
                        &title,
                        &body,
                        activation_route.as_deref(),
                        _activation_output_path,
                        _diagnostic_jsonl_path,
                    ) {
                        Ok(()) => (true, None),
                        Err(error) => (false, Some(error)),
                    }
                }
                #[cfg(not(target_os = "macos"))]
                DebugNotificationBackend::MacosUnuser => (
                    false,
                    Some("macos-unuser backend requires macOS".to_string()),
                ),
            };

            let audit_payload = debug_notification_audit_event_payload(
                "send",
                backend,
                ok,
                &title,
                &body,
                activation_route.as_deref(),
            );
            let audit_exit_code = append_debug_notification_audit_jsonl(&audit_payload);
            if audit_exit_code != 0 {
                return audit_exit_code;
            }

            match error_message {
                None => serde_json::json!({
                    "debug_notification": true,
                    "command": "send",
                    "ok": true,
                    "backend": backend.as_str(),
                }),
                Some(error) => serde_json::json!({
                    "debug_notification": true,
                    "command": "send",
                    "ok": false,
                    "backend": backend.as_str(),
                    "error": error,
                }),
            }
        }
    };

    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"debug_notification\":true,\"ok\":false,\"error\":\"json serialization failed\"}"
            .to_string()
    });
    let exit_code = emit_debug_notification_cli_json(&payload);
    if exit_code == 0 {
        hold_debug_notification_cli_if_requested();
    }
    exit_code
}

#[cfg(debug_assertions)]
pub(crate) fn run_debug_permissions_cli_command(command: DebugPermissionsCliCommand) -> i32 {
    #[cfg(target_os = "macos")]
    {
        use maekon_core::ports::accessibility::AccessibilityExtractor;

        match command {
            DebugPermissionsCliCommand::Status => {
                let accessibility =
                    maekon_vision::accessibility::MacOsNativeAccessibility::new().has_permission();
                // SAFETY: FFI call into CoreGraphics (declared in the extern
                // block below). It takes no arguments and returns a `bool` by
                // value — a read-only preflight with no pointer/lifetime
                // obligations.
                let screen_capture = unsafe { CGPreflightScreenCaptureAccess() };
                let payload = format!(
                    "{{\"debug_permissions\":true,\"command\":\"status\",\"accessibility_granted\":{},\"screen_capture_granted\":{}}}",
                    accessibility, screen_capture
                );
                emit_debug_permissions_cli_json(&payload)
            }
            DebugPermissionsCliCommand::ScreenCaptureRequest => {
                // SAFETY: FFI call into CoreGraphics (declared in the extern
                // block below). It takes no arguments and returns a `bool` by
                // value, so there are no pointer/lifetime obligations.
                let granted = unsafe { CGRequestScreenCaptureAccess() };
                let payload = format!(
                    "{{\"debug_permissions\":true,\"command\":\"screen-capture-request\",\"granted\":{}}}",
                    granted
                );
                let exit_code = emit_debug_permissions_cli_json(&payload);
                if exit_code == 0 {
                    hold_debug_permissions_cli_if_requested();
                }
                exit_code
            }
            DebugPermissionsCliCommand::ScreenCaptureAttempt => {
                let payload = match maekon_vision::capture::ScreenCapture::new().capture_primary() {
                    Ok(image) => format!(
                        "{{\"debug_permissions\":true,\"command\":\"screen-capture-attempt\",\"ok\":true,\"width\":{},\"height\":{}}}",
                        image.width(),
                        image.height()
                    ),
                    Err(error) => {
                        let escaped_error = serde_json::to_string(&error.to_string())
                            .unwrap_or_else(|_| "\"capture error\"".to_string());
                        format!(
                            "{{\"debug_permissions\":true,\"command\":\"screen-capture-attempt\",\"ok\":false,\"error\":{}}}",
                            escaped_error
                        )
                    }
                };
                let exit_code = emit_debug_permissions_cli_json(&payload);
                if exit_code == 0 {
                    hold_debug_permissions_cli_if_requested();
                }
                exit_code
            }
            DebugPermissionsCliCommand::AccessibilityRequest => {
                let granted = maekon_vision::accessibility::MacOsNativeAccessibility::new()
                    .request_permission();
                let payload = format!(
                    "{{\"debug_permissions\":true,\"command\":\"accessibility-request\",\"granted\":{}}}",
                    granted
                );
                let exit_code = emit_debug_permissions_cli_json(&payload);
                if exit_code == 0 {
                    hold_debug_permissions_cli_if_requested();
                }
                exit_code
            }
            DebugPermissionsCliCommand::OpenAccessibilitySettings => {
                emit_debug_permissions_open_settings_json("accessibility")
            }
            DebugPermissionsCliCommand::OpenScreenCaptureSettings => {
                emit_debug_permissions_open_settings_json("screen_capture")
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("debug-permissions is only supported on macOS");
        let _ = command;
        2
    }
}

#[cfg(debug_assertions)]
pub(crate) fn run_debug_permissions_runtime_cli_command<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    command: DebugPermissionsRuntimeCliCommand,
) -> i32 {
    match command {
        DebugPermissionsRuntimeCliCommand::ScreenCaptureRequest => {
            let payload =
                match crate::desktop_permissions::request_desktop_screen_capture_permission(
                    app_handle,
                ) {
                    Ok(snapshot) => serde_json::json!({
                        "debug_permissions": true,
                        "command": "screen-capture-request-runtime",
                        "ok": true,
                        "snapshot": snapshot,
                    }),
                    Err(error) => serde_json::json!({
                        "debug_permissions": true,
                        "command": "screen-capture-request-runtime",
                        "ok": false,
                        "error": error,
                    }),
                };
            let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                "{\"debug_permissions\":true,\"command\":\"screen-capture-request-runtime\",\"ok\":false,\"error\":\"json serialization failed\"}".to_string()
            });
            let exit_code = emit_debug_permissions_cli_json(&payload);
            if exit_code == 0 {
                hold_debug_permissions_cli_if_requested();
            }
            exit_code
        }
        DebugPermissionsRuntimeCliCommand::ScreenCaptureWatch {
            samples,
            interval_ms,
            warmup_ms,
        } => super::permission_watch::run_debug_screen_capture_watch(
            app_handle,
            samples,
            interval_ms,
            warmup_ms,
        ),
    }
}

#[cfg(debug_assertions)]
pub(crate) fn run_debug_ax_tree_cli_command(command: DebugAxTreeCliCommand) -> i32 {
    match command {
        DebugAxTreeCliCommand::Extract {
            app_name,
            max_depth,
            max_elements,
        } => run_debug_ax_tree_extract_cli_command(app_name, max_depth, max_elements),
        DebugAxTreeCliCommand::WindowsUiaBenchmark {
            samples,
            max_depth,
            max_elements,
        } => run_debug_windows_uia_benchmark_cli_command(samples, max_depth, max_elements),
        DebugAxTreeCliCommand::WindowsOcrBenchmark {
            samples,
            display_scale_x100,
        } => run_debug_windows_ocr_benchmark_cli_command(samples, display_scale_x100),
        DebugAxTreeCliCommand::WindowsGuiSessionE2eBenchmark {
            display_scale_x100,
            overlay_hold_ms,
        } => run_debug_windows_gui_session_e2e_cli_command(display_scale_x100, overlay_hold_ms),
        DebugAxTreeCliCommand::WindowsSandboxOverheadBenchmark {
            samples,
            timeout_probe_ms,
        } => run_debug_windows_sandbox_overhead_cli_command(samples, timeout_probe_ms),
    }
}

#[cfg(debug_assertions)]
fn run_debug_ax_tree_extract_cli_command(
    app_name: String,
    max_depth: u32,
    max_elements: usize,
) -> i32 {
    #[cfg(target_os = "macos")]
    {
        use maekon_core::ports::accessibility::AccessibilityExtractor;

        let extractor = maekon_vision::accessibility::MacOsNativeAccessibility::new();
        let permission_granted = extractor.has_permission();

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let payload = serde_json::json!({
                    "debug_ax_tree": true,
                    "command": "extract",
                    "ok": false,
                    "requested_app_name": app_name,
                    "permission_granted": permission_granted,
                    "error_code": "internal.generic",
                    "error_message": format!("tokio runtime init failed: {error}"),
                });
                let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                    "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                        .to_string()
                });
                return emit_debug_ax_tree_cli_json(&payload);
            }
        };

        let result = runtime.block_on(extractor.extract_application_elements(
            &app_name,
            max_depth,
            max_elements,
            maekon_core::config::PiiFilterLevel::Standard,
            false,
        ));

        let payload = match result {
            Ok(elements) => serde_json::json!({
                "debug_ax_tree": true,
                "command": "extract",
                "ok": true,
                "requested_app_name": app_name,
                "permission_granted": permission_granted,
                "max_depth": max_depth,
                "max_elements": max_elements,
                "element_count": elements.len(),
                "elements": elements,
            }),
            Err(error) => serde_json::json!({
                "debug_ax_tree": true,
                "command": "extract",
                "ok": false,
                "requested_app_name": app_name,
                "permission_granted": permission_granted,
                "max_depth": max_depth,
                "max_elements": max_elements,
                "element_count": 0,
                "elements": [],
                "error_code": error.code(),
                "error_message": error.to_string(),
            }),
        };

        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
            "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                .to_string()
        });
        emit_debug_ax_tree_cli_json(&payload)
    }

    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("debug-ax-tree extract is only supported on macOS");
        let _ = (app_name, max_depth, max_elements);
        2
    }
}

#[cfg(debug_assertions)]
fn run_debug_windows_uia_benchmark_cli_command(
    samples: u32,
    max_depth: u32,
    max_elements: usize,
) -> i32 {
    #[cfg(target_os = "windows")]
    {
        use maekon_core::config::PiiFilterLevel;
        use maekon_core::models::focused_element::AccessibilityElement;
        use maekon_core::ports::accessibility::AccessibilityExtractor;
        use maekon_core::ports::element_finder::ElementFinder;
        use std::time::Instant;

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let payload = serde_json::json!({
                    "debug_ax_tree": true,
                    "command": "windows-uia-benchmark",
                    "ok": false,
                    "platform": "windows",
                    "error_code": "internal.generic",
                    "error_message": format!("tokio runtime init failed: {error}"),
                });
                let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                    "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                        .to_string()
                });
                return emit_debug_ax_tree_cli_json(&payload);
            }
        };

        let payload = runtime.block_on(async {
            let extractor = maekon_vision::accessibility::WindowsUiaAccessibility::new();
            let finder = crate::platform_accessibility::create_platform_accessibility_finder(
                PiiFilterLevel::Strict,
            );
            let mut sample_payloads = Vec::with_capacity(samples as usize);
            let mut successful_samples = 0_u32;

            for sample_index in 0..samples {
                let focused_started = Instant::now();
                let focused_result = extractor
                    .extract_focused_element(PiiFilterLevel::Standard, false)
                    .await;
                let focused_latency_ms = elapsed_ms(focused_started);

                let com_tree_started = Instant::now();
                let com_tree_result = extractor
                    .extract_window_elements(
                        max_depth,
                        max_elements,
                        PiiFilterLevel::Strict,
                        false,
                    )
                    .await;
                let com_tree_latency_ms = elapsed_ms(com_tree_started);

                let platform_started = Instant::now();
                let platform_scene_result = finder
                    .analyze_scene(None, Some("windows-uia-benchmark"))
                    .await;
                let platform_latency_ms = elapsed_ms(platform_started);

                let focused_ref = focused_result.as_ref().ok().and_then(Option::as_ref);
                let com_tree_ref: &[AccessibilityElement] = match &com_tree_result {
                    Ok(elements) => elements.as_slice(),
                    Err(_) => &[],
                };
                let platform_scene_ref = platform_scene_result.as_ref().ok();
                let summary =
                    crate::platform_accessibility::benchmark::summarize_windows_uia_observations(
                        focused_ref,
                        com_tree_ref,
                        platform_scene_ref,
                    );
                let sample_ok = focused_result.is_ok()
                    && com_tree_result.is_ok()
                    && platform_scene_result.is_ok()
                    && (summary.com_focused.success
                        || summary.com_tree.success
                        || summary.platform_scene.success);
                if sample_ok {
                    successful_samples += 1;
                }

                sample_payloads.push(serde_json::json!({
                    "sample_index": sample_index,
                    "ok": sample_ok,
                    "latency_ms": {
                        "com_focused": focused_latency_ms,
                        "com_tree": com_tree_latency_ms,
                        "platform_scene": platform_latency_ms,
                    },
                    "errors": {
                        "com_focused": focused_result.as_ref().err().map(debug_core_error_payload),
                        "com_tree": com_tree_result.as_ref().err().map(debug_core_error_payload),
                        "platform_scene": platform_scene_result.as_ref().err().map(debug_core_error_payload),
                    },
                    "summary": summary,
                }));
            }

            serde_json::json!({
                "debug_ax_tree": true,
                "command": "windows-uia-benchmark",
                "ok": successful_samples > 0,
                "platform": "windows",
                "samples_requested": samples,
                "samples_ok": successful_samples,
                "max_depth": max_depth,
                "max_elements": max_elements,
                "privacy_contract": "focused_standard_no_full_text_consent_candidate_strict_summary_only",
                "samples": sample_payloads,
            })
        });

        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
            "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                .to_string()
        });
        emit_debug_ax_tree_cli_json(&payload)
    }

    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("debug-ax-tree windows-uia-benchmark is only supported on Windows");
        let _ = (samples, max_depth, max_elements);
        2
    }
}

#[cfg(debug_assertions)]
fn run_debug_windows_ocr_benchmark_cli_command(samples: u32, display_scale_x100: u32) -> i32 {
    #[cfg(target_os = "windows")]
    {
        use maekon_core::config::PiiFilterLevel;
        use maekon_core::ports::ocr_provider::OcrProvider;
        use maekon_vision::native_ocr::benchmark::{
            summarize_windows_native_ocr_sample, WindowsNativeOcrSampleInput,
        };
        use std::time::Instant;

        let Some(provider) = maekon_vision::native_ocr::create_native_ocr() else {
            let payload = serde_json::json!({
                "debug_ax_tree": true,
                "command": "windows-ocr-benchmark",
                "ok": false,
                "platform": "windows",
                "error_code": "provider.ocr_failed",
                "error_message": "Windows native OCR provider is unavailable",
                "failure_class": "integration_defect",
            });
            let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                    .to_string()
            });
            return emit_debug_ax_tree_cli_json(&payload);
        };

        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let payload = serde_json::json!({
                    "debug_ax_tree": true,
                    "command": "windows-ocr-benchmark",
                    "ok": false,
                    "platform": "windows",
                    "error_code": "internal.generic",
                    "error_message": format!("tokio runtime init failed: {error}"),
                    "failure_class": "runtime_join_failure",
                });
                let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                    "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                        .to_string()
                });
                return emit_debug_ax_tree_cli_json(&payload);
            }
        };

        let display_scale = display_scale_x100 as f64 / 100.0;
        let cases = windows_ocr_benchmark_cases();
        let payload = runtime.block_on(async {
            let mut observations = Vec::with_capacity(samples as usize * cases.len());
            let mut successful_observations = 0_usize;
            let mut successful_sample_sets = 0_u32;

            for sample_index in 0..samples {
                let mut sample_set_ok = true;

                for case in &cases {
                    let image = match generate_windows_ocr_benchmark_image(case) {
                        Ok(image) => image,
                        Err(error) => {
                            let generation_error = maekon_core::error::CoreError::OcrError {
                                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                                message: format!("benchmark image generation failed: {error}"),
                            };
                            let summary = summarize_windows_native_ocr_sample(
                                WindowsNativeOcrSampleInput {
                                    sample_label: case.label,
                                    image_width: case.width,
                                    image_height: case.height,
                                    image_format: "png",
                                    display_scale,
                                    latency_ms: 0,
                                },
                                Err(&generation_error),
                                PiiFilterLevel::Strict,
                            );
                            sample_set_ok = false;
                            observations.push(serde_json::json!({
                                "sample_index": sample_index,
                                "reference_kind": case.reference_kind,
                                "image_source": "controlled_local_synthetic_crop",
                                "summary": summary,
                            }));
                            continue;
                        }
                    };

                    let started = Instant::now();
                    let result = provider
                        .extract_elements(&image.bytes, image.image_format)
                        .await;
                    let latency_ms = elapsed_ms(started);
                    let summary = summarize_windows_native_ocr_sample(
                        WindowsNativeOcrSampleInput {
                            sample_label: case.label,
                            image_width: image.width,
                            image_height: image.height,
                            image_format: image.image_format,
                            display_scale,
                            latency_ms,
                        },
                        match &result {
                            Ok(results) => Ok(results.as_slice()),
                            Err(error) => Err(error),
                        },
                        PiiFilterLevel::Strict,
                    );
                    if summary.ok {
                        successful_observations += 1;
                    } else {
                        sample_set_ok = false;
                    }

                    observations.push(serde_json::json!({
                        "sample_index": sample_index,
                        "reference_kind": case.reference_kind,
                        "image_source": "controlled_local_synthetic_crop",
                        "summary": summary,
                    }));
                }

                if sample_set_ok {
                    successful_sample_sets += 1;
                }
            }

            serde_json::json!({
                "debug_ax_tree": true,
                "command": "windows-ocr-benchmark",
                "ok": successful_observations > 0,
                "platform": "windows",
                "provider": provider.provider_name(),
                "provider_external": provider.is_external(),
                "samples_requested": samples,
                "sample_sets_ok": successful_sample_sets,
                "observations_requested": samples as usize * cases.len(),
                "observations_ok": successful_observations,
                "display_scale": display_scale,
                "display_scale_x100": display_scale_x100,
                "image_format": "png",
                "language_assumption": "Windows user profile OCR language packs",
                "privacy_contract": "local_winrt_no_external_ocr_strict_sanitized_summary_only",
                "screenshot_policy": "no_broad_desktop_screenshot_default",
                "samples": observations,
            })
        });

        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
            "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                .to_string()
        });
        emit_debug_ax_tree_cli_json(&payload)
    }

    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("debug-ax-tree windows-ocr-benchmark is only supported on Windows");
        let _ = (samples, display_scale_x100);
        2
    }
}

#[cfg(debug_assertions)]
fn run_debug_windows_gui_session_e2e_cli_command(
    display_scale_x100: u32,
    overlay_hold_ms: u64,
) -> i32 {
    #[cfg(target_os = "windows")]
    {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let payload = serde_json::json!({
                    "debug_ax_tree": true,
                    "command": "windows-gui-session-e2e",
                    "ok": false,
                    "platform": "windows",
                    "error_code": "internal.generic",
                    "error_message": format!("tokio runtime init failed: {error}"),
                });
                let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                    "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                        .to_string()
                });
                return emit_debug_ax_tree_cli_json(&payload);
            }
        };

        let payload = runtime.block_on(async {
            crate::windows_gui_session_benchmark::run_windows_gui_session_e2e_benchmark(
                crate::windows_gui_session_benchmark::WindowsGuiSessionBenchmarkConfig {
                    display_scale_x100,
                    overlay_hold_ms,
                },
            )
            .await
        });
        let ok = payload
            .get("ok")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
            "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                .to_string()
        });
        let exit_code = emit_debug_ax_tree_cli_json(&payload);
        if exit_code != 0 {
            return exit_code;
        }
        if ok {
            0
        } else {
            1
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("debug-ax-tree windows-gui-session-e2e is only supported on Windows");
        let _ = (display_scale_x100, overlay_hold_ms);
        2
    }
}

#[cfg(debug_assertions)]
fn run_debug_windows_sandbox_overhead_cli_command(samples: u32, timeout_probe_ms: u64) -> i32 {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let payload = serde_json::json!({
                "debug_ax_tree": true,
                "command": "windows-sandbox-overhead",
                "ok": false,
                "samples_requested": samples,
                "timeout_probe_ms": timeout_probe_ms,
                "failure_class": "runtime_init_failed",
                "error_message": error.to_string(),
            });
            let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
                "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
                    .to_string()
            });
            return emit_debug_ax_tree_cli_json(&payload);
        }
    };

    let payload = runtime.block_on(async {
        super::windows_sandbox_overhead::run_windows_sandbox_overhead_benchmark(
            super::windows_sandbox_overhead::WindowsSandboxOverheadBenchmarkConfig {
                samples,
                timeout_probe_ms,
            },
        )
        .await
    });
    let ok = payload
        .get("ok")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"debug_ax_tree\":true,\"ok\":false,\"error_message\":\"json serialization failed\"}"
            .to_string()
    });
    let exit_code = emit_debug_ax_tree_cli_json(&payload);
    if exit_code != 0 {
        return exit_code;
    }
    if ok {
        0
    } else {
        1
    }
}

#[cfg(all(debug_assertions, target_os = "windows"))]
#[derive(Debug, Clone, Copy)]
struct WindowsOcrBenchmarkCase {
    label: &'static str,
    reference_kind: &'static str,
    text: &'static str,
    width: u32,
    height: u32,
}

#[cfg(all(debug_assertions, target_os = "windows"))]
struct WindowsOcrBenchmarkImage {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    image_format: &'static str,
}

#[cfg(all(debug_assertions, target_os = "windows"))]
fn windows_ocr_benchmark_cases() -> Vec<WindowsOcrBenchmarkCase> {
    vec![
        WindowsOcrBenchmarkCase {
            label: "maekon-settings-crop",
            reference_kind: "maekon_ui_text",
            text: "Maekon Settings Save Draft",
            width: 1200,
            height: 360,
        },
        WindowsOcrBenchmarkCase {
            label: "external-reference-app-crop",
            reference_kind: "external_reference_app_text",
            text: "Notepad Reference External App",
            width: 1200,
            height: 360,
        },
    ]
}

#[cfg(all(debug_assertions, target_os = "windows"))]
fn generate_windows_ocr_benchmark_image(
    case: &WindowsOcrBenchmarkCase,
) -> Result<WindowsOcrBenchmarkImage, String> {
    let nonce = chrono::Utc::now().timestamp_millis();
    let output_path = std::env::temp_dir().join(format!(
        "maekon-windows-ocr-{}-{}-{}.png",
        std::process::id(),
        nonce,
        case.label
    ));
    let script_path = std::env::temp_dir().join(format!(
        "maekon-windows-ocr-{}-{}-{}.ps1",
        std::process::id(),
        nonce,
        case.label
    ));

    let script = r#"
param(
    [string]$path,
    [string]$text,
    [int]$width,
    [int]$height
)
Add-Type -AssemblyName System.Drawing
$bitmap = New-Object System.Drawing.Bitmap($width, $height)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.Clear([System.Drawing.Color]::White)
$graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::ClearTypeGridFit
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
$font = New-Object System.Drawing.Font('Segoe UI', 48, [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
$brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::Black)
$graphics.DrawString($text, $font, $brush, 56, 118)
$bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$brush.Dispose()
$font.Dispose()
$graphics.Dispose()
$bitmap.Dispose()
"#;

    std::fs::write(&script_path, script)
        .map_err(|error| format!("PowerShell script write failed: {error}"))?;

    let output = match std::process::Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&script_path)
        .arg(&output_path)
        .arg(case.text)
        .arg(case.width.to_string())
        .arg(case.height.to_string())
        .output()
    {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_file(&script_path);
            return Err(format!("PowerShell launch failed: {error}"));
        }
    };
    let _ = std::fs::remove_file(&script_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = std::fs::remove_file(&output_path);
        return Err(format!("PowerShell image writer failed: {stderr}"));
    }

    let bytes = match std::fs::read(&output_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            let _ = std::fs::remove_file(&output_path);
            return Err(format!(
                "image read failed at {}: {error}",
                output_path.display()
            ));
        }
    };
    let _ = std::fs::remove_file(&output_path);

    Ok(WindowsOcrBenchmarkImage {
        bytes,
        width: case.width,
        height: case.height,
        image_format: "png",
    })
}

#[cfg(all(debug_assertions, target_os = "windows"))]
fn elapsed_ms(started: std::time::Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(all(debug_assertions, target_os = "windows"))]
fn debug_core_error_payload(error: &maekon_core::error::CoreError) -> serde_json::Value {
    serde_json::json!({
        "code": error.code(),
        "message": error.to_string(),
    })
}

#[cfg(all(debug_assertions, target_os = "macos"))]
pub(crate) fn emit_debug_permissions_open_settings_json(permission_kind: &str) -> i32 {
    let payload = match crate::desktop_permissions::open_desktop_permission_settings(
        permission_kind,
    ) {
        Ok(()) => format!(
            "{{\"debug_permissions\":true,\"command\":\"open-settings\",\"permission_kind\":\"{}\",\"ok\":true}}",
            permission_kind
        ),
        Err(error) => {
            let escaped_error = serde_json::to_string(&error)
                .unwrap_or_else(|_| "\"open settings error\"".to_string());
            format!(
                "{{\"debug_permissions\":true,\"command\":\"open-settings\",\"permission_kind\":\"{}\",\"ok\":false,\"error\":{}}}",
                permission_kind, escaped_error
            )
        }
    };

    emit_debug_permissions_cli_json(&payload)
}

// SAFETY: these declarations match the CoreGraphics C ABI — both
// CGPreflight/CGRequestScreenCaptureAccess take no arguments and return a C
// `bool` — so calls through them are sound.
#[cfg(all(debug_assertions, target_os = "macos"))]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CGPreflightScreenCaptureAccess() -> bool;
    pub(crate) fn CGRequestScreenCaptureAccess() -> bool;
}
