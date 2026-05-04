// NOTE: This file (1200+ lines) already uses directory module pattern
// (github.rs, install.rs, state.rs). Consider further extraction if it
// continues to grow.

#![allow(dead_code)] // Updater wired via update_runtime.rs; methods called from IPC commands and scheduler

pub(crate) mod delta;
mod github;
pub(crate) mod health_probe;
mod install;
mod state;
mod trusted_keys;

// Re-exports from health_probe for consumers in app_runtime_launch + scheduler
// (wired in Task 8 per plan — scaffolded here during Task 1).
#[allow(unused_imports)]
pub(crate) use health_probe::{HealthProbe, ProbeError, RollbackReason, StartupAction};

#[allow(unused_imports)] // UpdateChannel used in #[cfg(test)] only
use oneshim_core::config::{UpdateChannel, UpdateConfig};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether the matched release asset is a full binary or a delta patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateAssetType {
    FullBinary,
    DeltaPatch { from_version: String },
}

/// Preview of an available update without downloading.
///
/// Does not verify checksums or signatures — those are enforced during
/// the actual download performed by `download_update`.
#[derive(Debug, Clone, Serialize)]
pub struct UpdatePreview {
    /// Version string of the release that was found.
    pub version: String,
    /// Total download size in bytes across all platform assets (0 = already up to date).
    pub download_size_bytes: u64,
    /// Number of release assets available for the current platform.
    pub asset_count: usize,
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("GitHub API request failed: {0}")]
    ApiRequest(#[from] reqwest::Error),

    #[error("Failed to parse API response: {0}")]
    ParseResponse(String),

    #[error("Failed to parse version: {0}")]
    VersionParse(#[from] semver::Error),

    #[error("Download failed: {0}")]
    Download(String),

    #[error("Installation failed: {0}")]
    Install(String),

    #[error("Unsupported platform: {0}")]
    UnsupportedPlatform(String),

    #[error("Filesystem error: {0}")]
    Filesystem(#[from] std::io::Error),

    #[error("Auto-update is disabled")]
    Disabled,

    #[error("Already on latest version")]
    AlreadyLatest,

    #[error("No suitable release asset found for current platform")]
    NoSuitableAsset,

    #[error("Integrity verification failed: {0}")]
    Integrity(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub name: Option<String>,
    pub body: Option<String>,
    pub prerelease: bool,
    pub assets: Vec<ReleaseAsset>,
    /// HTML URL
    pub html_url: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    /// Content-Type
    pub content_type: String,
}

#[derive(Debug)]
pub enum UpdateCheckResult {
    Available {
        current: semver::Version,
        latest: semver::Version,
        release: Box<ReleaseInfo>,
        download_url: String,
        download_size: Option<u64>,
        asset_type: UpdateAssetType,
    },
    UpToDate {
        current: semver::Version,
    },
}

pub struct Updater {
    pub(super) config: UpdateConfig,
    pub(super) http_client: reqwest::Client,
}

impl Updater {
    pub(super) const ALLOWED_DOWNLOAD_HOSTS: [&'static str; 4] = [
        "github.com",
        "api.github.com",
        "objects.githubusercontent.com",
        "githubusercontent.com",
    ];

    /// Returns the canonical platform tag used in delta patch asset names.
    /// E.g. `"macos-arm64"`, `"linux-x64"`, `"windows-x64"`.
    pub(super) fn get_platform_tag() -> String {
        let os = std::env::consts::OS;
        let arch = match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => other,
        };
        format!("{os}-{arch}")
    }

    pub fn new(config: UpdateConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .user_agent(format!("oneshim/{}", CURRENT_VERSION))
            .build()
            .expect("failed to build HTTP client");

        Self {
            config,
            http_client,
        }
    }

    #[cfg(test)]
    pub fn with_client(config: UpdateConfig, http_client: reqwest::Client) -> Self {
        Self {
            config,
            http_client,
        }
    }

    #[cfg(test)]
    pub async fn check_for_updates_with_base_url(
        &self,
        base_url: &str,
    ) -> Result<UpdateCheckResult, UpdateError> {
        self.check_for_updates_from(base_url).await
    }

    pub async fn check_for_updates(&self) -> Result<UpdateCheckResult, UpdateError> {
        self.check_for_updates_from("https://api.github.com").await
    }

    async fn check_for_updates_from(
        &self,
        base_url: &str,
    ) -> Result<UpdateCheckResult, UpdateError> {
        if !self.config.enabled {
            return Err(UpdateError::Disabled);
        }

        let current = semver::Version::parse(CURRENT_VERSION)?;
        let release = self.fetch_target_release(base_url).await?;

        let latest_tag = release.tag_name.trim_start_matches('v');
        let latest = semver::Version::parse(latest_tag)?;
        self.enforce_version_floor(&latest)?;

        if latest > current {
            let latest_str = latest.to_string();
            let current_str = current.to_string();

            // Staged rollout gate: check if this installation is in the rollout bucket.
            // D10 defensive None handling (Phase 4): treat a missing installation_id
            // as rollout-EXCLUDED (was: always-eligible). This prevents a config
            // regression from silently admitting the device to the first-receive
            // cohort. The UUID is auto-generated on first launch at
            // app_runtime_launch.rs:66-74, so None at this point is an invariant
            // violation.
            let rollout_percent = parse_rollout_percent(&release.body);
            let Some(ref installation_id) = self.config.installation_id else {
                tracing::warn!(
                    "installation_id missing — treating as rollout-excluded for v{latest_str}"
                );
                return Ok(UpdateCheckResult::UpToDate { current });
            };
            if !is_eligible_for_rollout(installation_id, &latest_str, rollout_percent) {
                tracing::debug!(
                    "Update v{latest_str} available but device not in rollout bucket ({rollout_percent}%)"
                );
                return Ok(UpdateCheckResult::UpToDate { current });
            }

            // Try delta patch first, fall back to full binary
            let platform = Self::get_platform_tag();
            if let Some((patch_url, patch_size)) =
                github::find_patch_asset(&release.assets, &platform, &current_str, &latest_str)
            {
                tracing::info!(
                    "Delta patch available: {current_str} -> {latest_str} ({patch_size} bytes)"
                );
                return Ok(UpdateCheckResult::Available {
                    current,
                    latest,
                    release: Box::new(release),
                    download_url: patch_url,
                    download_size: Some(patch_size),
                    asset_type: UpdateAssetType::DeltaPatch {
                        from_version: current_str,
                    },
                });
            }

            let (download_url, asset_size) = self.find_platform_asset(&release)?;

            Ok(UpdateCheckResult::Available {
                current,
                latest,
                release: Box::new(release),
                download_url,
                download_size: Some(asset_size),
                asset_type: UpdateAssetType::FullBinary,
            })
        } else {
            Ok(UpdateCheckResult::UpToDate { current })
        }
    }

    /// Preview available update info without downloading.
    ///
    /// Reports version, download size, and asset count for the latest release.
    /// Does NOT download, install, or verify checksums/signatures — those are
    /// enforced during the actual download performed by `download_update`.
    pub async fn preview_update_availability(&self) -> Result<UpdatePreview, UpdateError> {
        let result = self.check_for_updates().await?;
        match result {
            UpdateCheckResult::Available {
                latest, release, ..
            } => {
                let download_size_bytes = release.assets.iter().map(|a| a.size).sum::<u64>();
                let asset_count = release.assets.len();
                Ok(UpdatePreview {
                    version: latest.to_string(),
                    download_size_bytes,
                    asset_count,
                })
            }
            UpdateCheckResult::UpToDate { current } => Ok(UpdatePreview {
                version: current.to_string(),
                download_size_bytes: 0,
                asset_count: 0,
            }),
        }
    }

    /// Fetch the target release from GitHub.
    /// When the effective channel includes prereleases (PreRelease or Nightly),
    /// queries `/releases` (all releases, newest first) so that RC/beta/nightly
    /// tags are visible. Otherwise, uses `/releases/latest` (stable only).
    async fn fetch_target_release(&self, base_url: &str) -> Result<ReleaseInfo, UpdateError> {
        let wants_prerelease = self.config.effective_channel().includes_prerelease();
        let url = if wants_prerelease {
            format!(
                "{}/repos/{}/{}/releases?per_page=1",
                base_url, self.config.repo_owner, self.config.repo_name
            )
        } else {
            format!(
                "{}/repos/{}/{}/releases/latest",
                base_url, self.config.repo_owner, self.config.repo_name
            )
        };

        let response = self.http_client.get(&url).send().await?;

        if !response.status().is_success() {
            return Err(UpdateError::ParseResponse(format!(
                "API response status: {}",
                response.status()
            )));
        }

        if wants_prerelease {
            let releases: Vec<ReleaseInfo> = response.json().await?;
            releases
                .into_iter()
                .next()
                .ok_or_else(|| UpdateError::ParseResponse("No releases found".to_string()))
        } else {
            Ok(response.json().await?)
        }
    }
}

/// Deterministic FNV-1a hash for rollout bucketing.
/// Stable across Rust versions (unlike `DefaultHasher`).
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Check if this installation is eligible for a staged rollout.
fn is_eligible_for_rollout(installation_id: &str, version: &str, rollout_percent: u8) -> bool {
    if rollout_percent >= 100 {
        return true;
    }
    if rollout_percent == 0 {
        return false;
    }
    let mut data = installation_id.as_bytes().to_vec();
    data.extend_from_slice(version.as_bytes());
    let hash = fnv1a_hash(&data);
    (hash % 100) < rollout_percent as u64
}

/// Parse rollout percentage from GitHub release body.
/// Looks for `<!-- rollout:N -->` comment. Returns 100 if absent or invalid.
fn parse_rollout_percent(body: &Option<String>) -> u8 {
    let Some(body) = body else { return 100 };
    if let Some(start) = body.find("<!-- rollout:") {
        let after = &body[start + 13..];
        if let Some(end) = after.find("-->") {
            if let Ok(percent) = after[..end].trim().parse::<u8>() {
                return percent.min(100);
            }
        }
    }
    100
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
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
            // Task 3 (Phase 4 D10): installation_id must be Some(...) — the
            // defensive None handling at mod.rs:197 now treats None as
            // rollout-excluded. Tests that want to exercise the None branch
            // explicitly override this field.
            installation_id: Some("test-install-00000000-0000-0000-0000-000000000000".to_string()),
            require_signature_verification: false,
            signature_public_key: String::new(),
            min_allowed_version: None,
        }
    }

    #[test]
    fn current_version_is_valid_semver() {
        let version = semver::Version::parse(CURRENT_VERSION);
        assert!(version.is_ok(), "CURRENT_VERSION must be a valid semver");
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
        let patterns = Updater::get_platform_patterns();
        assert!(
            patterns.is_ok(),
            "Current platform must have at least one pattern"
        );
        assert!(!patterns.unwrap().is_empty());
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
        let asset_name = "oneshim-macos-arm64.tar.gz";
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        let asset_name = "oneshim-macos-x64.tar.gz";
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        let asset_name = "oneshim-windows-x64.zip";
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let asset_name = "oneshim-linux-x64.tar.gz";
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )))]
        let asset_name = "oneshim-unknown.tar.gz";

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
        assert!(result.is_ok());
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
        let asset_name = "oneshim-macos-arm64.tar.gz";
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        let asset_name = "oneshim-macos-x64.tar.gz";
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        let asset_name = "oneshim-windows-x64.zip";
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let asset_name = "oneshim-linux-x64.tar.gz";
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )))]
        let asset_name = "oneshim-unknown.tar.gz";

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
        let asset_name = "oneshim-macos-arm64.tar.gz";
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        let asset_name = "oneshim-macos-x64.tar.gz";
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        let asset_name = "oneshim-windows-x64.zip";
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let asset_name = "oneshim-linux-x64.tar.gz";
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )))]
        let asset_name = "oneshim-unknown.tar.gz";

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
        let asset_name = "oneshim-macos-arm64.tar.gz";
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        let asset_name = "oneshim-macos-x64.tar.gz";
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        let asset_name = "oneshim-windows-x64.zip";
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let asset_name = "oneshim-linux-x64.tar.gz";
        #[cfg(not(any(
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "x86_64"),
        )))]
        let asset_name = "oneshim-unknown.tar.gz";

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

    #[test]
    fn parse_sha256_manifest_validates_format() {
        let hash = Updater::parse_sha256_manifest(
            "8f434346648f6b96df89dda901c5176b10a6d83961fca6f18e40f9f0f84f2304  oneshim.tar.gz",
        )
        .unwrap();
        assert_eq!(
            hash,
            "8f434346648f6b96df89dda901c5176b10a6d83961fca6f18e40f9f0f84f2304"
        );
    }

    #[test]
    fn parse_sha256_manifest_rejects_invalid_hash() {
        let err = Updater::parse_sha256_manifest("not-a-valid-hash  oneshim.tar.gz");
        assert!(matches!(err, Err(UpdateError::Integrity(_))));
    }

    #[test]
    fn validate_download_url_rejects_http_and_unknown_host() {
        let updater = Updater::new(test_config());

        let http_url = updater.validate_download_url("http://github.com/file.tar.gz");
        assert!(http_url.is_err());

        let unknown_host = updater.validate_download_url("https://evil.example.com/file.tar.gz");
        assert!(unknown_host.is_err());
    }

    #[test]
    fn extract_zip_rejects_path_traversal_entries() {
        use std::io::Write;

        let updater = Updater::new(test_config());
        let dir = tempdir().unwrap();
        let zip_path = dir.path().join("malicious.zip");

        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
            writer.start_file("../../outside", options).unwrap();
            writer.write_all(b"malicious").unwrap();
            writer.finish().unwrap();
        }

        let result = updater.extract_zip(&zip_path);
        assert!(matches!(result, Err(UpdateError::Install(_))));
    }

    #[test]
    fn verify_signature_accepts_valid_ed25519_signature() {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();

        let mut config = test_config();
        config.require_signature_verification = true;
        config.signature_public_key = BASE64.encode(verifying_key.as_bytes());
        let updater = Updater::new(config);

        let payload = b"oneshim-release-artifact";
        let signature = signing_key.sign(payload);

        let result = updater.verify_signature(payload, signature.to_bytes().as_slice());
        assert!(result.is_ok());
    }

    #[test]
    fn verify_signature_rejects_invalid_signature() {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[9u8; 32]);
        let verifying_key = signing_key.verifying_key();

        let mut config = test_config();
        config.require_signature_verification = true;
        config.signature_public_key = BASE64.encode(verifying_key.as_bytes());
        let updater = Updater::new(config);

        let payload = b"artifact-A";
        let signature = signing_key.sign(payload);

        let result = updater.verify_signature(b"artifact-B", signature.to_bytes().as_slice());
        assert!(matches!(result, Err(UpdateError::Integrity(_))));
    }

    // ── D9 multi-key trust tests ──────────────────────────────────────

    #[test]
    fn verify_signature_accepts_builtin_key() {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[11u8; 32]);
        let builtin_key = BASE64.encode(signing_key.verifying_key().as_bytes());

        let payload = b"builtin-release-artifact";
        let signature = signing_key.sign(payload);

        // Inject a single-entry trusted array; no configured key override.
        let trusted = [builtin_key.as_str()];
        let result = Updater::verify_signature_with_keys(
            &trusted,
            None,
            payload,
            signature.to_bytes().as_slice(),
        );
        assert!(result.is_ok(), "Builtin key should validate");
    }

    #[test]
    fn verify_signature_accepts_second_trusted_key_when_first_inactive() {
        use ed25519_dalek::{Signer, SigningKey};

        // First key in array is one we do NOT sign with.
        let first_key_unused = SigningKey::from_bytes(&[0u8; 32]).verifying_key();
        // Second key is the one that signs the payload.
        let second_key = SigningKey::from_bytes(&[12u8; 32]);

        let trusted_first = BASE64.encode(first_key_unused.as_bytes());
        let trusted_second = BASE64.encode(second_key.verifying_key().as_bytes());

        let payload = b"mid-rotation-artifact";
        let signature = second_key.sign(payload);

        let trusted = [trusted_first.as_str(), trusted_second.as_str()];
        let result = Updater::verify_signature_with_keys(
            &trusted,
            None,
            payload,
            signature.to_bytes().as_slice(),
        );
        assert!(
            result.is_ok(),
            "Second trusted key should validate during rotation"
        );
    }

    #[test]
    fn verify_signature_fallback_to_configured_key_when_not_in_array() {
        use ed25519_dalek::{Signer, SigningKey};

        let builtin = SigningKey::from_bytes(&[13u8; 32]).verifying_key();
        let configured = SigningKey::from_bytes(&[14u8; 32]);

        let trusted_only = BASE64.encode(builtin.as_bytes());
        let configured_b64 = BASE64.encode(configured.verifying_key().as_bytes());

        let payload = b"user-override-artifact";
        let signature = configured.sign(payload);

        // configured key is NOT in the trusted list → fallback should hit.
        let trusted = [trusted_only.as_str()];
        let result = Updater::verify_signature_with_keys(
            &trusted,
            Some(configured_b64.as_str()),
            payload,
            signature.to_bytes().as_slice(),
        );
        assert!(
            result.is_ok(),
            "Configured override should validate via fallback"
        );
    }

    #[test]
    fn verify_signature_rejects_payload_when_no_key_matches() {
        use ed25519_dalek::{Signer, SigningKey};

        let unknown = SigningKey::from_bytes(&[15u8; 32]);
        let payload = b"untrusted-artifact";
        let signature = unknown.sign(payload);

        // Provide a trusted list that does NOT include the unknown key, and
        // no configured override.
        let other = SigningKey::from_bytes(&[16u8; 32]).verifying_key();
        let trusted_entry = BASE64.encode(other.as_bytes());
        let trusted = [trusted_entry.as_str()];

        let result = Updater::verify_signature_with_keys(
            &trusted,
            None,
            payload,
            signature.to_bytes().as_slice(),
        );
        assert!(matches!(result, Err(UpdateError::Integrity(_))));
    }

    #[test]
    fn validate_integrity_policy_allows_empty_public_key() {
        use oneshim_core::config::UpdateConfig;

        let mut config = UpdateConfig {
            enabled: true,
            repo_owner: "pseudotop".to_string(),
            repo_name: "oneshim-client".to_string(),
            check_interval_hours: 24,
            channel: UpdateChannel::default(),
            include_prerelease: false,
            auto_install: false,
            installation_id: Some("test-install-id".to_string()),
            require_signature_verification: true,
            signature_public_key: String::new(), // empty override — should NOT error
            min_allowed_version: None,
        };

        assert!(
            config.validate_integrity_policy().is_ok(),
            "Empty signature_public_key with updates enabled must be OK (D9 array is authoritative)"
        );

        // Also confirm a malformed non-empty override still errors.
        config.signature_public_key = "not-valid-base64!!!".to_string();
        assert!(
            config.validate_integrity_policy().is_err(),
            "Malformed signature_public_key should still error"
        );
    }

    #[test]
    fn release_reliability_validate_download_url_allows_localhost_in_tests() {
        let updater = Updater::new(test_config());
        assert!(updater
            .validate_download_url("https://localhost/oneshim-update.tar.gz")
            .is_ok());
        assert!(updater
            .validate_download_url("https://127.0.0.1/oneshim-update.tar.gz")
            .is_ok());
    }

    #[tokio::test]
    async fn release_reliability_download_update_accepts_localhost_with_integrity() {
        let mut server = mockito::Server::new_async().await;
        let asset_name = "oneshim-test-update.tar.gz";
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
        let asset_name = "oneshim-test-update.tar.gz";

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
        let current_exe = dir.path().join("oneshim-current");
        let downloaded = dir.path().join("oneshim-new");
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
        let current_exe = dir.path().join("oneshim-current");
        let downloaded = dir.path().join("oneshim-new");
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
        let current_exe = dir.path().join("oneshim-current");
        let downloaded = dir.path().join("oneshim-new");
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
        assert!(result.is_ok(), "install-and-restart should succeed");

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
        let current_exe = dir.path().join("oneshim-current");
        let downloaded = dir.path().join("oneshim-new");
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
        let content = b"oneshim release artifact payload v42";
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
            Updater::is_safe_archive_path(Path::new("bin/oneshim")),
            "normal nested path should be accepted"
        );
        assert!(
            Updater::is_safe_archive_path(Path::new("oneshim")),
            "root-level file should be accepted"
        );
        assert!(
            Updater::is_safe_archive_path(Path::new("./oneshim")),
            "current-dir prefixed path should be accepted"
        );
        assert!(
            Updater::is_safe_archive_path(Path::new("release/bin/oneshim")),
            "deep nested path should be accepted"
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

        // GitHub HTTPS URLs are accepted
        assert!(updater
            .validate_download_url(
                "https://github.com/pseudotop/maekon-client/releases/download/v1.0.0/asset.tar.gz"
            )
            .is_ok());
        assert!(updater
            .validate_download_url(
                "https://objects.githubusercontent.com/github-releases/asset.tar.gz"
            )
            .is_ok());

        // Evil domains are rejected
        assert!(updater
            .validate_download_url("https://evil.com/malware.tar.gz")
            .is_err());
        assert!(updater
            .validate_download_url("https://not-github.com/fake.tar.gz")
            .is_err());

        // HTTP (non-HTTPS) is rejected for non-localhost
        assert!(updater
            .validate_download_url("http://github.com/asset.tar.gz")
            .is_err());
    }

    // -------------------------------------------------------------------
    // Platform-Specific E2E Update Tests
    // -------------------------------------------------------------------

    /// Verify that check_for_updates can reach the real GitHub API and parse a response.
    /// Requires network access — marked #[ignore] for CI.
    #[tokio::test]
    #[ignore]
    async fn e2e_check_for_updates_reaches_github() {
        let config = UpdateConfig {
            enabled: true,
            repo_owner: "pseudotop".to_string(),
            repo_name: "oneshim-client".to_string(),
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
            repo_name: "oneshim-client".to_string(),
            channel: UpdateChannel::default(),
            include_prerelease: false,
            ..UpdateConfig::default()
        };
        let updater = Updater::new(config);

        let result = updater.preview_update_availability().await;

        match result {
            Ok(preview) => {
                // Version string must be valid semver
                assert!(
                    semver::Version::parse(&preview.version).is_ok(),
                    "preview result version must be valid semver, got: {}",
                    preview.version
                );
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
                    name: "oneshim-macos-arm64.tar.gz".to_string(),
                    browser_download_url: "https://example.com/macos-arm64".to_string(),
                    size: 10_000,
                    content_type: "application/gzip".to_string(),
                },
                ReleaseAsset {
                    name: "oneshim-macos-x64.tar.gz".to_string(),
                    browser_download_url: "https://example.com/macos-x64".to_string(),
                    size: 10_000,
                    content_type: "application/gzip".to_string(),
                },
                ReleaseAsset {
                    name: "oneshim-windows-x64.zip".to_string(),
                    browser_download_url: "https://example.com/windows-x64".to_string(),
                    size: 12_000,
                    content_type: "application/zip".to_string(),
                },
                ReleaseAsset {
                    name: "oneshim-linux-x64.tar.gz".to_string(),
                    browser_download_url: "https://example.com/linux-x64".to_string(),
                    size: 9_000,
                    content_type: "application/gzip".to_string(),
                },
                ReleaseAsset {
                    name: "oneshim-linux-arm64.tar.gz".to_string(),
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

    // -------------------------------------------------------------------
    // Task 7: Staged Rollout Tests (FNV-1a bucketing + rollout parsing)
    // -------------------------------------------------------------------

    #[test]
    fn fnv1a_hash_deterministic() {
        let h1 = fnv1a_hash(b"test-device-v1.0.0");
        let h2 = fnv1a_hash(b"test-device-v1.0.0");
        assert_eq!(h1, h2);
    }

    #[test]
    fn fnv1a_hash_different_inputs() {
        let h1 = fnv1a_hash(b"device-a-v1.0.0");
        let h2 = fnv1a_hash(b"device-b-v1.0.0");
        assert_ne!(h1, h2);
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
        let r1 = is_eligible_for_rollout("device-123", "v2.0.0", 50);
        let r2 = is_eligible_for_rollout("device-123", "v2.0.0", 50);
        assert_eq!(r1, r2);
    }

    #[test]
    fn parse_rollout_present() {
        let body = Some("<!-- rollout:25 -->\n## Changes".to_string());
        assert_eq!(parse_rollout_percent(&body), 25);
    }

    #[test]
    fn parse_rollout_absent() {
        let body = Some("## Changes\n- Fix bugs".to_string());
        assert_eq!(parse_rollout_percent(&body), 100);
    }

    #[test]
    fn parse_rollout_none() {
        assert_eq!(parse_rollout_percent(&None), 100);
    }

    #[test]
    fn parse_rollout_caps_at_100() {
        let body = Some("<!-- rollout:150 -->".to_string());
        assert_eq!(parse_rollout_percent(&body), 100);
    }

    // ── D10 defensive None handling + rollout-gate end-to-end ─────────

    /// When a release body contains `<!-- rollout:0 -->`, every installation
    /// is excluded from the rollout bucket. `check_for_updates` must return
    /// `UpToDate` (no update offered) even though the semver comparison
    /// reports an available newer version.
    #[tokio::test]
    async fn update_check_respects_rollout_exclusion() {
        let mut server = mockito::Server::new_async().await;
        let newer_version = "99.0.0";

        let mock = server
            .mock("GET", "/repos/test-owner/test-repo/releases/latest")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{
                "tag_name": "v{}",
                "name": "Rollout Excluded",
                "body": "new features\n\n<!-- rollout:0 -->\n",
                "prerelease": false,
                "assets": [],
                "html_url": "https://github.com/test/releases/v99.0.0",
                "published_at": "2024-01-01T00:00:00Z"
            }}"#,
                newer_version
            ))
            .create_async()
            .await;

        let config = test_config(); // installation_id = Some("test-install-...")
        let updater = Updater::new(config);

        let result = updater.check_for_updates_with_base_url(&server.url()).await;
        mock.assert_async().await;

        match result {
            Ok(UpdateCheckResult::UpToDate { .. }) => {
                // Expected: rollout:0 excludes every device.
            }
            other => unreachable!("Expected UpToDate on rollout:0, got {:?}", other),
        }
    }

    /// When `installation_id` is `None` at check time (regression against the
    /// invariant that `app_runtime_launch.rs:66-74` writes a UUID before any
    /// update check spawns), the updater must treat the device as
    /// rollout-EXCLUDED rather than admitting it as always-eligible.
    #[tokio::test]
    async fn update_check_without_installation_id_is_excluded() {
        let mut server = mockito::Server::new_async().await;
        let newer_version = "99.0.0";

        let mock = server
            .mock("GET", "/repos/test-owner/test-repo/releases/latest")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{
                "tag_name": "v{}",
                "name": "Rollout 100%",
                "body": "new features",
                "prerelease": false,
                "assets": [],
                "html_url": "https://github.com/test/releases/v99.0.0",
                "published_at": "2024-01-01T00:00:00Z"
            }}"#,
                newer_version
            ))
            .create_async()
            .await;

        let mut config = test_config();
        config.installation_id = None; // regression scenario

        let updater = Updater::new(config);

        let result = updater.check_for_updates_with_base_url(&server.url()).await;
        mock.assert_async().await;

        match result {
            Ok(UpdateCheckResult::UpToDate { .. }) => {
                // Expected: None → defensive-exclude even at rollout:100.
            }
            other => unreachable!("Expected UpToDate on None installation_id, got {:?}", other),
        }
    }
}
