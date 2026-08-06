//! Backlog category contract tests — surface checks for source files declared
//! in tc-catalog.jsonl entries under the integration-adapters / ai-providers /
//! automation-control / coaching-engine / vector-rag /
//! auto-update / sandbox-worker / audio-stt / grpc-dashboard /
//! network-resilience / bug-telemetry suites.
//!
//! Each `#[test]` asserts the source file/dir for one TC exists in the workspace.
//! These are the lightest possible smoke tests — they don't run the underlying
//! logic, but they prevent the source from disappearing without a catalog
//! update.
//!
//! Run via:
//!   cargo test -p maekon-app --test backlog_contract

use std::path::PathBuf;

/// Returns the maekon-client workspace root (parent of src-tauri/).
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent (maekon-client workspace)")
        .to_path_buf()
}

fn assert_exists(rel_path: &str) {
    let p = workspace_root().join(rel_path);
    assert!(
        p.exists(),
        "Expected source path to exist: {} (resolved: {})",
        rel_path,
        p.display()
    );
}

// ───────────────── integration-adapters ─────────────────
// CRT-PRV-INT-001..010: crates/maekon-integration/src/ (ADR-034 P3 move)

#[test]
fn crt_prv_int_001_oidc_device_flow() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_002_http_transport() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_003_cloudevents() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_004_egress_coordinator() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_005_inbox_coordinator() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_006_live_channel() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_007_policy_egress() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_008_producer_loop() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_009_static_auth() {
    assert_exists("crates/maekon-integration/src");
}
#[test]
fn crt_prv_int_010_permission_audit() {
    assert_exists("crates/maekon-integration/src");
}

// ───────────────── ai-providers ─────────────────
// CRT-PRV-AI-001..010: ai_llm_client + ai_ocr_client + ai_model_lifecycle_policy

#[test]
fn crt_prv_ai_001_openai_llm() {
    assert_exists("crates/maekon-network/src/ai_llm_client");
}
#[test]
fn crt_prv_ai_002_anthropic_llm() {
    assert_exists("crates/maekon-network/src/ai_llm_client");
}
#[test]
fn crt_prv_ai_003_google_gemini_llm() {
    assert_exists("crates/maekon-network/src/ai_llm_client");
}
#[test]
fn crt_prv_ai_004_ollama_llm() {
    assert_exists("crates/maekon-network/src/ai_llm_client");
}
#[test]
fn crt_prv_ai_005_aws_bedrock() {
    assert_exists("crates/maekon-network/src/ai_llm_client");
}
#[test]
fn crt_prv_ai_006_openai_ocr() {
    assert_exists("crates/maekon-network/src/ai_ocr_client");
}
#[test]
fn crt_prv_ai_007_anthropic_ocr() {
    assert_exists("crates/maekon-network/src/ai_ocr_client");
}
#[test]
fn crt_prv_ai_008_google_ocr() {
    assert_exists("crates/maekon-network/src/ai_ocr_client");
}
#[test]
fn crt_prv_ai_009_tesseract_ocr() {
    assert_exists("crates/maekon-vision/src/ocr.rs");
}
#[test]
fn crt_prv_ai_010_model_lifecycle_policy() {
    assert_exists("crates/maekon-core/src/ai_model_lifecycle_policy");
}

// ───────────────── automation-control (10 cargo-testable; OVL stays computer-use) ─────────────────
// CRT-PRV-AUTO-001..010: crates/maekon-automation/src/

#[test]
fn crt_prv_auto_001_policy_signature() {
    assert_exists("crates/maekon-automation/src/policy");
}
#[test]
fn crt_prv_auto_002_intent_planner() {
    assert_exists("crates/maekon-automation/src/intent_planner.rs");
}
#[test]
fn crt_prv_auto_003_action_validator() {
    assert_exists("crates/maekon-automation/src/action_dispatcher.rs");
}
#[test]
fn crt_prv_auto_004_action_executor() {
    assert_exists("crates/maekon-automation/src/action_dispatcher.rs");
}
#[test]
fn crt_prv_auto_005_audit_logger() {
    assert_exists("crates/maekon-automation/src/audit");
}
#[test]
fn crt_prv_auto_006_input_driver() {
    // Split into an ADR-003 directory module (#8691) — assert the directory, same as audit (AUTO-005)
    assert_exists("crates/maekon-automation/src/input_driver");
}
#[test]
fn crt_prv_auto_007_overlay_driver() {
    assert_exists("crates/maekon-automation/src/overlay.rs");
}
#[test]
fn crt_prv_auto_008_element_finder() {
    assert_exists("crates/maekon-vision/src/element_finder.rs");
}
#[test]
fn crt_prv_auto_009_preset_storage() {
    assert_exists("crates/maekon-automation/src/presets.rs");
}
#[test]
fn crt_prv_auto_010_override_store() {
    // Override store lives in maekon-storage
    let p = workspace_root().join("crates/maekon-storage/src/sqlite/override_store_impl.rs");
    assert!(p.exists(), "Override store impl missing: {}", p.display());
}

