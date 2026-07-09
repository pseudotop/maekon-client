use chrono::{DateTime, Utc};
use maekon_core::models::suggestion::{
    FeedbackTallyRecord, FeedbackType, SuggestionSource, SuggestionType,
};
use std::collections::HashMap;

const MIN_SAMPLES: u32 = 5;
const HIGH_REJECTION_THRESHOLD: f64 = 0.7;
const MEDIUM_REJECTION_THRESHOLD: f64 = 0.5;
const HIGH_ACCEPTANCE_THRESHOLD: f64 = 0.7;
const HEAVY_PENALTY: f64 = -0.3;
const LIGHT_PENALTY: f64 = -0.15;
const ACCEPTANCE_BOOST: f64 = 0.1;
const SUPPRESSION_THRESHOLD: f64 = 0.2;

/// Sliding-window length (hours) for the scorer's self-decay: a `(type, source)`
/// tally older than this reads as neutral (score 0.0) and is reset on the next
/// `record`. Rationale: relevance preferences drift over a work session, so a
/// day-old rejection streak should not keep suppressing suggestions forever; 12h
/// ≈ one working day, letting yesterday's mood fade by the next morning.
///
/// #7913 T2.1c made this WALL-CLOCK-anchored across restarts: `last_updated` is
/// persisted with the tally, so a daily-restart user neither loses state (the old
/// RAM-only bug) nor escapes decay (a naive "reset last_updated on load" bug).
/// Do NOT silently change this length — coaching/relevance product behavior
/// already shifted once this week (#7930); a change needs its own decision.
const TALLY_DECAY_HOURS: i64 = 12;

struct FeedbackTally {
    accepted: u32,
    rejected: u32,
    deferred: u32,
    last_updated: DateTime<Utc>,
}

impl FeedbackTally {
    fn new() -> Self {
        Self {
            accepted: 0,
            rejected: 0,
            deferred: 0,
            last_updated: Utc::now(),
        }
    }

    fn total(&self) -> u32 {
        self.accepted + self.rejected + self.deferred
    }

    fn is_stale(&self) -> bool {
        Utc::now()
            .signed_duration_since(self.last_updated)
            .num_hours()
            > TALLY_DECAY_HOURS
    }
}

pub struct FeedbackScorer {
    tallies: HashMap<(SuggestionType, SuggestionSource), FeedbackTally>,
}

impl FeedbackScorer {
    pub fn new() -> Self {
        Self {
            tallies: HashMap::new(),
        }
    }

    /// Record a feedback event for a suggestion type+source pair.
    pub fn record(
        &mut self,
        suggestion_type: SuggestionType,
        source: SuggestionSource,
        feedback: &FeedbackType,
    ) {
        let tally = self
            .tallies
            .entry((suggestion_type, source))
            .or_insert_with(FeedbackTally::new);

        if tally.is_stale() {
            *tally = FeedbackTally::new();
        }

        match feedback {
            FeedbackType::Accepted => tally.accepted += 1,
            FeedbackType::Rejected => tally.rejected += 1,
            FeedbackType::Deferred => tally.deferred += 1,
        }
        tally.last_updated = Utc::now();
    }

    /// Compute a relevance boost/penalty for a suggestion type+source pair.
    /// Returns a value in [-0.3, +0.1] or 0.0 if insufficient data.
    pub fn score(&self, suggestion_type: &SuggestionType, source: &SuggestionSource) -> f64 {
        let Some(tally) = self.tallies.get(&(suggestion_type.clone(), source.clone())) else {
            return 0.0;
        };

        if tally.is_stale() || tally.total() < MIN_SAMPLES {
            return 0.0;
        }

        let total = f64::from(tally.total());
        let rejection_ratio = f64::from(tally.rejected) / total;
        let acceptance_ratio = f64::from(tally.accepted) / total;

        if rejection_ratio > HIGH_REJECTION_THRESHOLD {
            HEAVY_PENALTY
        } else if rejection_ratio > MEDIUM_REJECTION_THRESHOLD {
            LIGHT_PENALTY
        } else if acceptance_ratio > HIGH_ACCEPTANCE_THRESHOLD {
            ACCEPTANCE_BOOST
        } else {
            0.0
        }
    }

    /// Adjust a suggestion's relevance_score in-place. Returns true if the
    /// suggestion should be queued, false if suppressed (relevance < threshold).
    pub fn adjust(
        &self,
        suggestion_type: &SuggestionType,
        source: &SuggestionSource,
        relevance: &mut f64,
    ) -> bool {
        let boost = self.score(suggestion_type, source);
        *relevance = (*relevance + boost).clamp(0.0, 1.0);
        *relevance >= SUPPRESSION_THRESHOLD
    }

