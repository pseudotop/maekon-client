//! Debug-only deterministic fixture seeder for private capture-history QC.
//!
//! This module is compiled only with `debug_assertions`. The command is
//! additionally fail-closed behind explicit environment gates and a dedicated
//! `qc-*`/`tc-*` application flavor, so it cannot write into a normal profile.

use anyhow::{bail, Context, Result};

mod audio;
mod claims;
mod history;
mod recovery;
mod suggestions;
mod sync_peer;
mod types;

pub(crate) use audio::run_audio_from_env;
pub(crate) use claims::run_claims_from_env;
pub(crate) use history::run_from_env;
pub(crate) use recovery::{
    legacy_migration_prepare_command_requested, legacy_migration_verify_command_requested,
    run_legacy_migration_prepare_from_env, run_legacy_migration_verify_from_env,
};
pub(crate) use suggestions::{run_action_suggestion_from_env, run_suggestion_from_env};
pub(crate) use sync_peer::{run_sync_peer_from_env, sync_peer_command_requested};

const COMMAND: &str = "debug-seed-qc-history";
const SUGGESTION_COMMAND: &str = "debug-seed-qc-suggestion";
const ACTION_SUGGESTION_COMMAND: &str = "debug-seed-qc-action-suggestion";
const CLAIMS_COMMAND: &str = "debug-seed-qc-claims";
const AUDIO_COMMAND: &str = "debug-seed-qc-audio";
const DEBUG_GATE_ENV: &str = "MAEKON_DEBUG_QC_FIXTURE_CLI";
const ISOLATED_GATE_ENV: &str = "MAEKON_TC_ISOLATED_PROFILE";
const SUGGESTION_GATE_ENV: &str = "MAEKON_DEBUG_QC_SUGGESTION_FIXTURE";
const ACTION_SUGGESTION_GATE_ENV: &str = "MAEKON_DEBUG_QC_ACTION_SUGGESTION_FIXTURE";
const CLAIMS_GATE_ENV: &str = "MAEKON_DEBUG_QC_CLAIMS_FIXTURE";
const AUDIO_GATE_ENV: &str = "MAEKON_DEBUG_QC_AUDIO_FIXTURE";
const FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
const PIN_ENV: &str = "MAEKON_TC_FIXTURE_PIN";
const MARKER_KEY: &str = "qc_history_fixture_version";
const SUGGESTION_MARKER_KEY: &str = "qc_suggestion_fixture_version";
const ACTION_SUGGESTION_MARKER_KEY: &str = "qc_action_suggestion_fixture_version";
const CLAIMS_MARKER_KEY: &str = "qc_claims_fixture_version";
const MARKER_IN_PROGRESS: &str = "seeding";
const MARKER_VERSION: &str = "1";
const SUGGESTION_MARKER_VERSION: &str = "1";
const CLAIMS_MARKER_VERSION: &str = "2";
const SUGGESTION_ID: &str = "qc-fixture-cj-03-07-pending-v1";
const ACTION_SUGGESTION_ID: &str = "qc-fixture-cj-03-09-action-v1";
const AUDIO_STATE_FILE: &str = "qc-audio-fixture-state.json";

pub(crate) fn command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(COMMAND)
}

pub(crate) fn suggestion_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(SUGGESTION_COMMAND)
}

pub(crate) fn action_suggestion_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(ACTION_SUGGESTION_COMMAND)
}

pub(crate) fn claims_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(CLAIMS_COMMAND)
}

pub(crate) fn audio_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(AUDIO_COMMAND)
}

