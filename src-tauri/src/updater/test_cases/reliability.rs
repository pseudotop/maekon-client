use super::*;

#[test]
fn release_reliability_validate_download_url_allows_localhost_in_tests() {
    let updater = Updater::new(test_config());
    // validate_download_url returns Result<()> — Ok(()) is the full contract for an
    // allowed URL; there is no inner value to pin beyond the absence of an error.
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

    let config = test_config();
    let client = reqwest::Client::builder().build().unwrap();
    let updater = Updater::with_client(config, client);
    let download_url = format!("{}/{}", server.url(), asset_name);

    let downloaded_path = updater.download_update(&download_url).await.unwrap();
    let downloaded_bytes = std::fs::read(&downloaded_path).unwrap();
    assert_eq!(downloaded_bytes, payload);

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

    let config = test_config();
    let client = reqwest::Client::builder().build().unwrap();
    let updater = Updater::with_client(config, client);
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

    assert!(matches!(
        result,
        Err(UpdateError::Install(msg)) if msg.contains("Rollback completed after restart failure")
    ));
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

// -------------------------------------------------------------------
// Task 6 D11: install_pending writer + orphan-backup cleanup
// -------------------------------------------------------------------

#[test]
fn install_pending_written_after_successful_replace() {
    let updater = Updater::new(test_config());
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let downloaded = dir.path().join("maekon-new");
    std::fs::write(&current_exe, b"current-binary").unwrap();
    std::fs::write(&downloaded, b"new-binary").unwrap();

    // Pass a synthetic new_version; replace_binary succeeds; restart_app
    // returns Ok (so the happy path completes before we inspect state).
    let result = updater.install_and_restart_with_ops(
        &downloaded,
        &current_exe,
        Some("0.4.40-rc.1"),
        |_candidate| Ok(()),
        || Ok(()),
    );
    result.expect("install_and_restart_with_ops must succeed when replace_binary and restart_app both return Ok");

    // Probe should find the pending marker.
    let pending_path = dir.path().join(".install_pending_0.4.40-rc.1");
    assert!(
        pending_path.exists(),
        ".install_pending_{{new_version}} should be written in install_dir"
    );

    let bytes = std::fs::read(&pending_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
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

    // replace_binary fails on the first call (before pending is written).
    let result = updater.install_and_restart_with_ops(
        &downloaded,
        &current_exe,
        Some("0.4.40-rc.1"),
        |_candidate| Err(UpdateError::Install("replace failed".to_string())),
        || Ok(()),
    );
    assert!(matches!(result, Err(UpdateError::Install(_))));

    // The orphan `{binary}.rollback.{ts}` backup must be cleaned up.
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

// -------------------------------------------------------------------
// Task 8: Auto-Update Verification Tests
// -------------------------------------------------------------------

#[test]
fn sha256_verification_correct_file() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("artifact.bin");
    let content = b"maekon release artifact payload v42";
    std::fs::write(&file_path, content).unwrap();

    let file_bytes = std::fs::read(&file_path).unwrap();
    let computed_hash = Updater::sha256_hex(&file_bytes);

    // Verify the hash is a valid 64-char hex string
    assert_eq!(computed_hash.len(), 64);
    assert!(computed_hash.chars().all(|c| c.is_ascii_hexdigit()));

    // Computing again should yield the same hash (deterministic)
    let hash_again = Updater::sha256_hex(&file_bytes);
    assert_eq!(computed_hash, hash_again);
}

#[test]
fn sha256_verification_detects_corruption() {
    let original = b"genuine release artifact";
    let corrupted = b"corrupted release artifact";

    let hash_original = Updater::sha256_hex(original);
    let hash_corrupted = Updater::sha256_hex(corrupted);

    assert_ne!(
        hash_original, hash_corrupted,
        "different content must produce different hashes"
    );
}

#[test]
fn safe_archive_path_rejects_traversal() {
    use std::path::Path;

    // Paths with parent traversal must be rejected
    assert!(
        !Updater::is_safe_archive_path(Path::new("../../../etc/passwd")),
        "parent traversal should be rejected"
    );
    assert!(
        !Updater::is_safe_archive_path(Path::new("foo/../../bar")),
        "embedded traversal should be rejected"
    );
    assert!(
        !Updater::is_safe_archive_path(Path::new("../outside")),
        "single-level traversal should be rejected"
    );

    // Safe paths must be accepted
    assert!(
        Updater::is_safe_archive_path(Path::new("bin/maekon")),
        "normal nested path should be accepted"
    );
    assert!(
        Updater::is_safe_archive_path(Path::new("maekon")),
        "root-level file should be accepted"
    );
    assert!(
        Updater::is_safe_archive_path(Path::new("./maekon")),
        "current-dir prefixed path should be accepted"
    );
    assert!(
        Updater::is_safe_archive_path(Path::new("release/bin/maekon")),
        "deep nested path should be accepted"
    );
}

#[test]
fn archive_extract_dir_falls_back_to_system_temp_dir_without_parent() {
    use std::path::Path;

    assert_eq!(
        Updater::archive_extract_dir_for_path(Path::new("maekon.zip")),
        std::env::temp_dir()
    );
}

#[test]
fn url_allowlist_accepts_github_rejects_unknown() {
    // github.com and its subdomains are allowed
    assert!(Updater::is_allowed_download_host("github.com"));
    assert!(Updater::is_allowed_download_host("api.github.com"));
    assert!(Updater::is_allowed_download_host(
        "objects.githubusercontent.com"
    ));
    assert!(Updater::is_allowed_download_host("githubusercontent.com"));

    // Unknown hosts must be rejected
    assert!(!Updater::is_allowed_download_host("evil.com"));
    assert!(!Updater::is_allowed_download_host("not-github.com"));
    assert!(!Updater::is_allowed_download_host("github.com.evil.net"));
    assert!(!Updater::is_allowed_download_host("malicious.example.org"));
}

#[test]
fn url_allowlist_full_url_validation() {
    let updater = Updater::new(test_config());

    // GitHub HTTPS URLs are accepted — validate_download_url returns Result<()>;
    // the unit return means Ok(()) is the entire success contract. (#5594: ok-only justified)
    updater
        .validate_download_url(
            "https://github.com/pseudotop/maekon-client/releases/download/v1.0.0/asset.tar.gz",
        )
        .expect("github.com HTTPS release URL must pass the download URL allowlist");
    updater
        .validate_download_url(
            "https://objects.githubusercontent.com/github-releases/asset.tar.gz",
        )
        .expect("objects.githubusercontent.com HTTPS URL must pass the download URL allowlist");

    // Evil domains are rejected
    assert!(matches!(
        updater.validate_download_url("https://evil.com/malware.tar.gz").unwrap_err(),
        UpdateError::Download(_)
    ));
    assert!(matches!(
        updater.validate_download_url("https://not-github.com/fake.tar.gz").unwrap_err(),
        UpdateError::Download(_)
    ));

    // HTTP (non-HTTPS) is rejected for non-localhost
    assert!(matches!(
        updater.validate_download_url("http://github.com/asset.tar.gz").unwrap_err(),
        UpdateError::Download(_)
    ));
}

// -------------------------------------------------------------------