    /// Snapshot the tally for one `(type, source)` as a persistable record for
    /// write-through after `record` (#7913 T2.1c). `None` when no tally exists.
    pub fn tally_record(
        &self,
        suggestion_type: &SuggestionType,
        source: &SuggestionSource,
    ) -> Option<FeedbackTallyRecord> {
        let tally = self
            .tallies
            .get(&(suggestion_type.clone(), source.clone()))?;
        Some(FeedbackTallyRecord {
            suggestion_type: suggestion_type.clone(),
            source: source.clone(),
            accepted: tally.accepted,
            rejected: tally.rejected,
            deferred: tally.deferred,
            last_updated: tally.last_updated,
        })
    }

    /// Snapshot every tally as persistable records (#7913 T2.1c).
    pub fn snapshot(&self) -> Vec<FeedbackTallyRecord> {
        self.tallies
            .iter()
            .map(|((suggestion_type, source), tally)| FeedbackTallyRecord {
                suggestion_type: suggestion_type.clone(),
                source: source.clone(),
                accepted: tally.accepted,
                rejected: tally.rejected,
                deferred: tally.deferred,
                last_updated: tally.last_updated,
            })
            .collect()
    }

    /// Load persisted tallies on startup (#7913 T2.1c), PRESERVING each
    /// `last_updated` so the 12h self-decay stays wall-clock-anchored: a tally
    /// that aged past `TALLY_DECAY_HOURS` while the process was down reads as
    /// decayed (score 0.0) via the existing `is_stale` path, and is reset on its
    /// next `record` — the decay clock is NOT restarted by the load.
    pub fn hydrate(&mut self, records: Vec<FeedbackTallyRecord>) {
        for r in records {
            self.tallies.insert(
                (r.suggestion_type, r.source),
                FeedbackTally {
                    accepted: r.accepted,
                    rejected: r.rejected,
                    deferred: r.deferred,
                    last_updated: r.last_updated,
                },
            );
        }
    }
}

