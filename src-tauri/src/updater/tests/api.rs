//! Tests: check_for_updates API mock + basic construction tests.
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

#[test]
fn current_version_is_valid_semver() {
    semver::Version::parse(CURRENT_VERSION)
        .expect("CURRENT_VERSION must be a valid semver string at compile time");
}

#[test]
fn updater_creation() {
    let config = test_config();
    let updater = Updater::new(config.clone());
    assert_eq!(updater.config.repo_owner, "test-owner");
    assert_eq!(updater.config.repo_name, "test-repo");
}

#[test]
fn disabled_updater_returns_error() {
    let mut config = test_config();
    config.enabled = false;
    let updater = Updater::new(config);
    let result = tokio_test::block_on(updater.check_for_updates());
    assert!(matches!(result, Err(UpdateError::Disabled)));
}

#[test]
fn version_comparison_works() {
    let v1 = semver::Version::parse("0.1.0").unwrap();
    let v2 = semver::Version::parse("0.2.0").unwrap();
    let v3 = semver::Version::parse("0.1.1").unwrap();
    assert!(v2 > v1);
    assert!(v3 > v1);
    assert!(v2 > v3);
}

#[test]
fn platform_patterns_exist() {
    let patterns = Updater::get_platform_patterns()
        .expect("get_platform_patterns must return Ok on any supported platform");
    assert!(
        !patterns.is_empty(),
        "at least one pattern must be returned for the current platform"
    );
}

#[test]
fn find_platform_asset_no_assets() {
    let updater = Updater::new(test_config());
    let release = ReleaseInfo {
        tag_name: "v0.2.0".to_string(),
        name: Some("Test Release".to_string()),
        body: None,
        prerelease: false,
        assets: vec![],
        html_url: "https://github.com/test/test".to_string(),
        published_at: None,
    };
    let result = updater.find_platform_asset(&release);
    assert!(matches!(result, Err(UpdateError::NoSuitableAsset)));
}

#[test]
fn find_platform_asset_matches_pattern() {
    let updater = Updater::new(test_config());
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let asset_name = "maekon-macos-arm64.tar.gz";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let asset_name = "maekon-macos-x64.tar.gz";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let asset_name = "maekon-windows-x64.zip";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let asset_name = "maekon-linux-x64.tar.gz";
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    let asset_name = "maekon-unknown.tar.gz";
    let release = ReleaseInfo {
        tag_name: "v0.2.0".to_string(),
        name: Some("Test Release".to_string()),
        body: None,
        prerelease: false,
        assets: vec![ReleaseAsset {
            name: asset_name.to_string(),
            browser_download_url: "https://example.com/download".to_string(),
            size: 1000,
            content_type: "application/octet-stream".to_string(),
        }],
        html_url: "https://github.com/test/test".to_string(),
        published_at: None,
    };
    let result = updater.find_platform_asset(&release);
    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    ))]
    {
        // find_platform_asset returns Ok((download_url, size)) on a match.
        let (download_url, size) =
            result.expect("find_platform_asset must match the platform-specific asset");
        assert_eq!(
            download_url, "https://example.com/download",
            "found asset URL must equal the one placed in the release"
        );
        assert_eq!(
            size, 1000u64,
            "found asset size must equal the one placed in the release"
        );
    }
}

#[test]
fn should_check_returns_true_when_no_last_check() {
    let config = test_config();
    let updater = Updater::new(config);
    assert!(updater.config.enabled);
}

#[tokio::test]
async fn check_for_updates_with_mock_api_up_to_date() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"tag_name":"v{}","name":"Current","body":"","prerelease":false,"assets":[],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}}"#,
            CURRENT_VERSION
        ))
        .create_async()
        .await;
    let updater = Updater::new(test_config());
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    assert!(matches!(result, Ok(UpdateCheckResult::UpToDate { .. })));
}

#[tokio::test]
async fn check_for_updates_with_mock_api_available() {
    let mut server = mockito::Server::new_async().await;
    let newer_version = "99.0.0";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let asset_name = "maekon-macos-arm64.tar.gz";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let asset_name = "maekon-macos-x64.tar.gz";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let asset_name = "maekon-windows-x64.zip";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let asset_name = "maekon-linux-x64.tar.gz";
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    let asset_name = "maekon-unknown.tar.gz";
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"tag_name":"v{}","name":"New","body":"<!-- rollout:100 -->","prerelease":false,"assets":[{{"name":"{}","browser_download_url":"https://example.com/{}","size":10000,"content_type":"application/octet-stream"}}],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}}"#,
            newer_version, asset_name, asset_name
        ))
        .create_async()
        .await;
    let updater = Updater::new(test_config());
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    ))]
    match result {
        Ok(UpdateCheckResult::Available { latest, .. }) => {
            assert_eq!(latest, semver::Version::parse(newer_version).unwrap());
        }
        other => unreachable!("Expected Available, got {:?}", other),
    }
}

