use super::*;

// -------------------------------------------------------------------

/// Verify that check_for_updates can reach the real GitHub API and parse a response.
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

    let result = updater.check_for_updates().await;

    // The call must succeed — either a newer version is available or we are up-to-date.
    // Both variants are valid; only an Err would indicate an API or parsing problem.
    match result {
        Ok(UpdateCheckResult::Available {
            current, latest, ..
        }) => {
            assert!(
                latest > current,
                "Available variant must have latest > current"
            );
        }
        Ok(UpdateCheckResult::UpToDate { current }) => {
            assert_eq!(
                current,
                semver::Version::parse(CURRENT_VERSION).unwrap(),
                "UpToDate must report the running version"
            );
        }
        Err(e) => panic!("check_for_updates failed against live GitHub API: {}", e),
    }
}

/// Verify that preview_update_availability can reach GitHub and return a coherent result.
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

    let result = updater.preview_update_availability().await;

    match result {
        Ok(preview) => {
            // The version in the preview payload must be parseable as semver.
            semver::Version::parse(&preview.version).unwrap_or_else(|e| {
                panic!(
                    "preview result version must be valid semver — got {:?}: {}",
                    preview.version, e
                )
            });
        }
        Err(e) => panic!(
            "preview_update_availability failed against live GitHub API: {}",
            e
        ),
    }
}

/// Platform detection: verify the correct asset patterns are returned for the
/// current OS+arch and that they match the expected naming convention.
#[test]
fn e2e_platform_asset_selection() {
    let patterns = Updater::get_platform_patterns()
        .expect("get_platform_patterns must succeed on supported platforms");

    assert!(
        !patterns.is_empty(),
        "at least one platform pattern must be returned"
    );

    // Every pattern must be lowercase (asset matching uses to_lowercase)
    for pattern in &patterns {
        assert_eq!(
            *pattern,
            pattern.to_lowercase(),
            "platform pattern must already be lowercase: {}",
            pattern
        );
    }

    // Verify the patterns contain the expected OS token for this platform
    let os_token = std::env::consts::OS;
    let expected_os = match os_token {
        "macos" => vec!["macos", "darwin"],
        "windows" => vec!["windows", "win"],
        "linux" => vec!["linux"],
        other => panic!("unexpected OS: {}", other),
    };

    let has_os_match = patterns
        .iter()
        .any(|p| expected_os.iter().any(|tok| p.contains(tok)));
    assert!(
        has_os_match,
        "platform patterns {:?} must contain an OS token from {:?}",
        patterns, expected_os
    );

    // Verify the patterns contain an architecture token
    let arch_token = std::env::consts::ARCH;
    let expected_arch = match arch_token {
        "aarch64" => vec!["arm64", "aarch64"],
        "x86_64" => vec!["x64", "x86_64", "amd64"],
        other => panic!("unexpected arch: {}", other),
    };

    let has_arch_match = patterns
        .iter()
        .any(|p| expected_arch.iter().any(|tok| p.contains(tok)));
    assert!(
        has_arch_match,
        "platform patterns {:?} must contain an arch token from {:?}",
        patterns, expected_arch
    );
}

/// Verify that find_platform_asset correctly picks the right asset from a
/// release that contains assets for multiple platforms.
#[test]
fn e2e_platform_asset_selection_multi_platform_release() {
    let config = test_config();
    let updater = Updater::new(config);

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
        .expect("must find an asset for the current platform");

    assert!(size > 0, "asset size must be positive");

    // The selected URL must correspond to the current OS
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("macos", "aarch64") => assert_eq!(url, "https://example.com/macos-arm64"),
        ("macos", "x86_64") => assert_eq!(url, "https://example.com/macos-x64"),
        ("windows", "x86_64") => assert_eq!(url, "https://example.com/windows-x64"),
        ("linux", "x86_64") => assert_eq!(url, "https://example.com/linux-x64"),
        ("linux", "aarch64") => assert_eq!(url, "https://example.com/linux-arm64"),
        _ => {
            // On unsupported platforms the earlier expect will already fail,
            // but guard here for completeness.
            panic!("unhandled platform: {}-{}", os, arch);
        }
    }
}