impl Default for FeedbackScorer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insufficient_data_returns_zero() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..3 {
            scorer.record(
                SuggestionType::WorkGuidance,
                SuggestionSource::LlmServer,
                &FeedbackType::Rejected,
            );
        }
        let score = scorer.score(&SuggestionType::WorkGuidance, &SuggestionSource::LlmServer);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn high_rejection_applies_heavy_penalty() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..8 {
            scorer.record(
                SuggestionType::EmailDraft,
                SuggestionSource::RuleBased,
                &FeedbackType::Rejected,
            );
        }
        for _ in 0..2 {
            scorer.record(
                SuggestionType::EmailDraft,
                SuggestionSource::RuleBased,
                &FeedbackType::Accepted,
            );
        }
        let score = scorer.score(&SuggestionType::EmailDraft, &SuggestionSource::RuleBased);
        assert!((score - HEAVY_PENALTY).abs() < f64::EPSILON);
    }

    #[test]
    fn medium_rejection_applies_light_penalty() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..6 {
            scorer.record(
                SuggestionType::ProductivityTip,
                SuggestionSource::LlmLocal,
                &FeedbackType::Rejected,
            );
        }
        for _ in 0..4 {
            scorer.record(
                SuggestionType::ProductivityTip,
                SuggestionSource::LlmLocal,
                &FeedbackType::Accepted,
            );
        }
        let score = scorer.score(
            &SuggestionType::ProductivityTip,
            &SuggestionSource::LlmLocal,
        );
        assert!((score - LIGHT_PENALTY).abs() < f64::EPSILON);
    }

    #[test]
    fn high_acceptance_applies_boost() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..8 {
            scorer.record(
                SuggestionType::ContextBased,
                SuggestionSource::LlmServer,
                &FeedbackType::Accepted,
            );
        }
        for _ in 0..2 {
            scorer.record(
                SuggestionType::ContextBased,
                SuggestionSource::LlmServer,
                &FeedbackType::Rejected,
            );
        }
        let score = scorer.score(&SuggestionType::ContextBased, &SuggestionSource::LlmServer);
        assert!((score - ACCEPTANCE_BOOST).abs() < f64::EPSILON);
    }

    #[test]
    fn neutral_ratio_returns_zero() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..3 {
            scorer.record(
                SuggestionType::WorkGuidance,
                SuggestionSource::LlmServer,
                &FeedbackType::Rejected,
            );
        }
        for _ in 0..3 {
            scorer.record(
                SuggestionType::WorkGuidance,
                SuggestionSource::LlmServer,
                &FeedbackType::Accepted,
            );
        }
        for _ in 0..4 {
            scorer.record(
                SuggestionType::WorkGuidance,
                SuggestionSource::LlmServer,
                &FeedbackType::Deferred,
            );
        }
        let score = scorer.score(&SuggestionType::WorkGuidance, &SuggestionSource::LlmServer);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_suppresses_low_relevance() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..10 {
            scorer.record(
                SuggestionType::WorkGuidance,
                SuggestionSource::LlmServer,
                &FeedbackType::Rejected,
            );
        }
        let mut relevance = 0.4;
        let should_queue = scorer.adjust(
            &SuggestionType::WorkGuidance,
            &SuggestionSource::LlmServer,
            &mut relevance,
        );
        assert!(!should_queue);
        assert!((relevance - 0.1).abs() < f64::EPSILON);
    }

    #[test]
    fn adjust_allows_high_relevance() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..10 {
            scorer.record(
                SuggestionType::WorkGuidance,
                SuggestionSource::LlmServer,
                &FeedbackType::Rejected,
            );
        }
        let mut relevance = 0.9;
        let should_queue = scorer.adjust(
            &SuggestionType::WorkGuidance,
            &SuggestionSource::LlmServer,
            &mut relevance,
        );
        assert!(should_queue);
        assert!((relevance - 0.6).abs() < f64::EPSILON);
    }

    #[test]
    fn decay_resets_stale_tallies() {
        let mut scorer = FeedbackScorer::new();
        scorer.record(
            SuggestionType::EmailDraft,
            SuggestionSource::RuleBased,
            &FeedbackType::Rejected,
        );
        if let Some(tally) = scorer
            .tallies
            .get_mut(&(SuggestionType::EmailDraft, SuggestionSource::RuleBased))
        {
            tally.rejected = 10;
            tally.last_updated = Utc::now() - chrono::Duration::hours(13);
        }
        let score = scorer.score(&SuggestionType::EmailDraft, &SuggestionSource::RuleBased);
        assert!((score - 0.0).abs() < f64::EPSILON);

        scorer.record(
            SuggestionType::EmailDraft,
            SuggestionSource::RuleBased,
            &FeedbackType::Rejected,
        );
        let score = scorer.score(&SuggestionType::EmailDraft, &SuggestionSource::RuleBased);
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn unknown_type_source_returns_zero() {
        let scorer = FeedbackScorer::new();
        let score = scorer.score(
            &SuggestionType::WorkflowOptimization,
            &SuggestionSource::LlmLocal,
        );
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    /// #7913 T2.1c — snapshot/hydrate round-trips a live tally so the learned
    /// penalty survives a restart (a fresh scorer hydrated from the snapshot
    /// reproduces the same score).
    #[test]
    fn snapshot_and_hydrate_roundtrip_preserves_score() {
        let mut scorer = FeedbackScorer::new();
        for _ in 0..10 {
            scorer.record(
                SuggestionType::EmailDraft,
                SuggestionSource::RuleBased,
                &FeedbackType::Rejected,
            );
        }
        let before = scorer.score(&SuggestionType::EmailDraft, &SuggestionSource::RuleBased);
        assert!((before - HEAVY_PENALTY).abs() < f64::EPSILON);

        let snapshot = scorer.snapshot();
        assert_eq!(snapshot.len(), 1);

        // A fresh scorer hydrated from the snapshot reproduces the same score.
        let mut restored = FeedbackScorer::new();
        restored.hydrate(snapshot);
        let after = restored.score(&SuggestionType::EmailDraft, &SuggestionSource::RuleBased);
        assert!((after - HEAVY_PENALTY).abs() < f64::EPSILON);
    }

    /// #7913 T2.1c — DECAY on load: a persisted tally whose `last_updated` is
    /// older than `TALLY_DECAY_HOURS` reads as decayed (score 0.0) after hydrate,
    /// because the wall-clock anchor is preserved rather than reset. This is the
    /// "daily-restart user does not escape decay" property.
    #[test]
    fn hydrate_applies_wall_clock_decay_for_stale_last_updated() {
        // A record with strong rejections but a last_updated 13h in the past
        // (> TALLY_DECAY_HOURS = 12) — should read as decayed on load.
        let stale = FeedbackTallyRecord {
            suggestion_type: SuggestionType::WorkGuidance,
            source: SuggestionSource::LlmServer,
            accepted: 0,
            rejected: 10,
            deferred: 0,
            last_updated: Utc::now() - chrono::Duration::hours(13),
        };
        let mut scorer = FeedbackScorer::new();
        scorer.hydrate(vec![stale]);

        let score = scorer.score(&SuggestionType::WorkGuidance, &SuggestionSource::LlmServer);
        assert!(
            (score - 0.0).abs() < f64::EPSILON,
            "a tally aged past the decay window must read as neutral on load, got {score}"
        );

        // Sanity: an otherwise-identical FRESH tally (last_updated now) is NOT
        // decayed — proving the decay is time-driven, not blanket-zeroed on load.
        let fresh = FeedbackTallyRecord {
            suggestion_type: SuggestionType::WorkGuidance,
            source: SuggestionSource::LlmServer,
            accepted: 0,
            rejected: 10,
            deferred: 0,
            last_updated: Utc::now(),
        };
        let mut scorer2 = FeedbackScorer::new();
        scorer2.hydrate(vec![fresh]);
        let score2 = scorer2.score(&SuggestionType::WorkGuidance, &SuggestionSource::LlmServer);
        assert!(
            (score2 - HEAVY_PENALTY).abs() < f64::EPSILON,
            "a fresh tally must retain its learned penalty, got {score2}"
        );
    }
}
