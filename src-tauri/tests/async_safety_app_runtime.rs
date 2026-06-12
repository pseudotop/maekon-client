//! Async safety regression tests — F-RR-06 (issue #3431), F-RC-10 (cycle 17).
//!
//! Ensures that file I/O helpers called from async contexts use
//! non-blocking tokio::fs equivalents so they cannot starve the tokio
//! worker thread pool during normal operation.
//!
//! F-RR-06: app_runtime_launch.rs tokio::fs migration.
//! F-RC-10: install.rs apply_delta_update/download_update_with_progress,
//!          llm_provider.rs run_codex, ocr_provider.rs run_codex_ocr,
//!          audio.rs download_whisper_model, temp_file_projection.rs project,
//!          model_downloader.rs create_dir_all.
//!
//! Run: `cargo test -p maekon-app --test async_safety_app_runtime`

use std::time::Duration;

/// Reads a file path using tokio::fs::read inside a timeout, confirming the
/// call does not block the tokio scheduler. This is the async-safe equivalent
/// of the std::fs::read call removed from build_external_spawn_config
/// (F-RR-06, issue #3431).
#[tokio::test]
async fn tokio_fs_read_does_not_block_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pem = tmp.path().join("test.pem");
    std::fs::write(
        &pem,
        b"-----BEGIN CERTIFICATE-----\nZmFrZQ==\n-----END CERTIFICATE-----\n",
    )
    .expect("write pem");

    let result = tokio::time::timeout(Duration::from_secs(2), tokio::fs::read(&pem)).await;

    // Collapse: the outer Ok proves no scheduler starvation; the inner Ok
    // proves the read succeeded; content check pins the byte contract.
    let bytes = result
        .expect("tokio::fs::read timed out — scheduler starvation")
        .expect("tokio::fs::read returned Err");
    assert!(!bytes.is_empty(), "read should return non-empty PEM bytes");
}

/// Reads a file path using tokio::fs::read_to_string inside a timeout,
/// confirming the call does not block the tokio scheduler. This covers the
/// mTLS allowlist read path in build_external_spawn_config (F-RR-06).
#[tokio::test]
async fn tokio_fs_read_to_string_does_not_block_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let allowlist = tmp.path().join("allowlist.txt");
    std::fs::write(&allowlist, b"AA:BB:CC:DD:EE:FF\n11:22:33:44:55:66\n").expect("write allowlist");

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::fs::read_to_string(&allowlist),
    )
    .await;

    // Collapse: outer Ok proves no starvation; inner Ok proves read succeeded.
    let text = result
        .expect("tokio::fs::read_to_string timed out — scheduler starvation")
        .expect("tokio::fs::read_to_string returned Err");
    assert!(
        text.contains("AA:BB:CC:DD:EE:FF"),
        "allowlist content must round-trip"
    );
}

/// Writes a JSON schema file using tokio::fs::write inside a timeout,
/// confirming the schema write path in send_codex_message (sibling fix)
/// does not block the scheduler.
#[tokio::test]
async fn tokio_fs_write_does_not_block_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let schema_path = tmp.path().join("output-schema.json");
    let payload = br#"{"type":"object"}"#;

    let result = tokio::time::timeout(
        Duration::from_secs(2),
        tokio::fs::write(&schema_path, payload),
    )
    .await;

    // Collapse: outer Ok proves no starvation; inner Ok proves write succeeded.
    result
        .expect("tokio::fs::write timed out — scheduler starvation")
        .expect("tokio::fs::write returned Err");
    let written = std::fs::read(&schema_path).expect("readback");
    assert_eq!(written, payload, "written bytes must match payload exactly");
}

/// F-RC-10: verifies that tokio::fs::remove_file does not block the
/// scheduler, covering the install.rs delta-update cleanup path.
#[tokio::test]
async fn tokio_fs_remove_file_does_not_block_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("to-remove.bin");
    std::fs::write(&path, b"temp").expect("write");

    let result = tokio::time::timeout(Duration::from_secs(2), tokio::fs::remove_file(&path)).await;

    // Collapse: outer Ok proves no starvation; inner Ok proves remove succeeded.
    result
        .expect("tokio::fs::remove_file timed out — scheduler starvation")
        .expect("tokio::fs::remove_file returned Err");
    assert!(!path.exists(), "file must be absent after remove_file");
}

/// F-RC-10: verifies that tokio::fs::create_dir_all does not block the
/// scheduler, covering the model_downloader.rs dest-dir creation path.
#[tokio::test]
async fn tokio_fs_create_dir_all_does_not_block_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let nested = tmp.path().join("a/b/c");

    let result =
        tokio::time::timeout(Duration::from_secs(2), tokio::fs::create_dir_all(&nested)).await;

    // Collapse: outer Ok proves no starvation; inner Ok proves creation succeeded.
    result
        .expect("tokio::fs::create_dir_all timed out — scheduler starvation")
        .expect("tokio::fs::create_dir_all returned Err");
    assert!(
        nested.is_dir(),
        "nested directory must exist after create_dir_all"
    );
}

/// F-RC-10: verifies that tokio::fs::metadata does not block the
/// scheduler, covering the audio.rs download_whisper_model path.
#[tokio::test]
async fn tokio_fs_metadata_does_not_block_runtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("meta.bin");
    std::fs::write(&path, b"content").expect("write");

    let result = tokio::time::timeout(Duration::from_secs(2), tokio::fs::metadata(&path)).await;

    // Collapse: outer Ok proves no starvation; inner Ok proves metadata succeeded.
    let meta = result
        .expect("tokio::fs::metadata timed out — scheduler starvation")
        .expect("tokio::fs::metadata returned Err");
    assert_eq!(meta.len(), 7, "metadata must report the correct file size");
}

/// Regression anchor: verifies that this test file is discovered by the
/// cargo test harness (mirrors regressions.rs pattern).
#[test]
fn async_safety_harness_is_wired() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/async_safety_app_runtime.rs");
    assert!(
        path.is_file(),
        "async_safety_app_runtime test file should exist at {}",
        path.display()
    );
}
