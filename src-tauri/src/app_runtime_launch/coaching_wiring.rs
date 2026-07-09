use maekon_core::config::AppConfig;
use maekon_core::ports::coaching_effectiveness_store::CoachingEffectivenessStore;
use maekon_core::ports::coaching_storage::CoachingStoragePort;
use maekon_storage::sqlite::SqliteStorage;
use std::sync::Arc;
use tokio::runtime::Handle;

/// Builds the PII-sanitized coaching engine plus the shared `CoachingStoragePort`
/// handle the agent runtime consumes, hydrating learned effectiveness before the
/// coaching loops start. Behavior mirrors the previous inline composition-root
/// wiring exactly (#7913 T2.1b) — this is a pure extraction of that section.
pub(super) fn build_coaching_wiring(
    config: &AppConfig,
    handle: &Handle,
    sqlite_storage: Arc<SqliteStorage>,
) -> (
    Arc<maekon_analysis::CoachingEngine>,
    Arc<dyn CoachingStoragePort>,
) {
    // Keep coaching template interpolation behind the same PII sanitizer as
    // other surfaces.
    let coaching_engine =
        Arc::new(
            maekon_analysis::CoachingEngine::new(config.coaching.clone())
                .with_pii_sanitizer(
                    Arc::new(maekon_vision::privacy::VisionPiiSanitizer),
                    config.privacy.pii_filter_level,
                )
                // #7913 T2.1b: back the coaching effectiveness learning state with
                // the pre-existing `coaching_effectiveness` table (schema-only until
                // now) so learned (profile, trigger) effectiveness survives restart.
                .with_effectiveness_store(
                    sqlite_storage.clone() as Arc<dyn CoachingEffectivenessStore>
                ),
        );
    // Hydrate prior effectiveness before the coaching loops start. Fail-safe:
    // a missing/corrupt store starts empty and only warns (never panics).
    handle.block_on(coaching_engine.hydrate_effectiveness_from_store());

    let coaching_storage: Arc<dyn CoachingStoragePort> = sqlite_storage;

    (coaching_engine, coaching_storage)
}