// ───────────────── runtime-launch refactor ─────────────────

#[test]
fn crt_prv_launch_adr013_002_capture_runtime_factory_present() {
    let root = workspace_root();
    let launch_dir = root.join("src-tauri/src/app_runtime_launch");
    let mod_src = std::fs::read_to_string(launch_dir.join("mod.rs"))
        .expect("app_runtime_launch/mod.rs must be readable");
    let mod_line_count = mod_src.lines().count();
    assert!(
        mod_line_count <= 500,
        "app_runtime_launch/mod.rs should remain a small composition root; got {mod_line_count} lines"
    );

    let capture_src = std::fs::read_to_string(launch_dir.join("capture_wiring.rs"))
        .expect("app_runtime_launch/capture_wiring.rs must exist");
    for needle in [
        "CaptureLaunchWiring",
        "build_capture_wiring",
        "shared_capture_services",
        "consent_manager",
        "capture_paused",
    ] {
        assert!(
            capture_src.contains(needle),
            "capture_wiring.rs should contain `{needle}`"
        );
    }
}

#[test]
fn crt_prv_config_002_config_consent_core_placement_adr() {
    let root = workspace_root();
    let adr = std::fs::read_to_string(
        root.join("docs/architecture/ADR-021-config-consent-core-placement.md"),
    )
    .expect("ADR-021 must be readable");
    let adr_ko = std::fs::read_to_string(
        root.join("docs/architecture/ADR-021-config-consent-core-placement.ko.md"),
    )
    .expect("ADR-021 Korean companion must be readable");

    for needle in [
        "Keep ConfigManager and ConsentManager in maekon-core",
        "must not perform provider calls, network egress, native automation, screen capture, notification delivery, or OS permission mutation",
        "If configuration or consent state gains a second production backend",
    ] {
        assert!(adr.contains(needle), "ADR-021 should lock `{needle}`");
    }

    assert!(
        adr_ko.contains("ConfigManager 와 ConsentManager 는 maekon-core 에 유지한다"),
        "ADR-021 Korean companion should mirror the placement decision"
    );
    assert_exists("crates/maekon-core/src/config_manager/mod.rs");
    assert_exists("crates/maekon-core/src/consent.rs");
}

#[test]
fn crt_prv_id_adr055_web_and_analysis_use_generate_id() {
    let root = workspace_root();
    let scanned_roots = [
        root.join("crates/maekon-web/src"),
        root.join("crates/maekon-analysis/src"),
    ];
    let mut violations = Vec::new();

    fn scan_rs_files(path: &std::path::Path, violations: &mut Vec<String>) {
        for entry in std::fs::read_dir(path).unwrap_or_else(|e| {
            panic!(
                "Failed to scan {} for ADR-055 UUID violations: {e}",
                path.display()
            )
        }) {
            let entry = entry.expect("directory entry should be readable");
            let path = entry.path();
            if path.is_dir() {
                scan_rs_files(&path, violations);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()));
            if source.contains("Uuid::new_v4(") || source.contains("uuid::Uuid::new_v4(") {
                violations.push(path.display().to_string());
            }
        }
    }

    for root in scanned_roots {
        scan_rs_files(&root, &mut violations);
    }

    assert!(
        violations.is_empty(),
        "ADR-055 requires maekon-web/maekon-analysis IDs to use maekon_core::id_generation::generate_id(prefix), not raw UUID v4: {violations:#?}"
    );
}

// ───────────────── coaching-engine ─────────────────
// CRT-PRV-CCH-001..005: coaching_engine + coaching_template + coaching_storage

#[test]
fn crt_prv_cch_001_trigger_evaluation() {
    assert_exists("crates/maekon-analysis/src/coaching_engine");
}
#[test]
fn crt_prv_cch_002_template_rendering() {
    assert_exists("crates/maekon-analysis/src/coaching_template");
}
#[test]
fn crt_prv_cch_003_guards() {
    assert_exists("crates/maekon-analysis/src/coaching_engine");
}
#[test]
fn crt_prv_cch_004_feedback_fsm() {
    assert_exists("crates/maekon-analysis/src/coaching_engine");
}
#[test]
fn crt_prv_cch_005_storage_retention() {
    assert_exists("crates/maekon-storage/src/sqlite/coaching_storage.rs");
}

// ───────────────── vector-rag ─────────────────
// CRT-PRV-VEC-001..005: maekon-embedding + vector_store_impl

#[test]
fn crt_prv_vec_001_int8_quantization() {
    assert_exists("crates/maekon-core/src/quantization.rs");
}
#[test]
fn crt_prv_vec_002_ivf_index() {
    assert_exists("crates/maekon-core/src/ivf_index.rs");
}
#[test]
fn crt_prv_vec_003_similarity_topk() {
    assert_exists("crates/maekon-embedding");
}
#[test]
fn crt_prv_vec_004_vector_store_crud() {
    assert_exists("crates/maekon-storage/src/sqlite/vector_store_impl");
}
#[test]
fn crt_prv_vec_005_rag_pipeline() {
    assert_exists("crates/maekon-analysis/src/vector_retriever.rs");
}

