//! Tests: download reliability, D11 install_pending, rollout bucketing, E2E platform.
use crate::updater::*;
use maekon_core::config::{UpdateChannel, UpdateConfig};
use tempfile::tempdir;

fn test_config() -> UpdateConfig {
    UpdateConfig {
        enabled: true,
        repo_owner: "test-owner".to_string(),
        repo_name: "test-repo".to_string(),
        check_interval_hours: 24,
        channel: UpdateChannel::default(),
        include_prerelease: false,
        auto_install: false,
        installation_id: Some("test-install-00000000-0000-0000-0000-000000000000".to_string()),
        require_signature_verification: false,
        signature_public_key: String::new(),
        min_allowed_version: None,
        max_allowed_version: None,
    }
}

#[test]
fn release_reliability_validate_download_url_allows_localhost_in_tests() {
    let updater = Updater::new(test_config());
    // validate_download_url returns Result<()>; Ok(()) is the full allowlist-pass contract.
    // (#5594: ok-only justified — unit return type)
    updater
        .validate_download_url("https://localhost/maekon-update.tar.gz")
        .expect("localhost HTTPS must be accepted by the URL allowlist in test mode");
    updater
        .validate_download_url("https://127.0.0.1/maekon-update.tar.gz")
        .expect("127.0.0.1 HTTPS must be accepted by the URL allowlist in test mode");
}

#[tokio::test]
async fn release_reliability_download_update_accepts_localhost_with_integrity() {
    let mut server = mockito::Server::new_async().await;
    let asset_name = "maekon-test-update.tar.gz";
    let payload = b"release-artifact-v1".to_vec();
    let expected_hash = Updater::sha256_hex(&payload);
    let artifact_mock = server
        .mock("GET", format!("/{asset_name}").as_str())
        .with_status(200)
        .with_body(payload.clone())
        .create_async()
        .await;
    let checksum_mock = server
        .mock("GET", format!("/{asset_name}.sha256").as_str())
        .with_status(200)
        .with_body(format!("{expected_hash}  {asset_name}\n"))
        .create_async()
        .await;
    let updater = Updater::with_client(test_config(), reqwest::Client::builder().build().unwrap());
    let download_url = format!("{}/{}", server.url(), asset_name);
    let downloaded_path = updater.download_update(&download_url).await.unwrap();
    assert_eq!(std::fs::read(&downloaded_path).unwrap(), payload);
    std::fs::remove_file(&downloaded_path).unwrap();
    artifact_mock.assert_async().await;
    checksum_mock.assert_async().await;
}

#[tokio::test]
async fn release_reliability_download_update_rejects_checksum_mismatch() {
    let mut server = mockito::Server::new_async().await;
    let asset_name = "maekon-test-update.tar.gz";
    let artifact_mock = server
        .mock("GET", format!("/{asset_name}").as_str())
        .with_status(200)
        .with_body("release-artifact-v1")
        .create_async()
        .await;
    let checksum_mock = server
        .mock("GET", format!("/{asset_name}.sha256").as_str())
        .with_status(200)
        .with_body(format!("{}  {asset_name}\n", "0".repeat(64)))
        .create_async()
        .await;
    let updater = Updater::with_client(test_config(), reqwest::Client::builder().build().unwrap());
    let download_url = format!("{}/{}", server.url(), asset_name);
    let err = updater.download_update(&download_url).await.unwrap_err();
    assert!(matches!(err, UpdateError::Integrity(msg) if msg.contains("Checksum mismatch")));
    artifact_mock.assert_async().await;
    checksum_mock.assert_async().await;
}

#[test]
fn release_reliability_install_and_restart_rolls_back_after_restart_failure() {
    let updater = Updater::new(test_config());
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let downloaded = dir.path().join("maekon-new");
    std::fs::write(&current_exe, b"current-binary").unwrap();
    std::fs::write(&downloaded, b"new-binary").unwrap();
    let mut replaced = Vec::new();
    let result = updater.install_and_restart_with_ops(
        &downloaded,
        &current_exe,
        None,
        |candidate| {
            replaced.push(candidate.to_path_buf());
            Ok(())
        },
        || {
            Err(UpdateError::Install(
                "simulated restart failure".to_string(),
            ))
        },
    );
    assert!(
        matches!(result, Err(UpdateError::Install(msg)) if msg.contains("Rollback completed after restart failure"))
    );
    assert_eq!(replaced.len(), 2);
    assert_eq!(replaced[0], downloaded);
    assert!(replaced[1]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(".rollback."));
}

#[test]
fn release_reliability_install_and_restart_reports_rollback_failure() {
    let updater = Updater::new(test_config());
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let downloaded = dir.path().join("maekon-new");
    std::fs::write(&current_exe, b"current-binary").unwrap();
    std::fs::write(&downloaded, b"new-binary").unwrap();
    let mut replace_calls = 0usize;
    let result = updater.install_and_restart_with_ops(
        &downloaded,
        &current_exe,
        None,
        |_candidate| {
            replace_calls += 1;
            if replace_calls == 1 {
                Ok(())
            } else {
                Err(UpdateError::Install(
                    "simulated rollback replace failure".to_string(),
                ))
            }
        },
        || {
            Err(UpdateError::Install(
                "simulated restart failure".to_string(),
            ))
        },
    );
    match result {
        Err(UpdateError::Install(msg)) => {
            assert!(msg.contains("Restart failed and rollback failed"));
            assert!(msg.contains("simulated restart failure"));
            assert!(msg.contains("simulated rollback replace failure"));
        }
        other => panic!("unexpected result: {:?}", other),
    }
    assert_eq!(replace_calls, 2);
}

#[test]
fn install_pending_written_after_successful_replace() {
    let updater = Updater::new(test_config());
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let downloaded = dir.path().join("maekon-new");
    std::fs::write(&current_exe, b"current-binary").unwrap();
    std::fs::write(&downloaded, b"new-binary").unwrap();
    let result = updater.install_and_restart_with_ops(
        &downloaded,
        &current_exe,
        Some("0.4.40-rc.1"),
        |_candidate| Ok(()),
        || Ok(()),
    );
    result.expect("install_and_restart_with_ops must succeed when replace_binary and restart_app both return Ok");
    let pending_path = dir.path().join(".install_pending_0.4.40-rc.1");
    assert!(
        pending_path.exists(),
        ".install_pending_{{new_version}} should be written"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&pending_path).unwrap()).unwrap();
    assert!(parsed.get("installed_at").is_some());
    assert!(parsed.get("previous_version").is_some());
    assert!(parsed.get("backup_path").is_some());
}

#[test]
fn orphan_backup_removed_on_replace_binary_failure() {
    let updater = Updater::new(test_config());
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let downloaded = dir.path().join("maekon-new");
    std::fs::write(&current_exe, b"current-binary").unwrap();
    std::fs::write(&downloaded, b"new-binary").unwrap();
    let result = updater.install_and_restart_with_ops(
        &downloaded,
        &current_exe,
        Some("0.4.40-rc.1"),
        |_candidate| Err(UpdateError::Install("replace failed".to_string())),
        || Ok(()),
    );
    assert!(matches!(result, Err(UpdateError::Install(_))));
    let rollback_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.contains(".rollback."))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        rollback_files.is_empty(),
        "orphan backup should have been removed on replace failure; found: {:?}",
        rollback_files
    );
}
