use chrono::{DateTime, Utc};
use maekon_core::models::tiered_memory::Regime;
use parking_lot::RwLock;

/// Snapshot of current regime state, shared between monitor and the
/// suggestion/coaching loops. Monitor loop writes (~1/sec), reader loops
/// read (~1/30s and on analysis ticks).
/// parking_lot::RwLock chosen over tokio::sync::RwLock: no .await needed
/// for <1μs read/write operations. Already a workspace dependency.
#[derive(Debug, Clone)]
pub struct RegimeSnapshot {
    pub regime_id: Option<String>,
    pub regime_label: Option<String>,
    /// E20-26 (#4818): the full owned `Regime` for the current tick, when one
    /// is active. Carried alongside the id/label strings so regime-aware
    /// consumers (e.g. `filter_by_regime` in the local-suggestion enqueue
    /// path) can read `name`/`auto_label` without re-resolving the manager.
    /// `None` when no regime is classified yet — consumers must pass through.
    pub regime: Option<Regime>,
    pub current_app: String,
    pub updated_at: DateTime<Utc>,
}

pub struct SharedRegimeState {
    inner: RwLock<RegimeSnapshot>,
}

impl SharedRegimeState {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(RegimeSnapshot {
                regime_id: None,
                regime_label: None,
                regime: None,
                current_app: String::new(),
                updated_at: Utc::now(),
            }),
        }
    }

    /// Called by monitor loop after regime classification each tick.
    ///
    /// E20-26 (#4818): `regime` carries the full owned `Regime` (cloned from the
    /// regime manager) so regime-aware consumers get `name`/`auto_label` without
    /// re-locking the manager. `regime_id`/`regime_label` are derived from it to
    /// keep the existing string readers (coaching loop) consistent.
    pub fn update(&self, regime: Option<&Regime>, app: &str) {
        let mut guard = self.inner.write();
        guard.regime_id = regime.map(|r| r.regime_id.clone());
        guard.regime_label = regime.map(|r| r.name.clone().unwrap_or_else(|| r.auto_label.clone()));
        guard.regime = regime.cloned();
        guard.current_app = app.to_string();
        guard.updated_at = Utc::now();
    }

    /// Called by coaching loop to get current regime context.
    pub fn snapshot(&self) -> RegimeSnapshot {
        self.inner.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::tiered_memory::{RegimeFeatures, RegimeStatus, TriggerParams};

    fn make_regime(id: &str, name: Option<&str>, auto_label: &str) -> Regime {
        Regime {
            regime_id: id.to_string(),
            name: name.map(|s| s.to_string()),
            auto_label: auto_label.to_string(),
            centroid: RegimeFeatures::default(),
            optimal_params: TriggerParams::default(),
            sample_count: 100,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status: RegimeStatus::Active,
        }
    }

    #[test]
    fn default_snapshot_has_no_regime() {
        let state = SharedRegimeState::new();
        let snap = state.snapshot();
        assert!(snap.regime_id.is_none());
        assert!(snap.regime_label.is_none());
        assert!(snap.regime.is_none());
        assert!(snap.current_app.is_empty());
    }

    #[test]
    fn update_then_snapshot_returns_latest() {
        let state = SharedRegimeState::new();
        let regime = make_regime("regime-1", Some("Deep Work"), "auto-deep");
        state.update(Some(&regime), "VSCode");
        let snap = state.snapshot();
        assert_eq!(snap.regime_id.as_deref(), Some("regime-1"));
        // name preferred over auto_label for the label string.
        assert_eq!(snap.regime_label.as_deref(), Some("Deep Work"));
        assert_eq!(
            snap.regime.as_ref().map(|r| r.regime_id.as_str()),
            Some("regime-1")
        );
        assert_eq!(snap.current_app, "VSCode");
    }

    #[test]
    fn update_label_falls_back_to_auto_label() {
        let state = SharedRegimeState::new();
        let regime = make_regime("r-auto", None, "Deep Focus (VSCode)");
        state.update(Some(&regime), "VSCode");
        let snap = state.snapshot();
        assert_eq!(snap.regime_label.as_deref(), Some("Deep Focus (VSCode)"));
    }

    #[test]
    fn update_clears_previous_values_when_none() {
        let state = SharedRegimeState::new();
        let regime = make_regime("r1", Some("label"), "auto");
        state.update(Some(&regime), "App");
        state.update(None, "");
        let snap = state.snapshot();
        assert!(snap.regime_id.is_none());
        assert!(snap.regime_label.is_none());
        assert!(snap.regime.is_none());
    }

    // -----------------------------------------------------------------------
    // E20-26 (#4818): regime/context-aware local-suggestion gating — wiring test.
    //
    // This exercises the exact path the local-suggestion PRODUCERS use
    // (intelligence.rs `spawn_analysis_loop` + helpers/analysis.rs
    // `handle_event_analysis`): read the current `Regime` from the shared state
    // snapshot, run `maekon_analysis::filter_by_regime` over the produced
    // suggestions, then enqueue the survivors. We assert that in a "Deep Focus"
    // regime Low/Medium-priority suggestions never reach the queue while
    // High/Critical do, and that with no regime everything passes through.
    // -----------------------------------------------------------------------
    mod regime_gated_enqueue {
        use super::*;
        use chrono::Utc;
        use maekon_core::id_generation::generate_id;
        use maekon_core::models::suggestion::{
            Priority, Suggestion, SuggestionSource, SuggestionType,
        };
        use maekon_suggestion::queue::SuggestionQueue;

        fn make_suggestion(priority: Priority) -> Suggestion {
            // Unique content per suggestion so the queue's content-fingerprint
            // dedup does not collapse our distinct test items.
            let id = generate_id("sug");
            Suggestion {
                suggestion_id: id.clone(),
                suggestion_type: SuggestionType::ProductivityTip,
                content: format!("Test suggestion {id}"),
                priority,
                confidence_score: 0.8,
                relevance_score: 0.7,
                is_actionable: true,
                created_at: Utc::now(),
                expires_at: None,
                source: SuggestionSource::RuleBased,
                reasoning: None,
                context_scope: None,
            }
        }

        /// Mirror of the producer enqueue step: filter by the shared regime,
        /// then push survivors into the queue.
        fn enqueue_with_regime_gate(
            shared: &SharedRegimeState,
            mut suggestions: Vec<Suggestion>,
            queue: &mut SuggestionQueue,
        ) {
            let regime = shared.snapshot().regime;
            maekon_analysis::filter_by_regime(&mut suggestions, regime.as_ref());
            for s in suggestions {
                queue.push(s);
            }
        }

        #[test]
        fn deep_focus_regime_drops_low_medium_before_enqueue() {
            let shared = SharedRegimeState::new();
            // auto_label starting with "Deep Focus" triggers High+ retention.
            let regime = make_regime("r-focus", None, "Deep Focus (VSCode)");
            shared.update(Some(&regime), "VSCode");

            let mut queue = SuggestionQueue::new(64);
            enqueue_with_regime_gate(
                &shared,
                vec![
                    make_suggestion(Priority::Low),
                    make_suggestion(Priority::Medium),
                    make_suggestion(Priority::High),
                    make_suggestion(Priority::Critical),
                ],
                &mut queue,
            );

            // Only High + Critical survive into the queue.
            assert_eq!(queue.len(), 2, "deep-focus must drop Low/Medium at enqueue");
            assert!(
                queue.iter().all(|s| s.priority >= Priority::High),
                "no sub-High suggestion may reach the queue in deep focus"
            );
        }

        #[test]
        fn non_focus_regime_passes_everything_through() {
            let shared = SharedRegimeState::new();
            let regime = make_regime("r-comm", None, "Communication (Slack)");
            shared.update(Some(&regime), "Slack");

            let mut queue = SuggestionQueue::new(64);
            enqueue_with_regime_gate(
                &shared,
                vec![
                    make_suggestion(Priority::Low),
                    make_suggestion(Priority::Medium),
                    make_suggestion(Priority::High),
                ],
                &mut queue,
            );

            assert_eq!(queue.len(), 3, "non-focus regime must not filter");
        }

        #[test]
        fn no_regime_passes_everything_through() {
            // Default state has `regime: None` => filter is a pass-through.
            let shared = SharedRegimeState::new();

            let mut queue = SuggestionQueue::new(64);
            enqueue_with_regime_gate(
                &shared,
                vec![
                    make_suggestion(Priority::Low),
                    make_suggestion(Priority::Medium),
                ],
                &mut queue,
            );

            assert_eq!(queue.len(), 2, "no regime => no filtering");
        }
    }
}