// ───────────────── auto-update ─────────────────
// CRT-PRV-UPD-001..003: src-tauri/src/updater/ + update_*.rs

#[test]
fn crt_prv_upd_001_check_stream() {
    assert_exists("src-tauri/src/updater");
}
#[test]
fn crt_prv_upd_002_download_verify_install() {
    assert_exists("src-tauri/src/updater");
}
#[test]
fn crt_prv_upd_003_rollback_on_signature_mismatch() {
    assert_exists("src-tauri/src/updater");
}

// ───────────────── sandbox-worker ─────────────────
// CRT-PRV-SBX-001..005: maekon-sandbox-worker

#[test]
fn crt_prv_sbx_001_stdin_stdout_contract() {
    assert_exists("crates/maekon-sandbox-worker");
}
#[test]
fn crt_prv_sbx_002_process_isolation() {
    assert_exists("crates/maekon-sandbox-worker");
}
#[test]
fn crt_prv_sbx_003_crash_recovery() {
    assert_exists("crates/maekon-sandbox-worker");
}
#[test]
fn crt_prv_sbx_004_timeout_enforcement() {
    assert_exists("crates/maekon-sandbox-worker");
}
#[test]
fn crt_prv_sbx_005_output_size_limit() {
    assert_exists("crates/maekon-sandbox-worker");
}

// ───────────────── audio-stt ─────────────────
// CRT-PRV-AUD-001..003: maekon-audio + fallback_stt

#[test]
fn crt_prv_aud_001_cpal_capture() {
    assert_exists("crates/maekon-audio/src/capture.rs");
}
#[test]
fn crt_prv_aud_002_whisper_stt() {
    assert_exists("crates/maekon-audio/src/whisper.rs");
}
#[test]
fn crt_prv_aud_003_fallback_stt() {
    // Fallback STT lives in maekon-audio (cloud_stt) per current layout
    let p1 = workspace_root().join("crates/maekon-audio/src/cloud_stt.rs");
    let p2 = workspace_root().join("src-tauri/src/fallback_stt.rs");
    assert!(
        p1.exists() || p2.exists(),
        "Expected fallback STT module at maekon-audio/src/cloud_stt.rs or src-tauri/src/fallback_stt.rs"
    );
}

// ───────────────── grpc-dashboard ─────────────────
// CRT-PRV-GRPC-001..005: maekon-web/src/grpc/

#[test]
fn crt_prv_grpc_001_mtls_handshake() {
    assert_exists("crates/maekon-web/src/grpc");
}
#[test]
fn crt_prv_grpc_002_jwt_auth_gate() {
    assert_exists("crates/maekon-web/src/grpc");
}
#[test]
fn crt_prv_grpc_003_streaming() {
    assert_exists("crates/maekon-web/src/grpc");
}
#[test]
fn crt_prv_grpc_004_cert_hot_reload() {
    assert_exists("crates/maekon-web/src/grpc");
}
#[test]
fn crt_prv_grpc_005_policy_loading() {
    assert_exists("crates/maekon-web/src/grpc");
}

// ───────────────── network-resilience ─────────────────
// CRT-PRV-NET-001..005: maekon-network/src/{resilience, circuit_breaker, connectivity, compression, sse_client}.rs

#[test]
fn crt_prv_net_001_circuit_breaker() {
    assert_exists("crates/maekon-http-core/src/circuit_breaker.rs");
}
#[test]
fn crt_prv_net_002_jittered_backoff() {
    assert_exists("crates/maekon-http-core/src/resilience.rs");
}
#[test]
fn crt_prv_net_003_sse_reconnect() {
    assert_exists("crates/maekon-network/src/sse_client.rs");
}
#[test]
fn crt_prv_net_004_adaptive_compression() {
    assert_exists("crates/maekon-network/src/compression.rs");
}
#[test]
fn crt_prv_net_005_connectivity_detection() {
    assert_exists("crates/maekon-network/src/connectivity.rs");
}

// ───────────────── bug-telemetry ─────────────────
// CRT-PRV-TEL-001..003

#[test]
fn crt_prv_tel_001_bug_report_pii_scrub() {
    assert_exists("src-tauri/src/commands/bug_report.rs");
}
#[test]
fn crt_prv_tel_002_error_report_panic_capture() {
    assert_exists("src-tauri/src/commands/error_report.rs");
}
#[test]
fn crt_prv_tel_003_telemetry_opt_in_out() {
    // Telemetry consent lives in core consent + commands/bug_report
    let p = workspace_root().join("crates/maekon-core/src/consent.rs");
    assert!(
        p.exists(),
        "Telemetry consent module missing: {}",
        p.display()
    );
}
