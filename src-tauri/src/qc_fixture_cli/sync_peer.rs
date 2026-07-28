//! `debug-seed-qc-sync-peer` isolated fixture — CJ-05-17 sync recovery journey.
//!
//! After the ADR-003 directory-module split (#8765), the sync-peer recovery
//! fixture dispatch was relocated from the single `qc_fixture_cli.rs` into this
//! module. Behavior and contract are unchanged. The synthetic peer state itself
//! lives in `crate::qc_sync_peer`.

use std::fmt;
use std::path::Path;

use anyhow::{bail, Context, Result};
use maekon_core::config::{AppConfig, SyncTransportKind, MIN_SYNC_PASSPHRASE_LEN};
use maekon_core::config_manager::ConfigManager;

use super::recovery::configure_recovery_fixture;
use super::{require_exact_gate, require_isolated_profile};

const SYNC_PEER_COMMAND: &str = "debug-seed-qc-sync-peer";
const SYNC_PEER_GATE_ENV: &str = "MAEKON_DEBUG_QC_SYNC_PEER_FIXTURE";
const SYNC_PASSPHRASE_ENV: &str = "MAEKON_SYNC_PASSPHRASE";
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyncPeerSeedReport {
    data_dir: String,
    peer_count: usize,
    network_disabled: bool,
    keychain_untouched: bool,
}

impl fmt::Display for SyncPeerSeedReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC sync-peer fixture ready: data_dir={} peers={} network_disabled={} keychain_untouched={}",
            self.data_dir, self.peer_count, self.network_disabled, self.keychain_untouched
        )
    }
}

pub(crate) fn sync_peer_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(SYNC_PEER_COMMAND)
}

pub(crate) fn run_sync_peer_from_env() -> Result<SyncPeerSeedReport> {
    require_isolated_profile()?;
    require_exact_gate(SYNC_PEER_GATE_ENV)?;
    let passphrase = std::env::var(SYNC_PASSPHRASE_ENV)
        .context("MAEKON_SYNC_PASSPHRASE is required for the isolated sync fixture")?;
    if passphrase.chars().count() < MIN_SYNC_PASSPHRASE_LEN {
        bail!("MAEKON_SYNC_PASSPHRASE must be at least {MIN_SYNC_PASSPHRASE_LEN} characters")
    }

    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let sync_folder = data_dir.join("qc-sync-peer");
    std::fs::create_dir_all(&sync_folder).context("create isolated sync fixture directory")?;
    crate::qc_sync_peer::prepare_fixture(&data_dir)
        .map_err(anyhow::Error::from)
        .context("prepare isolated synthetic peer state")?;

    ConfigManager::new()
        .context("initialize isolated config")?
        .update_with(|config| {
            configure_sync_peer_fixture(config, &sync_folder);
            Ok(())
        })
        .context("persist isolated QC sync fixture config")?;

    Ok(SyncPeerSeedReport {
        data_dir: data_dir.display().to_string(),
        peer_count: 1,
        network_disabled: true,
        keychain_untouched: true,
    })
}

fn configure_sync_peer_fixture(config: &mut AppConfig, sync_folder: &Path) {
    configure_recovery_fixture(config);
    config.sync.enabled = true;
    config.sync.transport = SyncTransportKind::File;
    config.sync.sync_folder = Some(sync_folder.display().to_string());
    config.sync.lan_advertise = false;
    config.sync.device_name = "Synthetic local device".to_string();
}

#[cfg(test)]
mod tests {
    use super::super::COMMAND;
    use super::*;

    #[test]
    fn sync_peer_command_flag_is_explicit() {
        assert!(sync_peer_command_requested([SYNC_PEER_COMMAND].into_iter()));
        assert!(!sync_peer_command_requested([COMMAND].into_iter()));
    }

    #[test]
    fn sync_peer_fixture_enables_only_isolated_file_sync() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;
        config.audio.enabled = true;
        config.integration.enabled = true;
        config.telemetry.enabled = true;
        config.web.allow_external = true;
        config.external_grpc.enabled = true;
        config.automation.enabled = true;
        let folder = Path::new("isolated-sync-folder");

        configure_sync_peer_fixture(&mut config, folder);

        assert!(config.sync.enabled);
        assert_eq!(config.sync.transport, SyncTransportKind::File);
        assert_eq!(
            config.sync.sync_folder.as_deref(),
            Some("isolated-sync-folder")
        );
        assert!(!config.sync.lan_advertise);
        assert!(!config.vision.capture_enabled);
        assert!(!config.audio.enabled);
        assert!(!config.integration.enabled);
        assert!(!config.telemetry.enabled);
        assert!(!config.web.allow_external);
        assert!(!config.external_grpc.enabled);
        assert!(!config.automation.enabled);
    }
}
