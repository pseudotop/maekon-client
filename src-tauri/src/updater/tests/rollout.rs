//! Tests: staged rollout (FNV-1a), E2E platform selection, rollout gate.
use crate::updater::*;
use maekon_core::config::{UpdateChannel, UpdateConfig};

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

/// Requires network access — marked #[ignore] for CI.
#[tokio::test]
#[ignore]
async fn e2e_check_for_updates_reaches_github() {
    let config = UpdateConfig {
        enabled: true,
        repo_owner: "pseudotop".to_string(),
        repo_name: "maekon-client".to_string(),
        channel: UpdateChannel::default(),
        include_prerelease: false,
        ..UpdateConfig::default()
    };
    let updater = Updater::new(config);
    match updater.check_for_updates().await {
        Ok(UpdateCheckResult::Available {
            current, latest, ..
        }) => {
            assert!(latest > current, "Available must have latest > current");
        }
        Ok(UpdateCheckResult::UpToDate { current }) => {
            assert_eq!(current, semver::Version::parse(CURRENT_VERSION).unwrap());
        }
        Err(e) => panic!("check_for_updates failed: {}", e),
    }
}

/// Requires network access — marked #[ignore] for CI.
#[tokio::test]
#[ignore]
async fn e2e_preview_update_availability_reaches_github() {
    let config = UpdateConfig {
        enabled: true,
        repo_owner: "pseudotop".to_string(),
        repo_name: "maekon-client".to_string(),
        channel: UpdateChannel::default(),
        include_prerelease: false,
        ..UpdateConfig::default()
    };
    let updater = Updater::new(config);
    match updater.preview_update_availability().await {
        Ok(preview) => {
            // The version field in a real GitHub response must be parseable as semver.
            semver::Version::parse(&preview.version).unwrap_or_else(|e| {
                panic!(
                    "preview version must be valid semver — got {:?}: {}",
                    preview.version, e
                )
            });
        }
        Err(e) => panic!("preview_update_availability failed: {}", e),
    }
}

#[test]
fn e2e_platform_asset_selection() {
    let patterns = Updater::get_platform_patterns()
        .expect("get_platform_patterns must succeed on supported platforms");
    assert!(
        !patterns.is_empty(),
        "at least one platform pattern must be returned"
    );
    for pattern in &patterns {
        assert_eq!(
            *pattern,
            pattern.to_lowercase(),
            "pattern must be lowercase: {}",
            pattern
        );
    }
    let expected_os: Vec<&str> = match std::env::consts::OS {
        "macos" => vec!["macos", "darwin"],
        "windows" => vec!["windows", "win"],
        "linux" => vec!["linux"],
        other => panic!("unexpected OS: {}", other),
    };
    assert!(
        patterns
            .iter()
            .any(|p| expected_os.iter().any(|tok| p.contains(tok))),
        "platform patterns {:?} must contain an OS token from {:?}",
        patterns,
        expected_os
    );
    let expected_arch: Vec<&str> = match std::env::consts::ARCH {
        "aarch64" => vec!["arm64", "aarch64"],
        "x86_64" => vec!["x64", "x86_64", "amd64"],
        other => panic!("unexpected arch: {}", other),
    };
    assert!(
        patterns
            .iter()
            .any(|p| expected_arch.iter().any(|tok| p.contains(tok))),
        "platform patterns {:?} must contain an arch token from {:?}",
        patterns,
        expected_arch
    );
}

