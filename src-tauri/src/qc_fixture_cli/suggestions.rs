use std::path::Path;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use maekon_core::config::{AppConfig, ConfirmationRequirement};
use maekon_core::config_manager::ConfigManager;
use maekon_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::sqlite::SqliteStorage;

use crate::storage_runtime::resolve_shared_master_key;

use super::types::SuggestionSeedReport;
use super::{
    require_exact_gate, require_isolated_profile, ACTION_SUGGESTION_GATE_ENV, ACTION_SUGGESTION_ID,
    ACTION_SUGGESTION_MARKER_KEY, MARKER_IN_PROGRESS, SUGGESTION_GATE_ENV, SUGGESTION_ID,
    SUGGESTION_MARKER_KEY, SUGGESTION_MARKER_VERSION,
};

pub(crate) fn run_suggestion_from_env() -> Result<SuggestionSeedReport> {
    require_isolated_profile()?;
    require_exact_gate(SUGGESTION_GATE_ENV)?;

    let config = ConfigManager::new()
        .context("initialize isolated config")?
        .get();
    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let encryption_key =
        resolve_shared_master_key(&data_dir).context("resolve isolated profile encryption key")?;

    seed_suggestion_fixture(&data_dir, encryption_key, config.storage.retention_days)
}

pub(crate) fn run_action_suggestion_from_env() -> Result<SuggestionSeedReport> {
    require_isolated_profile()?;
    require_exact_gate(ACTION_SUGGESTION_GATE_ENV)?;

    let config = ConfigManager::new()
        .context("initialize isolated config")?
        .update_with(|config| {
            // The pending-suggestion DTO only exposes a bound preset while
            // automation is enabled. Keep execution blocked by default so the
            // fixture deterministically exercises the production policy and
            // audit gates before a later QC step opts into an allowed policy.
            configure_action_suggestion_fixture(config);
            Ok(())
        })
        .context("enable automation in isolated action-suggestion fixture")?;
    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let encryption_key =
        resolve_shared_master_key(&data_dir).context("resolve isolated profile encryption key")?;

    seed_action_suggestion_fixture(&data_dir, encryption_key, config.storage.retention_days)
}

pub(super) fn configure_action_suggestion_fixture(config: &mut AppConfig) {
    config.automation.enabled = true;
    config.automation.confirmation_policy = ConfirmationRequirement::Block;
}

pub(super) fn seed_suggestion_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<SuggestionSeedReport> {
    let suggestion = Suggestion {
        suggestion_id: SUGGESTION_ID.to_string(),
        suggestion_type: SuggestionType::ProductivityTip,
        content: "Pause for a moment, review the current task, and choose the next concrete step."
            .to_string(),
        priority: Priority::Medium,
        confidence_score: 0.88,
        relevance_score: 0.9,
        is_actionable: false,
        created_at: Utc::now(),
        expires_at: None,
        source: SuggestionSource::RuleBased,
        reasoning: Some(
            "Deterministic local-only QC fixture for suggestion feedback validation.".to_string(),
        ),
        context_scope: None,
    };
    persist_suggestion_fixture(
        data_dir,
        encryption_key,
        retention_days,
        SUGGESTION_MARKER_KEY,
        SUGGESTION_MARKER_VERSION,
        suggestion,
    )
}

pub(super) fn seed_action_suggestion_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<SuggestionSeedReport> {
    let suggestion = Suggestion {
        suggestion_id: ACTION_SUGGESTION_ID.to_string(),
        suggestion_type: SuggestionType::NeedFocusTime,
        content: "Clear the desktop and start a protected focus session for the current task."
            .to_string(),
        priority: Priority::High,
        confidence_score: 0.91,
        relevance_score: 0.93,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: SuggestionSource::RuleBased,
        reasoning: Some(
            "Deterministic local-only QC fixture for suggestion-to-automation validation."
                .to_string(),
        ),
        context_scope: None,
    };
    persist_suggestion_fixture(
        data_dir,
        encryption_key,
        retention_days,
        ACTION_SUGGESTION_MARKER_KEY,
        SUGGESTION_MARKER_VERSION,
        suggestion,
    )
}

fn persist_suggestion_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
    marker_key: &str,
    marker_version: &str,
    suggestion: Suggestion,
) -> Result<SuggestionSeedReport> {
    let db_path = data_dir.join("maekon.db");
    let storage = SqliteStorage::open(&db_path, retention_days, Some(&encryption_key))
        .context("open isolated encrypted QC database")?;

    match storage.get_meta(marker_key).as_deref() {
        Some(version) if version == marker_version => {
            return Ok(SuggestionSeedReport {
                data_dir: data_dir.display().to_string(),
                suggestions: 0,
                already_seeded: true,
            });
        }
        Some(MARKER_IN_PROGRESS) => {
            bail!(
                "an earlier suggestion seed attempt did not finish; discard this isolated QC profile and retry"
            );
        }
        Some(other) => bail!("unsupported QC suggestion fixture marker version: {other}"),
        None => {}
    }

    storage
        .set_meta_checked(marker_key, MARKER_IN_PROGRESS)
        .context("mark QC suggestion fixture seed in progress")?;
    let expected_id = suggestion.suggestion_id.clone();
    let persisted_id = storage
        .save_rule_suggestion_sync(&suggestion)
        .context("persist deterministic QC suggestion")?;
    if persisted_id != expected_id {
        bail!("QC suggestion persistence was rejected by the storage write barrier")
    }

    storage
        .set_meta_checked(marker_key, marker_version)
        .context("commit QC suggestion fixture marker")?;

    Ok(SuggestionSeedReport {
        data_dir: data_dir.display().to_string(),
        suggestions: 1,
        already_seeded: false,
    })
}
