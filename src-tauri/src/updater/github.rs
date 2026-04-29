//! GitHub API: fetch releases, parse JSON, select asset.
#![allow(dead_code)] // GitHub API helpers called from updater check/download paths

use super::{ReleaseAsset, ReleaseInfo, UpdateError, Updater};

/// Look for a delta patch asset matching current -> target version.
/// Returns `(download_url, size)` if found.
pub(super) fn find_patch_asset(
    assets: &[ReleaseAsset],
    platform: &str,
    current_version: &str,
    target_version: &str,
) -> Option<(String, u64)> {
    let patch_name = format!(
        "oneshim-{}-{}-to-{}.patch",
        platform, current_version, target_version
    );
    assets
        .iter()
        .find(|a| a.name == patch_name)
        .map(|a| (a.browser_download_url.clone(), a.size))
}

impl Updater {
    /// Returns `(download_url, asset_size)` for the first matching platform asset.
    pub(super) fn find_platform_asset(
        &self,
        release: &ReleaseInfo,
    ) -> Result<(String, u64), UpdateError> {
        let platform_patterns = Self::get_platform_patterns()?;

        for asset in &release.assets {
            let name_lower = asset.name.to_lowercase();
            for pattern in &platform_patterns {
                if name_lower.contains(pattern) {
                    return Ok((asset.browser_download_url.clone(), asset.size));
                }
            }
        }

        Err(UpdateError::NoSuitableAsset)
    }

    pub(super) fn get_platform_patterns() -> Result<Vec<&'static str>, UpdateError> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Ok(vec![
                "macos-arm64",
                "darwin-arm64",
                "macos-aarch64",
                "darwin-aarch64",
            ])
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Ok(vec![
                "macos-x64",
                "darwin-x64",
                "macos-x86_64",
                "darwin-x86_64",
            ])
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            Ok(vec!["windows-x64", "windows-x86_64", "win64", "win-x64"])
        }
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        {
            Ok(vec!["windows-arm64", "windows-aarch64", "win-arm64"])
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Ok(vec!["linux-x64", "linux-x86_64", "linux-amd64"])
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            Ok(vec!["linux-arm64", "linux-aarch64"])
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
            Err(UpdateError::UnsupportedPlatform(format!(
                "{}-{}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )))
        }
    }

    pub(super) fn enforce_version_floor(
        &self,
        latest: &semver::Version,
    ) -> Result<(), UpdateError> {
        let Some(min_allowed) = self
            .config
            .min_allowed_version
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return Ok(());
        };

        let min_allowed = semver::Version::parse(min_allowed)
            .map_err(|e| UpdateError::Integrity(format!("Invalid min_allowed_version: {}", e)))?;

        if latest < &min_allowed {
            return Err(UpdateError::Integrity(format!(
                "Release version {} is below configured minimum {}",
                latest, min_allowed
            )));
        }

        Ok(())
    }
}
