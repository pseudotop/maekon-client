//! Final revocable-authority check for periodic local analysis.

use std::sync::Arc;

use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::consent_manager::{ConsentGate, ConsentManagerPort};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PeriodicInvocationBlock {
    AnalysisDisabled,
    ConsentRequired(Vec<&'static str>),
    CapturePolicyBlocked,
}

pub(super) struct LiveAnalysisConfig {
    pub(super) config: Option<Arc<maekon_core::config::AppConfig>>,
    pub(super) enabled: bool,
}

/// Read one internally consistent live settings snapshot and resolve the
/// analysis switch, falling back to the spawn-time value only in compositions
/// without a `ConfigManager`.
pub(super) fn analysis_config_now(
    config_manager: Option<&ConfigManager>,
    fallback_analysis_enabled: bool,
) -> LiveAnalysisConfig {
    let config = config_manager.map(ConfigManager::snapshot);
    let enabled = config
        .as_ref()
        .map_or(fallback_analysis_enabled, |config| config.analysis.enabled);
    LiveAnalysisConfig { config, enabled }
}

/// Re-read every revocable authority at the final provider boundary. The
/// periodic loop awaits a synchronous server-coexistence read after its first
/// tick snapshot, so that earlier snapshot is not sufficient authorization.
pub(super) fn periodic_invocation_block(
    config_manager: Option<&ConfigManager>,
    fallback_analysis_enabled: bool,
    consent_manager: Option<&Arc<dyn ConsentManagerPort>>,
    capture_paused: bool,
) -> Option<PeriodicInvocationBlock> {
    let live_config = analysis_config_now(config_manager, fallback_analysis_enabled);
    let permissions = ConsentGate::from_ref(consent_manager).permissions_snapshot();
    let capture_permitted = live_config.config.as_ref().is_some_and(|cfg| {
        crate::scheduler::capture_permitted_now(cfg, &permissions, capture_paused)
    });
    classify_periodic_invocation(live_config.enabled, &permissions, capture_permitted)
}

fn classify_periodic_invocation(
    analysis_enabled: bool,
    permissions: &maekon_core::consent::ConsentPermissions,
    capture_permitted: bool,
) -> Option<PeriodicInvocationBlock> {
    if !analysis_enabled {
        return Some(PeriodicInvocationBlock::AnalysisDisabled);
    }
    let missing = crate::local_analysis_status::missing_local_analysis_permissions(permissions);
    if !missing.is_empty() {
        return Some(PeriodicInvocationBlock::ConsentRequired(missing));
    }
    if !capture_permitted {
        return Some(PeriodicInvocationBlock::CapturePolicyBlocked);
    }
    None
}

#[cfg(feature = "local-suggestions")]
pub(super) async fn record_periodic_invocation_block(
    manager: Option<&Arc<crate::suggestion_manager::SuggestionManager>>,
    block: PeriodicInvocationBlock,
) {
    use crate::local_analysis_status::{
        LocalAnalysisProducer, LocalAnalysisStatus, LocalAnalysisStatusKind,
    };

    let Some(manager) = manager else {
        return;
    };
    match block {
        PeriodicInvocationBlock::AnalysisDisabled => {
            crate::local_analysis_status::record_periodic_status(
                Some(manager),
                LocalAnalysisStatusKind::PolicyBlocked,
                "analysis_disabled",
                0,
            )
            .await;
        }
        PeriodicInvocationBlock::ConsentRequired(missing) => {
            let queue_count = manager.queue().lock().await.len();
            manager
                .record_local_analysis(LocalAnalysisStatus::consent_required(
                    LocalAnalysisProducer::Periodic,
                    missing,
                    queue_count,
                ))
                .await;
        }
        PeriodicInvocationBlock::CapturePolicyBlocked => {
            crate::local_analysis_status::record_periodic_status(
                Some(manager),
                LocalAnalysisStatusKind::PolicyBlocked,
                "capture_policy_blocked",
                0,
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_gate_observes_consent_withdrawn_after_the_tick_snapshot() {
        let initial = maekon_core::consent::ConsentPermissions {
            screen_capture: true,
            ocr_processing: true,
            activity_pattern_learning: true,
            ..Default::default()
        };
        assert_eq!(classify_periodic_invocation(true, &initial, true), None);

        let mut withdrawn = initial;
        withdrawn.ocr_processing = false;
        assert!(matches!(
            classify_periodic_invocation(true, &withdrawn, true),
            Some(PeriodicInvocationBlock::ConsentRequired(missing))
                if missing == vec!["ocr_processing"]
        ));
    }

    #[test]
    fn live_analysis_switch_overrides_stale_fallback() {
        let fixture_dir = tempfile::tempdir().expect("config tempdir");
        let manager = ConfigManager::with_paths(fixture_dir.path().join("config.json"), None)
            .expect("create isolated config");
        manager
            .update_with(|config| {
                config.analysis.enabled = false;
                Ok(())
            })
            .expect("disable analysis");
        assert!(!analysis_config_now(Some(&manager), true).enabled);

        manager
            .update_with(|config| {
                config.analysis.enabled = true;
                Ok(())
            })
            .expect("enable analysis");
        assert!(analysis_config_now(Some(&manager), false).enabled);
    }
}
