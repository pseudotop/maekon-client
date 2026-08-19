use maekon_core::config::AppConfig;
use maekon_core::ports::adaptive_scorer_store::AdaptiveScorerStore;
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
    let coaching_engine = Arc::new(
        maekon_analysis::CoachingEngine::new(config.coaching.clone())
            .with_pii_sanitizer(
                Arc::new(maekon_vision::privacy::VisionPiiSanitizer),
                config.privacy.pii_filter_level,
            )
            // #7913 T2.1b: back the coaching effectiveness learning state with
            // the pre-existing `coaching_effectiveness` table (schema-only until
            // now) so learned (profile, trigger) effectiveness survives restart.
            .with_effectiveness_store(sqlite_storage.clone() as Arc<dyn CoachingEffectivenessStore>)
            // #8058 P2-1: back the adaptive-scorer weights with the
            // `adaptive_scorer_state` singleton (V46) so the online-logistic-
            // regression model warm-up survives restart — previously the LAST
            // feedback-learning component that reset on every exit.
            .with_adaptive_scorer_store(sqlite_storage.clone() as Arc<dyn AdaptiveScorerStore>),
    );
    // Hydrate prior effectiveness before the coaching loops start. Fail-safe:
    // a missing/corrupt store starts empty and only warns (never panics).
    handle.block_on(coaching_engine.hydrate_effectiveness_from_store());
    // #8058 P2-1: hydrate the persisted adaptive-scorer weights too, so a restart
    // mid-warm-up keeps its accumulated training toward is_ready(). Same fail-safe
    // contract: a missing/corrupt/stale-shaped row starts from the default model.
    handle.block_on(coaching_engine.hydrate_adaptive_scorer_from_store());

    // #8052: hydrate today's goal progress from the persisted `habit_streaks`
    // rows so a restart of this 24/7 daemon does not reset same-day minutes
    // (goal widget under-reports) or re-fire the 25/50/75/100% coaching nudges
    // already delivered earlier today. Rows are keyed by LOCAL date (the
    // coaching loop writes `Local::now()`), so a small day window is read and
    // filtered to today's local date in Rust — the SQL `date()` filter is UTC
    // and would drift across the local/UTC boundary near midnight. Fail-safe: a
    // read error only warns and leaves goal progress fresh (never panics).
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    match sqlite_storage.query_habit_streaks(2) {
        Ok(rows) => {
            let today_minutes: Vec<(String, u32)> = rows
                .into_iter()
                .filter(|row| row.date == today)
                .map(|row| (row.regime_label, row.minutes_logged))
                .collect();
            if !today_minutes.is_empty() {
                handle.block_on(coaching_engine.hydrate_goal_progress(today_minutes));
            }
        }
        Err(e) => tracing::warn!(
            error = %e,
            "habit-streak goal hydration read failed; goal progress starts fresh"
        ),
    }

    let coaching_storage: Arc<dyn CoachingStoragePort> = sqlite_storage;

    (coaching_engine, coaching_storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::AppConfig;

    /// #8052 end-to-end: `build_coaching_wiring` must seed today's goal progress
    /// from the persisted `habit_streaks` so a restart does not under-report
    /// same-day minutes. A row of 80/100 min is written for today (as the
    /// coaching loop would have before a restart); the freshly-built engine must
    /// surface 80 min (not a reset 0) at the composition-root seam.
    ///
    /// Runs on a plain thread with an owned runtime so `build_coaching_wiring`'s
    /// internal `handle.block_on` is not nested inside a runtime worker.
    #[test]
    fn build_coaching_wiring_hydrates_today_goal_progress() {
        const LABEL: &str = "Deep Work";
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");

        let sqlite_storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        // "Before restart": 80 of 100 min logged today, target not yet met.
        sqlite_storage
            .upsert_habit_streak(LABEL, &today, 80, 100, false)
            .expect("seed habit row");

        let mut config = AppConfig::default_config();
        config.coaching.regime_goals.insert(LABEL.to_string(), 100);

        let (engine, _storage) =
            build_coaching_wiring(&config, runtime.handle(), sqlite_storage.clone());

        // Hydrated: the widget-facing progress reads 80 min, not a reset 0.
        let progress = runtime.block_on(engine.all_goal_progress());
        let deep = progress
            .iter()
            .find(|g| g.regime_label == LABEL)
            .expect("configured goal present");
        assert_eq!(deep.current_minutes, 80, "restart must not under-report");
        assert_eq!(deep.percentage, 80);
    }
}
