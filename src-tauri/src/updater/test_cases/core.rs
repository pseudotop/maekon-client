use super::*;

#[test]
fn current_version_is_valid_semver() {
    let version = semver::Version::parse(CURRENT_VERSION)
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
    assert!(!patterns.is_empty(), "at least one pattern must be returned for the current platform");
}

#[test]
fn find_platform_asset_no_assets() {
    let config = test_config();
    let updater = Updater::new(config);

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
    let config = test_config();
    let updater = Updater::new(config);

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
        assert_eq!(size, 1000u64, "found asset size must equal the one placed in the release");
    }
}

fn current_primary_asset_name() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "maekon-macos-arm64.tar.gz"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "maekon-macos-x64.tar.gz"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "maekon-windows-x64.zip"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "maekon-windows-arm64.zip"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "maekon-linux-x64.tar.gz"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "maekon-linux-arm64.tar.gz"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    {
        "maekon-unknown.tar.gz"
    }
}

#[test]
fn find_platform_asset_ignores_sidecars_before_archive() {
    let updater = Updater::new(test_config());
    let asset_name = current_primary_asset_name();
    let release = ReleaseInfo {
        tag_name: "v0.2.0".to_string(),
        name: Some("Sidecar release".to_string()),
        body: None,
        prerelease: false,
        assets: vec![
            ReleaseAsset {
                name: format!("{asset_name}.sha256"),
                browser_download_url: "https://example.com/checksum".to_string(),
                size: 64,
                content_type: "text/plain".to_string(),
            },
            ReleaseAsset {
                name: format!("{asset_name}.sig"),
                browser_download_url: "https://example.com/signature".to_string(),
                size: 96,
                content_type: "application/octet-stream".to_string(),
            },
            ReleaseAsset {
                name: asset_name.to_string(),
                browser_download_url: "https://example.com/archive".to_string(),
                size: 1000,
                content_type: "application/octet-stream".to_string(),
            },
        ],
        html_url: "https://github.com/test/test".to_string(),
        published_at: None,
    };

    let (url, _) = updater
        .find_platform_asset(&release)
        .expect("primary archive must be selected");

    assert_eq!(url, "https://example.com/archive");
}

#[test]
fn find_platform_asset_rejects_loose_substring_match() {
    let updater = Updater::new(test_config());
    let asset_name = current_primary_asset_name();
    let release = ReleaseInfo {
        tag_name: "v0.2.0".to_string(),
        name: Some("Substring release".to_string()),
        body: None,
        prerelease: false,
        assets: vec![
            ReleaseAsset {
                name: format!("notes-{asset_name}"),
                browser_download_url: "https://example.com/notes".to_string(),
                size: 100,
                content_type: "text/plain".to_string(),
            },
            ReleaseAsset {
                name: asset_name.to_string(),
                browser_download_url: "https://example.com/archive".to_string(),
                size: 1000,
                content_type: "application/octet-stream".to_string(),
            },
        ],
        html_url: "https://github.com/test/test".to_string(),
        published_at: None,
    };

    let (url, _) = updater
        .find_platform_asset(&release)
        .expect("exact primary archive must be selected");

    assert_eq!(url, "https://example.com/archive");
}