fn is_isolated_flavor(flavor: &str) -> bool {
    let trimmed = flavor.trim();
    (trimmed.starts_with("qc-") || trimmed.starts_with("tc-"))
        && trimmed.len() > 3
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn require_exact_gate(name: &str) -> Result<()> {
    if !exact_gate_enabled(std::env::var(name).ok().as_deref()) {
        bail!("{name}=1 is required")
    }
    Ok(())
}

fn exact_gate_enabled(value: Option<&str>) -> bool {
    value == Some("1")
}

fn require_isolated_profile() -> Result<()> {
    require_exact_gate(DEBUG_GATE_ENV)?;
    require_exact_gate(ISOLATED_GATE_ENV)?;

    let flavor = std::env::var(FLAVOR_ENV).context("MAEKON_APP_FLAVOR is required")?;
    if !is_isolated_flavor(&flavor) {
        bail!("MAEKON_APP_FLAVOR must be a dedicated qc-* or tc-* flavor")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{Duration, Utc};
    use maekon_core::config::{
        AppConfig, CloudSttPolicy, ConfirmationRequirement, MicInputMode, PiiFilterLevel,
        SttProviderKind,
    };
    use maekon_core::models::memory_graph::{ClaimStatus, EdgeType};
    use maekon_core::models::suggestion::{SuggestionSource, SuggestionType};
    use maekon_core::ports::memory_graph_port::MemoryGraphPort;
    use maekon_storage::encryption::EncryptionKey;
    use maekon_storage::sqlite::SqliteStorage;
    use maekon_vision::privacy::sanitize_title_with_level;

    use crate::reauth::pin::{verify_pin, REAUTH_PIN_HASH_KEY};

    use super::audio::configure_audio_fixture;
    use super::claims::seed_claims_fixture;
    use super::history::{ensure_sanitized, fixture_frames, seed_fixture};
    use super::suggestions::{
        configure_action_suggestion_fixture, seed_action_suggestion_fixture,
        seed_suggestion_fixture,
    };
    use super::*;

    #[test]
    fn command_is_explicit() {
        assert!(command_requested([COMMAND].into_iter()));
        assert!(suggestion_command_requested(
            [SUGGESTION_COMMAND].into_iter()
        ));
        assert!(action_suggestion_command_requested(
            [ACTION_SUGGESTION_COMMAND].into_iter()
        ));
        assert!(claims_command_requested([CLAIMS_COMMAND].into_iter()));
        assert!(audio_command_requested([AUDIO_COMMAND].into_iter()));
        assert!(!command_requested(["--version"].into_iter()));
        assert!(!suggestion_command_requested([COMMAND].into_iter()));
        assert!(!action_suggestion_command_requested(
            [SUGGESTION_COMMAND].into_iter()
        ));
        assert!(!claims_command_requested([COMMAND].into_iter()));
        assert!(!audio_command_requested([COMMAND].into_iter()));
        assert!(!command_requested(std::iter::empty()));
    }

    #[test]
    fn gates_require_exact_opt_in() {
        assert!(exact_gate_enabled(Some("1")));
        assert!(!exact_gate_enabled(None));
        assert!(!exact_gate_enabled(Some("true")));
        assert!(!exact_gate_enabled(Some(" 1")));
    }

    #[test]
    fn action_fixture_exposes_cta_but_defaults_execution_to_blocked() {
        let mut config = AppConfig::default_config();
        configure_action_suggestion_fixture(&mut config);
        assert!(config.automation.enabled);
        assert_eq!(
            config.automation.confirmation_policy,
            ConfirmationRequirement::Block
        );
    }

    #[test]
    fn audio_fixture_is_synthetic_local_only_and_egress_closed() {
        let mut config = AppConfig::default_config();
        config.audio.cloud_api_key = "synthetic-secret-must-be-cleared".into();
        config.sync.enabled = true;
        config.integration.enabled = true;
        config.telemetry.enabled = true;
        config.web.allow_external = true;
        config.external_grpc.enabled = true;

        configure_audio_fixture(&mut config);

        assert!(config.audio.enabled);
        assert_eq!(config.audio.mic_input_mode, MicInputMode::VoiceActivity);
        assert_eq!(config.audio.stt_provider, SttProviderKind::Local);
        assert!(config.audio.cloud_api_key.is_empty());
        assert_eq!(config.audio.cloud_stt_policy, CloudSttPolicy::Disabled);
        assert!(!config.vision.capture_enabled);
        assert!(!config.sync.enabled);
        assert!(!config.integration.enabled);
        assert!(!config.telemetry.enabled);
        assert!(!config.web.allow_external);
        assert!(!config.external_grpc.enabled);
        assert!(!config.automation.enabled);
    }

    #[tokio::test]
    async fn claims_seeding_is_encrypted_local_and_idempotent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x86; 32]);
        let first = seed_claims_fixture(temp.path(), key.clone(), 30)
            .await
            .expect("first claims seed");
        assert_eq!(first.claims, 4);
        assert_eq!(first.segments, 3);
        assert_eq!(first.edges, 3);
        assert!(!first.already_seeded);

        let storage = Arc::new(
            SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
                .expect("reopen encrypted fixture database"),
        );
        let memory_graph: Arc<dyn MemoryGraphPort> = storage;
        assert_eq!(
            memory_graph
                .list_claims_by_status(ClaimStatus::Active)
                .await
                .expect("active claims")
                .len(),
            2
        );
        assert_eq!(
            memory_graph
                .prune_orphan_evidence_edges()
                .await
                .expect("fixture provenance must not be orphaned"),
            0
        );
        assert_eq!(
            memory_graph
                .edges_from("qc-cj04-04-procedural-active", Some(EdgeType::Evidence))
                .await
                .expect("procedural provenance after GC")
                .len(),
            2
        );
        assert_eq!(
            memory_graph
                .list_claims_by_status(ClaimStatus::Superseded)
                .await
                .expect("superseded claims")
                .len(),
            1
        );
        assert_eq!(
            memory_graph
                .list_claims_by_status(ClaimStatus::Retracted)
                .await
                .expect("retracted claims")
                .len(),
            1
        );
        drop(memory_graph);

        let second = seed_claims_fixture(temp.path(), key, 30)
            .await
            .expect("second claims seed");
        assert!(second.already_seeded);
        assert_eq!(second.claims, 0);
        assert_eq!(second.segments, 0);
        assert_eq!(second.edges, 0);
    }

    #[test]
    fn only_dedicated_sanitized_flavors_are_allowed() {
        assert!(is_isolated_flavor("qc-8324-history"));
        assert!(is_isolated_flavor("tc-history"));
        assert!(!is_isolated_flavor("dev"));
        assert!(!is_isolated_flavor("release"));
        assert!(!is_isolated_flavor("qc-../main"));
    }

    #[test]
    fn synthetic_decoys_are_redacted() {
        let raw = fixture_frames()[2].ocr_text;
        let sanitized = sanitize_title_with_level(raw, PiiFilterLevel::Standard);
        ensure_sanitized(raw, &sanitized).expect("fixture decoys must be redacted");
        assert!(!sanitized.contains("qa.user+8324@example.com"));
        assert!(!sanitized.contains("4111 1111 1111 1111"));
    }

    #[tokio::test]
    async fn seeding_is_encrypted_and_idempotent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x83; 32]);
        let first = seed_fixture(temp.path(), key.clone(), 30, 500, "2468")
            .await
            .expect("first seed");
        assert_eq!(first.frames, 8);
        assert_eq!(first.events, 8);
        assert!(!first.already_seeded);

        let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
            .expect("reopen encrypted fixture database");
        let stored_hash = storage
            .get_meta(REAUTH_PIN_HASH_KEY)
            .expect("PIN verifier marker");
        assert!(verify_pin("2468", &stored_hash));
        let frames = storage
            .get_frames(Utc::now() - Duration::hours(2), Utc::now(), 20)
            .expect("read seeded frames");
        assert_eq!(frames.len(), 8);
        assert!(frames.iter().all(|frame| {
            !frame.window_title.contains("qa.user+8324@example.com")
                && frame.ocr_text.as_deref().is_none_or(|ocr| {
                    !ocr.contains("qa.user+8324@example.com")
                        && !ocr.contains("4111 1111 1111 1111")
                })
        }));
        drop(storage);

        let second = seed_fixture(temp.path(), key, 30, 500, "9999")
            .await
            .expect("second seed");
        assert!(second.already_seeded);
        assert_eq!(second.frames, 0);
        assert_eq!(second.events, 0);
    }

    #[test]
    fn suggestion_seeding_is_encrypted_local_and_idempotent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x84; 32]);
        let first =
            seed_suggestion_fixture(temp.path(), key.clone(), 30).expect("first suggestion seed");
        assert_eq!(first.suggestions, 1);
        assert!(!first.already_seeded);

        let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
            .expect("reopen encrypted fixture database");
        let rows = storage
            .list_suggestions_by_state("pending", 10)
            .expect("read pending suggestions");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].suggestion_id, SUGGESTION_ID);
        let suggestion = rows[0]
            .clone()
            .try_into_suggestion()
            .expect("decode persisted suggestion");
        assert_eq!(suggestion.source, SuggestionSource::RuleBased);
        assert_eq!(suggestion.suggestion_type, SuggestionType::ProductivityTip);
        assert!(!suggestion.is_actionable);
        assert!(suggestion.context_scope.is_none());
        drop(storage);

        let second = seed_suggestion_fixture(temp.path(), key, 30).expect("second suggestion seed");
        assert!(second.already_seeded);
        assert_eq!(second.suggestions, 0);
    }

    #[test]
    fn action_suggestion_seeding_is_encrypted_local_and_idempotent() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x85; 32]);
        let first = seed_action_suggestion_fixture(temp.path(), key.clone(), 30)
            .expect("first action suggestion seed");
        assert_eq!(first.suggestions, 1);
        assert!(!first.already_seeded);

        let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
            .expect("reopen encrypted fixture database");
        let rows = storage
            .list_suggestions_by_state("pending", 10)
            .expect("read pending suggestions");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].suggestion_id, ACTION_SUGGESTION_ID);
        let suggestion = rows[0]
            .clone()
            .try_into_suggestion()
            .expect("decode persisted suggestion");
        assert_eq!(suggestion.source, SuggestionSource::RuleBased);
        assert_eq!(suggestion.suggestion_type, SuggestionType::NeedFocusTime);
        assert!(suggestion.is_actionable);
        assert!(suggestion.context_scope.is_none());
        drop(storage);

        let second = seed_action_suggestion_fixture(temp.path(), key, 30)
            .expect("second action suggestion seed");
        assert!(second.already_seeded);
        assert_eq!(second.suggestions, 0);
    }
}
