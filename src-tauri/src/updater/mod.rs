// OOS-TBD: ADR-013 file split applied — mod.rs (1724L) split:
//   production code retained here (<400L), test suite extracted to tests.rs.
#![allow(dead_code)] // Updater wired via update_runtime.rs; methods called from IPC commands and scheduler

pub(crate) mod delta;
mod github;
pub(crate) mod health_probe;
mod install;
mod state;
mod trusted_keys;

// Re-exports from health_probe for consumers in app_runtime_launch + scheduler
#[allow(unused_imports)]
pub(crate) use health_probe::{HealthProbe, ProbeError, RollbackReason, StartupAction};

#[allow(unused_imports)] // UpdateChannel used in #[cfg(test)] only
use maekon_core::config::{UpdateChannel, UpdateConfig};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(test)]
pub(crate) use install::SignatureKeySource;

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
            .user_agent(format!("maekon/{}", CURRENT_VERSION))
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
        let metadata_base_url = self.validate_metadata_base_url(base_url)?;
        let metadata_base_url = metadata_base_url.as_str().trim_end_matches('/').to_string();
        let release = self.fetch_target_release(&metadata_base_url).await?;

        let latest_tag = release.tag_name.trim_start_matches('v');
        let latest = semver::Version::parse(latest_tag)?;
        self.enforce_version_floor(&latest)?;

        if latest > current {
            let latest_str = latest.to_string();
            let current_str = current.to_string();

            // #4836: managed-config update ceiling (MDM kill-switch). A release
            // above `update.max_allowed_version` is HELD — reported as
            // up-to-date (not an error), exactly like a rollout-excluded device
            // — so an admin can freeze the fleet at a known-good version via a
            // `managed.json` lock while still allowing updates up to the cap.
            // Checked before the rollout gate: a policy ceiling overrides any
            // rollout bucket assignment.
            if !self.update_ceiling_permits(&latest) {
                tracing::info!(
                    "Update v{latest_str} available but held by the update.max_allowed_version ceiling"
                );
                return Ok(UpdateCheckResult::UpToDate { current });
            }

            // D10 defensive None handling: treat a missing installation_id
            // as rollout-EXCLUDED.
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

            // Try delta patch first, fall back to full binary.
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

        // #6941: cap the releases JSON body before parse — a forged multi-GB body
        // from a MITM'd GitHub-allowlisted host would otherwise OOM the agent.
        let body =
            install::read_body_capped_update(response, install::MAX_AUX_UPDATE_BYTES).await?;
        if wants_prerelease {
            let releases: Vec<ReleaseInfo> = serde_json::from_slice(&body)
                .map_err(|e| UpdateError::ParseResponse(format!("parse releases: {e}")))?;
            releases
                .into_iter()
                .next()
                .ok_or_else(|| UpdateError::ParseResponse("No releases found".to_string()))
        } else {
            serde_json::from_slice(&body)
                .map_err(|e| UpdateError::ParseResponse(format!("parse release: {e}")))
        }
    }
}

/// Deterministic FNV-1a hash for rollout bucketing.
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
mod tests;
