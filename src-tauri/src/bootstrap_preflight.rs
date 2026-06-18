use anyhow::Result;
use maekon_core::config::{AiAccessMode, AppConfig};
use std::path::Path;
use tracing::{info, warn};

use crate::cli_subscription_bridge::{
    default_context_export_path, should_autoinstall_bridge_files, should_include_user_scope,
    sync_bridge_files,
};

pub(crate) struct BootstrapPreflightCoordinator;

impl BootstrapPreflightCoordinator {
    /// Run boot-time preflight. The CLI-subscription bridge sync is genuinely
    /// optional (non-fatal), but the integrity preflight is fail-closed: its
    /// `Err` is propagated so a downgraded updater trust chain or a tampered
    /// provisioned policy bundle aborts startup, matching `integrity_guard`'s
    /// unit-level contract and the `preflight_*` tests (#6257 — the previous
    /// implementation swallowed this `Err` into a warning, making the gate
    /// fail-open at runtime).
    pub(crate) fn run(config: &AppConfig, data_dir: &Path, offline_mode: bool) -> Result<()> {
        maybe_sync_cli_subscription_bridge(config, data_dir);

        crate::integrity_guard::run_preflight(config, offline_mode)
    }
}

fn maybe_sync_cli_subscription_bridge(config: &AppConfig, data_dir: &Path) {
    if config.ai_provider.access_mode != AiAccessMode::ProviderSubscriptionCli {
        return;
    }
    if !should_autoinstall_bridge_files() {
        info!(
            "ProviderSubscriptionCli mode: CLI bridge auto-install disabled (MAEKON_CLI_BRIDGE_AUTOINSTALL=1)"
        );
        return;
    }

    let project_root = std::env::current_dir().unwrap_or_else(|_| data_dir.to_path_buf());
    let include_user_scope = should_include_user_scope();
    let context_export_path = default_context_export_path(data_dir);
    let report = sync_bridge_files(&project_root, &context_export_path, include_user_scope);
    info!(
        project_root = %project_root.display(),
        context_export = %context_export_path.display(),
        written = report.written_files.len(),
        unchanged = report.unchanged_files.len(),
        errors = report.errors.len(),
        "CLI subscription bridge sync complete"
    );
    if !report.is_successful() {
        for error in report.errors {
            warn!(error = %error, "CLI subscribe file failure");
        }
    }
}