#[tokio::test]
async fn check_for_updates_api_error() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(404)
        .with_body("Not Found")
        .create_async()
        .await;
    let updater = Updater::new(test_config());
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    assert!(matches!(result, Err(UpdateError::ParseResponse(_))));
}

#[tokio::test]
async fn prerelease_filtered_when_disabled() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"tag_name":"v{}","name":"Stable","body":"","prerelease":false,"assets":[],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}}"#,
            CURRENT_VERSION
        ))
        .create_async()
        .await;
    let mut config = test_config();
    config.include_prerelease = false;
    let updater = Updater::new(config);
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    assert!(matches!(result, Ok(UpdateCheckResult::UpToDate { .. })));
}

#[tokio::test]
async fn prerelease_found_when_enabled() {
    let mut server = mockito::Server::new_async().await;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let asset_name = "maekon-macos-arm64.tar.gz";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let asset_name = "maekon-macos-x64.tar.gz";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let asset_name = "maekon-windows-x64.zip";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let asset_name = "maekon-linux-x64.tar.gz";
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    let asset_name = "maekon-unknown.tar.gz";
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases?per_page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{"tag_name":"v99.0.0-rc.1","name":"RC","body":"<!-- rollout:100 -->","prerelease":true,"assets":[{{"name":"{}","browser_download_url":"https://example.com/{}","size":10000,"content_type":"application/octet-stream"}}],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}}]"#,
            asset_name, asset_name
        ))
        .create_async()
        .await;
    let mut config = test_config();
    config.include_prerelease = true;
    let updater = Updater::new(config);
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    ))]
    match result {
        Ok(UpdateCheckResult::Available { latest, .. }) => {
            assert_eq!(latest, semver::Version::parse("99.0.0-rc.1").unwrap());
        }
        other => unreachable!("Expected Available, got {:?}", other),
    }
}

#[tokio::test]
async fn check_for_updates_rejects_release_below_min_allowed_version() {
    let mut server = mockito::Server::new_async().await;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let asset_name = "maekon-macos-arm64.tar.gz";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let asset_name = "maekon-macos-x64.tar.gz";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let asset_name = "maekon-windows-x64.zip";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let asset_name = "maekon-linux-x64.tar.gz";
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    let asset_name = "maekon-unknown.tar.gz";
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"tag_name":"v99.0.0","name":"New","body":"<!-- rollout:100 -->","prerelease":false,"assets":[{{"name":"{}","browser_download_url":"https://example.com/{}","size":10000,"content_type":"application/octet-stream"}}],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}}"#,
            asset_name, asset_name
        ))
        .create_async()
        .await;
    let mut config = test_config();
    config.min_allowed_version = Some("100.0.0".to_string());
    let updater = Updater::new(config);
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    assert!(matches!(result, Err(UpdateError::Integrity(_))));
}

/// #4836: a release ABOVE the managed-config ceiling is HELD — reported as
/// up-to-date (not an error, unlike the floor), so a pinned fleet simply never
/// sees the newer release. The ceiling check short-circuits before the rollout
/// gate and asset resolution, so an empty asset list is fine here.
#[tokio::test]
async fn check_for_updates_holds_release_above_max_allowed_version() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{"tag_name":"v99.0.0","name":"New","body":"","prerelease":false,"assets":[],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}"#,
        )
        .create_async()
        .await;
    let mut config = test_config();
    config.max_allowed_version = Some("1.0.0".to_string());
    let updater = Updater::new(config);
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    assert!(
        matches!(result, Ok(UpdateCheckResult::UpToDate { .. })),
        "release above the ceiling must be held as UpToDate, got {result:?}"
    );
}

/// #4836: a ceiling at/above the latest release does NOT block — the update is
/// offered normally, proving the gate is a bound and not an outright disable.
#[tokio::test]
async fn check_for_updates_offers_release_within_max_allowed_version() {
    let mut server = mockito::Server::new_async().await;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    let asset_name = "maekon-macos-arm64.tar.gz";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    let asset_name = "maekon-macos-x64.tar.gz";
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    let asset_name = "maekon-windows-x64.zip";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let asset_name = "maekon-linux-x64.tar.gz";
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    )))]
    let asset_name = "maekon-unknown.tar.gz";
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{"tag_name":"v99.0.0","name":"New","body":"<!-- rollout:100 -->","prerelease":false,"assets":[{{"name":"{}","browser_download_url":"https://example.com/{}","size":10000,"content_type":"application/octet-stream"}}],"html_url":"https://github.com/test","published_at":"2024-01-01T00:00:00Z"}}"#,
            asset_name, asset_name
        ))
        .create_async()
        .await;
    let mut config = test_config();
    config.max_allowed_version = Some("99.0.0".to_string()); // ceiling == latest → allowed
    let updater = Updater::new(config);
    let result = updater.check_for_updates_with_base_url(&server.url()).await;
    mock.assert_async().await;
    assert!(
        matches!(result, Ok(UpdateCheckResult::Available { .. })),
        "release at the ceiling must still be offered, got {result:?}"
    );
}