#[cfg(target_os = "macos")]
#[test]
fn find_platform_asset_supports_macos_universal_archive() {
    let updater = Updater::new(test_config());
    let release = ReleaseInfo {
        tag_name: "v0.2.0".to_string(),
        name: Some("macOS universal release".to_string()),
        body: None,
        prerelease: false,
        assets: vec![ReleaseAsset {
            name: "maekon-macos-universal.tar.gz".to_string(),
            browser_download_url: "https://example.com/macos-universal".to_string(),
            size: 2000,
            content_type: "application/gzip".to_string(),
        }],
        html_url: "https://github.com/test/test".to_string(),
        published_at: None,
    };

    let (url, size) = updater
        .find_platform_asset(&release)
        .expect("macOS universal archive must be accepted");

    assert_eq!(url, "https://example.com/macos-universal");
    assert_eq!(size, 2000);
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
            r#"{{
            "tag_name": "v{}",
            "name": "Current Release",
            "body": "No changes",
            "prerelease": false,
            "assets": [],
            "html_url": "https://github.com/test/releases/v0.1.0",
            "published_at": "2024-01-01T00:00:00Z"
        }}"#,
            CURRENT_VERSION
        ))
        .create_async()
        .await;

    let config = test_config();
    let updater = Updater::new(config);

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
            r#"{{
            "tag_name": "v{}",
            "name": "New Release",
            "body": "New features",
            "prerelease": false,
            "assets": [{{
                "name": "{}",
                "browser_download_url": "https://example.com/download/{}",
                "size": 10000,
                "content_type": "application/octet-stream"
            }}],
            "html_url": "https://github.com/test/releases/v99.0.0",
            "published_at": "2024-01-01T00:00:00Z"
        }}"#,
            newer_version, asset_name, asset_name
        ))
        .create_async()
        .await;

    let config = test_config();
    let updater = Updater::new(config);

    let result = updater.check_for_updates_with_base_url(&server.url()).await;

    mock.assert_async().await;

    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
    ))]
    {
        match result {
            Ok(UpdateCheckResult::Available { latest, .. }) => {
                assert_eq!(latest, semver::Version::parse(newer_version).unwrap());
            }
            other => unreachable!("Expected Available, got {:?}", other),
        }
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

    let config = test_config();
    let updater = Updater::new(config);

    let result = updater.check_for_updates_with_base_url(&server.url()).await;

    mock.assert_async().await;

    assert!(matches!(result, Err(UpdateError::ParseResponse(_))));
}

#[tokio::test]
async fn prerelease_filtered_when_disabled() {
    let mut server = mockito::Server::new_async().await;

    // With include_prerelease=false, uses /releases/latest which returns stable only
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases/latest")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
            "tag_name": "v{}",
            "name": "Current Stable",
            "body": "Stable release",
            "prerelease": false,
            "assets": [],
            "html_url": "https://github.com/test/releases/v0.1.0",
            "published_at": "2024-01-01T00:00:00Z"
        }}"#,
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

    // With include_prerelease=true, uses /releases?per_page=1
    let mock = server
        .mock("GET", "/repos/test-owner/test-repo/releases?per_page=1")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"[{{
            "tag_name": "v99.0.0-rc.1",
            "name": "RC Release",
            "body": "Release candidate",
            "prerelease": true,
            "assets": [{{
                "name": "{}",
                "browser_download_url": "https://example.com/download/{}",
                "size": 10000,
                "content_type": "application/octet-stream"
            }}],
            "html_url": "https://github.com/test/releases/v99.0.0-rc.1",
            "published_at": "2024-01-01T00:00:00Z"
        }}]"#,
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
    {
        match result {
            Ok(UpdateCheckResult::Available { latest, .. }) => {
                assert_eq!(latest, semver::Version::parse("99.0.0-rc.1").unwrap());
            }
            other => unreachable!("Expected Available, got {:?}", other),
        }
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
            r#"{{
            "tag_name": "v99.0.0",
            "name": "New Release",
            "body": "New features",
            "prerelease": false,
            "assets": [{{
                "name": "{}",
                "browser_download_url": "https://example.com/download/{}",
                "size": 10000,
                "content_type": "application/octet-stream"
            }}],
            "html_url": "https://github.com/test/releases/v99.0.0",
            "published_at": "2024-01-01T00:00:00Z"
        }}"#,
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

#[test]
fn error_display_messages() {
    let errors = vec![
        UpdateError::Disabled,
        UpdateError::AlreadyLatest,
        UpdateError::NoSuitableAsset,
        UpdateError::UnsupportedPlatform("test".to_string()),
        UpdateError::ParseResponse("test".to_string()),
        UpdateError::Download("test".to_string()),
        UpdateError::Install("test".to_string()),
    ];

    for error in errors {
        let msg = format!("{}", error);
        assert!(!msg.is_empty());
    }
}
