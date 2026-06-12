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
        "maekon-{}-{}-to-{}.patch",
        platform, current_version, target_version
    );
    assets
        .iter()
        .find(|a| a.name == patch_name)
        .map(|a| (a.browser_download_url.clone(), a.size))
}

impl Updater {
    /// Returns `(download_url, asset_size)` for the first exact platform asset match.
    pub(super) fn find_platform_asset(
        &self,
        release: &ReleaseInfo,
    ) -> Result<(String, u64), UpdateError> {
        let platform_assets = Self::get_platform_asset_names()?;

        for expected_name in platform_assets {
            if let Some(asset) = release
                .assets
                .iter()
                .find(|asset| asset.name.eq_ignore_ascii_case(expected_name))
            {
                return Ok((asset.browser_download_url.clone(), asset.size));
            }
        }

        Err(UpdateError::NoSuitableAsset)
    }

    pub(super) fn get_platform_asset_names() -> Result<Vec<&'static str>, UpdateError> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Ok(vec![
                "maekon-macos-arm64.tar.gz",
                "maekon-macos-aarch64.tar.gz",
                "maekon-darwin-arm64.tar.gz",
                "maekon-darwin-aarch64.tar.gz",
                "maekon-macos-universal.tar.gz",
            ])
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Ok(vec![
                "maekon-macos-x64.tar.gz",
                "maekon-macos-x86_64.tar.gz",
                "maekon-darwin-x64.tar.gz",
                "maekon-darwin-x86_64.tar.gz",
                "maekon-macos-universal.tar.gz",
            ])
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            Ok(vec![
                "maekon-windows-x64.zip",
                "maekon-windows-x86_64.zip",
                "maekon-win64.zip",
                "maekon-win-x64.zip",
            ])
        }
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        {
            Ok(vec![
                "maekon-windows-arm64.zip",
                "maekon-windows-aarch64.zip",
                "maekon-win-arm64.zip",
            ])
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Ok(vec![
                "maekon-linux-x64.tar.gz",
                "maekon-linux-x86_64.tar.gz",
                "maekon-linux-amd64.tar.gz",
            ])
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            Ok(vec![
                "maekon-linux-arm64.tar.gz",
                "maekon-linux-aarch64.tar.gz",
            ])
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

    pub(super) fn get_platform_patterns() -> Result<Vec<&'static str>, UpdateError> {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Ok(vec![
                "macos-arm64",
                "darwin-arm64",
                "macos-aarch64",
                "darwin-aarch64",
                "macos-universal",
            ])
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Ok(vec![
                "macos-x64",
                "darwin-x64",
                "macos-x86_64",
                "darwin-x86_64",
                "macos-universal",
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

    /// #4836: whether `latest` is within the configured update **ceiling**.
    ///
    /// Returns `true` (offer the update) when no ceiling is set or
    /// `latest <= max_allowed_version`; returns `false` (hold at the current
    /// version) when the candidate is above the ceiling. This is the
    /// managed-config / MDM update kill-switch: an admin pins
    /// `update.max_allowed_version` (lockable via `managed.json`) to freeze a
    /// fleet at a known-good version while still permitting updates *up to* it.
    ///
    /// Unlike the floor (which `Err`s — a sub-floor release is a
    /// downgrade/supply-chain signal), an over-ceiling release is the *normal,
    /// expected* state when a fleet is pinned, so it is a silent hold, not an
    /// error. A ceiling string that fails to parse is treated as a hold
    /// (fail-closed): `validate_integrity_policy` rejects a malformed ceiling at
    /// config load, so this only guards a bypassed validation, and a broken
    /// freeze policy should hold rather than release.
    pub(super) fn update_ceiling_permits(&self, latest: &semver::Version) -> bool {
        let Some(max_raw) = self
            .config
            .max_allowed_version
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return true;
        };

        match semver::Version::parse(max_raw) {
            Ok(max_allowed) => latest <= &max_allowed,
            Err(e) => {
                tracing::warn!(
                    "update.max_allowed_version is not valid semver ({max_raw:?}: {e}); \
                     holding updates (fail-closed)"
                );
                false
            }
        }
    }
}