#[test]
fn e2e_platform_asset_selection_multi_platform_release() {
    let updater = Updater::new(test_config());
    let release = ReleaseInfo {
        tag_name: "v99.0.0".to_string(),
        name: Some("Multi-platform release".to_string()),
        body: None,
        prerelease: false,
        assets: vec![
            ReleaseAsset {
                name: "maekon-macos-arm64.tar.gz".to_string(),
                browser_download_url: "https://example.com/macos-arm64".to_string(),
                size: 10_000,
                content_type: "application/gzip".to_string(),
            },
            ReleaseAsset {
                name: "maekon-macos-x64.tar.gz".to_string(),
                browser_download_url: "https://example.com/macos-x64".to_string(),
                size: 10_000,
                content_type: "application/gzip".to_string(),
            },
            ReleaseAsset {
                name: "maekon-windows-x64.zip".to_string(),
                browser_download_url: "https://example.com/windows-x64".to_string(),
                size: 12_000,
                content_type: "application/zip".to_string(),
            },
            ReleaseAsset {
                name: "maekon-linux-x64.tar.gz".to_string(),
                browser_download_url: "https://example.com/linux-x64".to_string(),
                size: 9_000,
                content_type: "application/gzip".to_string(),
            },
            ReleaseAsset {
                name: "maekon-linux-arm64.tar.gz".to_string(),
                browser_download_url: "https://example.com/linux-arm64".to_string(),
                size: 9_000,
                content_type: "application/gzip".to_string(),
            },
        ],
        html_url: "https://github.com/test/releases/v99.0.0".to_string(),
        published_at: None,
    };
    let (url, size) = updater
        .find_platform_asset(&release)
        .expect("must find asset");
    assert!(size > 0);
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => assert_eq!(url, "https://example.com/macos-arm64"),
        ("macos", "x86_64") => assert_eq!(url, "https://example.com/macos-x64"),
        ("windows", "x86_64") => assert_eq!(url, "https://example.com/windows-x64"),
        ("linux", "x86_64") => assert_eq!(url, "https://example.com/linux-x64"),
        ("linux", "aarch64") => assert_eq!(url, "https://example.com/linux-arm64"),
        (os, arch) => panic!("unhandled platform: {}-{}", os, arch),
    }
}

#[test]
fn fnv1a_hash_deterministic() {
    let h1 = fnv1a_hash(b"test-device-v1.0.0");
    let h2 = fnv1a_hash(b"test-device-v1.0.0");
    assert_eq!(h1, h2);
}

#[test]
fn fnv1a_hash_different_inputs() {
    assert_ne!(
        fnv1a_hash(b"device-a-v1.0.0"),
        fnv1a_hash(b"device-b-v1.0.0")
    );
}

#[test]
fn rollout_100_always_eligible() {
    assert!(is_eligible_for_rollout("any-device", "v1.0.0", 100));
}

#[test]
fn rollout_0_never_eligible() {
    assert!(!is_eligible_for_rollout("any-device", "v1.0.0", 0));
}

#[test]
fn rollout_deterministic() {
    assert_eq!(
        is_eligible_for_rollout("device-123", "v2.0.0", 50),
        is_eligible_for_rollout("device-123", "v2.0.0", 50)
    );
}

#[test]
fn parse_rollout_present() {
    assert_eq!(
        parse_rollout_percent(&Some("<!-- rollout:25 -->\n## Changes".to_string())),
        25
    );
}

#[test]
fn parse_rollout_absent() {
    assert_eq!(
        parse_rollout_percent(&Some("## Changes\n- Fix bugs".to_string())),
        100
    );
}

#[test]
fn parse_rollout_none() {
    assert_eq!(parse_rollout_percent(&None), 100);
}

#[test]
fn parse_rollout_caps_at_100() {
    assert_eq!(
        parse_rollout_percent(&Some("<!-- rollout:150 -->".to_string())),
        100
    );
}

#[tokio::test]
async fn update_check_respects_rollout_exclusion() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"tag_name":"v99.0.0","name":"Excluded","body":"<!-- rollout:0 -->","prerelease":false,"assets":[],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}"#)
        .create_async()
        .await;
    let result = Updater::new(test_config())
        .check_for_updates_with_base_url(&server.url())
        .await;
    mock.assert_async().await;
    assert!(
        matches!(result, Ok(UpdateCheckResult::UpToDate { .. })),
        "rollout:0 must exclude every device"
    );
}

#[tokio::test]
async fn update_check_without_installation_id_is_excluded() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"tag_name":"v99.0.0","name":"100%","body":"new features","prerelease":false,"assets":[],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}"#)
        .create_async()
        .await;
    let mut config = test_config();
    config.installation_id = None;
    let result = Updater::new(config)
        .check_for_updates_with_base_url(&server.url())
        .await;
    mock.assert_async().await;
    assert!(
        matches!(result, Ok(UpdateCheckResult::UpToDate { .. })),
        "None installation_id must be treated as rollout-excluded"
    );
}
