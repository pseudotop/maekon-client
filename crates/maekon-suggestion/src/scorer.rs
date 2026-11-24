use chrono::{DateTime, Utc};
use maekon_core::models::suggestion::{
    FeedbackTallyRecord, FeedbackType, SuggestionShadowBaselineSummary,
    SuggestionShadowOpportunity, SuggestionShadowValidationError, SuggestionSource, SuggestionType,
};
use maekon_core::models::suggestion_shadow_capture::{
    SuggestionShadowBatchError, SuggestionShadowCaptureBinding, SuggestionShadowCaptureReceipt,
    SuggestionShadowCaptureSummary,
};
use std::collections::{BTreeMap, HashMap, HashSet};

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

/// Summarize complete caller-supplied opportunities without changing scoring or admission.
/// No runtime collector, provider/model effect, UI exposure, or GO evidence is supplied.
/// Unknown observations remain separate from observed rejection and explicit false labels.
pub fn summarize_shadow_baseline(
    opportunities: &[SuggestionShadowOpportunity],
) -> Result<SuggestionShadowBaselineSummary, SuggestionShadowValidationError> {
    SuggestionShadowOpportunity::validate_population(opportunities)?;
    let mut summary = SuggestionShadowBaselineSummary {
        rubric_revision: opportunities[0].rubric_revision.clone(),
        opportunity_count: opportunities.len(),
        ..Default::default()
    };
    for row in opportunities {
        let generated = row.generated_candidate_ids.len();
        summary.generated_count += generated;
        let admitted = row
            .baseline_admitted_candidate_ids
            .as_ref()
            .map(|ids| ids.iter().collect::<HashSet<_>>());
        match &admitted {
            Some(ids) => {
                summary.admitted_count += ids.len();
                summary.not_admitted_count += generated - ids.len();
            }
            None => summary.admission_unknown_count += generated,
        }
        let Some(labels) = &row.independent_usefulness else {
            summary.oracle_unknown_count += generated;
            continue;
        };
        summary.oracle_labelled_count += generated;
        for (id, useful) in labels {
            if *useful {
                summary.useful_count += 1;
                match &admitted {
                    Some(ids) if ids.contains(id) => summary.useful_admitted_count += 1,
                    Some(_) => summary.useful_not_admitted_count += 1,
                    None => summary.useful_admission_unknown_count += 1,
                }
            }
        }
    }
    Ok(summary)
}

/// Summarize every declared receipt after current-binding and population validation.
/// Subtotals cover observed original generation only; missing observations stay unknown.
/// Caller-supplied data does not prove actual population, oracle or billing authority,
/// consent, additional Jev costs, savings, exposure, adoption, or an experiment verdict.
pub fn summarize_shadow_capture(
    receipts: &[SuggestionShadowCaptureReceipt],
    expected: &BTreeMap<String, SuggestionShadowCaptureBinding>,
) -> Result<SuggestionShadowCaptureSummary, SuggestionShadowBatchError> {
    let population = SuggestionShadowCaptureReceipt::validated_population(receipts, expected)?;
    let mut summary = SuggestionShadowCaptureSummary {
        protocol_revision: receipts[0].binding().protocol_revision.clone(),
        baseline: summarize_shadow_baseline(&population)?,
        ..Default::default()
    };
    for receipt in receipts {
        let generation = receipt.generation();
        let usage = generation.usage.as_ref();
        summary.elapsed_ms.add_observation(generation.elapsed_ms)?;
        summary
            .input_tokens
            .add_observation(usage.map(|u| u.input_tokens))?;
        summary
            .output_tokens
            .add_observation(usage.map(|u| u.output_tokens))?;
        match &generation.paid_cost {
            Some(cost) => {
                let subtotal = summary
                    .paid_cost
                    .subtotals_minor
                    .entry(cost.currency.clone())
                    .or_default();
                *subtotal = subtotal
                    .checked_add(cost.amount_minor)
                    .ok_or(SuggestionShadowBatchError::ObservationOverflow)?;
                summary.paid_cost.observed_opportunities += 1;
            }
            None => summary.paid_cost.unknown_opportunities += 1,
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::candidate_decision::DecisionUsage;
    use maekon_core::models::suggestion_shadow_capture::{
        SuggestionShadowAdmissionObservation, SuggestionShadowCaptureBinding as Binding,
        SuggestionShadowCaptureError, SuggestionShadowCaptureReceipt as Receipt,
        SuggestionShadowGenerationObservation as Generation, SuggestionShadowObservedCost,
        SuggestionShadowObservedCostSummary as CostSummary, SuggestionShadowObservedTotal,
        SuggestionShadowRejectionReason,
    };

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

    fn shadow_opportunity() -> SuggestionShadowOpportunity {
        SuggestionShadowOpportunity {
            opportunity_id: "one".into(),
            rubric_revision: "r1".into(),
            generated_candidate_ids: vec!["A".into(), "B".into(), "C".into()],
            baseline_admitted_candidate_ids: Some(vec!["A".into()]),
            independent_usefulness: Some(BTreeMap::from([
                ("A".into(), true),
                ("B".into(), true),
                ("C".into(), false),
            ])),
        }
    }

    fn shadow_summary() -> SuggestionShadowBaselineSummary {
        SuggestionShadowBaselineSummary {
            rubric_revision: "r1".into(),
            opportunity_count: 1,
            generated_count: 3,
            admitted_count: 1,
            not_admitted_count: 2,
            oracle_labelled_count: 3,
            useful_count: 2,
            useful_admitted_count: 1,
            useful_not_admitted_count: 1,
            ..Default::default()
        }
    }

    #[test]
    fn shadow_baseline_preserves_complete_opportunities() {
        let mut empty = shadow_opportunity();
        empty.opportunity_id = "empty".into();
        empty.generated_candidate_ids.clear();
        empty.baseline_admitted_candidate_ids = Some(vec![]);
        empty.independent_usefulness = Some(BTreeMap::new());
        assert_eq!(
            summarize_shadow_baseline(&[shadow_opportunity(), empty]),
            Ok(SuggestionShadowBaselineSummary {
                opportunity_count: 2,
                ..shadow_summary()
            })
        );
        let mut second = shadow_opportunity();
        second.opportunity_id = "two".into();
        assert_eq!(
            summarize_shadow_baseline(&[shadow_opportunity(), second])
                .unwrap()
                .generated_count,
            6
        );
    }

    #[test]
    fn shadow_baseline_all_rejected_keeps_useful_misses() {
        let mut row = shadow_opportunity();
        row.baseline_admitted_candidate_ids = Some(vec![]);
        assert_eq!(
            summarize_shadow_baseline(&[row]),
            Ok(SuggestionShadowBaselineSummary {
                admitted_count: 0,
                not_admitted_count: 3,
                useful_admitted_count: 0,
                useful_not_admitted_count: 2,
                ..shadow_summary()
            })
        );
    }

    #[test]
    fn shadow_baseline_unknown_is_not_negative() {
        let mut row = shadow_opportunity();
        row.independent_usefulness = None;
        assert_eq!(
            summarize_shadow_baseline(&[row]),
            Ok(SuggestionShadowBaselineSummary {
                oracle_labelled_count: 0,
                oracle_unknown_count: 3,
                useful_count: 0,
                useful_admitted_count: 0,
                useful_not_admitted_count: 0,
                ..shadow_summary()
            })
        );
        let mut row = shadow_opportunity();
        row.baseline_admitted_candidate_ids = None;
        assert_eq!(
            summarize_shadow_baseline(&[row]),
            Ok(SuggestionShadowBaselineSummary {
                admitted_count: 0,
                not_admitted_count: 0,
                admission_unknown_count: 3,
                useful_admitted_count: 0,
                useful_not_admitted_count: 0,
                useful_admission_unknown_count: 2,
                ..shadow_summary()
            })
        );
    }

    fn generation(
        elapsed_ms: Option<u64>,
        usage: Option<(u64, u64)>,
        cost: Option<(u64, &str)>,
    ) -> Generation {
        Generation {
            source: SuggestionSource::RuleBased,
            elapsed_ms,
            usage: usage.map(|(input_tokens, output_tokens)| DecisionUsage {
                input_tokens,
                output_tokens,
            }),
            paid_cost: cost.map(|(amount_minor, currency)| SuggestionShadowObservedCost {
                amount_minor,
                currency: currency.into(),
                receipt_revision: "bill1".into(),
            }),
        }
    }

    fn observed(subtotal: u64, known: usize, unknown: usize) -> SuggestionShadowObservedTotal {
        SuggestionShadowObservedTotal {
            subtotal,
            observed_opportunities: known,
            unknown_opportunities: unknown,
        }
    }

    fn observed_costs(values: &[(&str, u64)], known: usize, unknown: usize) -> CostSummary {
        CostSummary {
            subtotals_minor: values
                .iter()
                .map(|(currency, value)| ((*currency).into(), *value))
                .collect(),
            observed_opportunities: known,
            unknown_opportunities: unknown,
        }
    }

    fn shadow_capture(opportunity: SuggestionShadowOpportunity, generation: Generation) -> Receipt {
        use SuggestionShadowAdmissionObservation::{Admitted, NotAdmitted};
        let reasons = opportunity
            .baseline_admitted_candidate_ids
            .as_ref()
            .map(|admitted| {
                opportunity
                    .generated_candidate_ids
                    .iter()
                    .map(|id| {
                        let reason = if admitted.contains(id) {
                            Admitted
                        } else {
                            NotAdmitted(SuggestionShadowRejectionReason::ReasonNotObserved)
                        };
                        (id.clone(), reason)
                    })
                    .collect()
            });
        let binding = Binding {
            protocol_revision: "p1".into(),
            source_id: "source".into(),
            source_revision: "g1".into(),
            withdrawal_epoch: "w1".into(),
            rubric_revision: opportunity.rubric_revision.clone(),
        };
        SuggestionShadowCaptureReceipt::new(opportunity, binding, generation, reasons).unwrap()
    }

    fn capture(id: &str, generation: Generation) -> Receipt {
        let mut opportunity = shadow_opportunity();
        opportunity.opportunity_id = id.into();
        shadow_capture(opportunity, generation)
    }

    fn capture_bindings(rows: &[Receipt]) -> BTreeMap<String, Binding> {
        rows.iter()
            .map(|row| {
                (
                    row.opportunity().opportunity_id.clone(),
                    row.binding().clone(),
                )
            })
            .collect()
    }

    #[test]
    fn capture_summary_preserves_complete_population_and_useful_all_hide_misses() {
        let one = capture(
            "one",
            generation(Some(10), Some((7, 3)), Some((125, "USD"))),
        );
        let mut unknown = shadow_opportunity();
        unknown.opportunity_id = "two".into();
        unknown.generated_candidate_ids = vec!["A".into(), "B".into()];
        unknown.baseline_admitted_candidate_ids = None;
        unknown.independent_usefulness = None;
        let mut empty = shadow_opportunity();
        empty.opportunity_id = "empty".into();
        empty.generated_candidate_ids.clear();
        empty.baseline_admitted_candidate_ids = Some(vec![]);
        empty.independent_usefulness = Some(BTreeMap::new());
        let rows = vec![
            one,
            shadow_capture(unknown, generation(None, None, None)),
            shadow_capture(empty, generation(Some(0), Some((0, 0)), Some((0, "USD")))),
        ];
        let original = rows.clone();
        let expected = SuggestionShadowCaptureSummary {
            protocol_revision: "p1".into(),
            baseline: SuggestionShadowBaselineSummary {
                opportunity_count: 3,
                generated_count: 5,
                admission_unknown_count: 2,
                oracle_unknown_count: 2,
                ..shadow_summary()
            },
            elapsed_ms: observed(10, 2, 1),
            input_tokens: observed(7, 2, 1),
            output_tokens: observed(3, 2, 1),
            paid_cost: observed_costs(&[("USD", 125)], 2, 1),
        };
        assert_eq!(
            summarize_shadow_capture(&rows, &capture_bindings(&rows)),
            Ok(expected.clone())
        );
        assert_eq!(rows, original);
        let mut hidden = rows[0].opportunity().clone();
        hidden.baseline_admitted_candidate_ids = Some(vec![]);
        let hidden = vec![
            shadow_capture(hidden, rows[0].generation().clone()),
            rows[1].clone(),
            rows[2].clone(),
        ];
        let hidden_summary = summarize_shadow_capture(&hidden, &capture_bindings(&hidden)).unwrap();
        assert_eq!(
            hidden_summary.baseline,
            SuggestionShadowBaselineSummary {
                admitted_count: 0,
                not_admitted_count: 3,
                useful_admitted_count: 0,
                useful_not_admitted_count: 2,
                ..expected.baseline
            }
        );
    }

    #[test]
    fn capture_summary_coverage_is_independent_for_elapsed_usage_and_cost() {
        let mut rows = Vec::new();
        for (id, elapsed_ms, usage) in [
            ("one", Some(0), Some((0, 0))),
            ("two", Some(10), None),
            ("three", None, None),
        ] {
            rows.push(capture(id, generation(elapsed_ms, usage, None)));
        }
        let summary = summarize_shadow_capture(&rows, &capture_bindings(&rows)).unwrap();
        assert_eq!(summary.elapsed_ms, observed(10, 2, 1));
        assert_eq!(summary.input_tokens, observed(0, 1, 2));
        assert_eq!(summary.output_tokens, observed(0, 1, 2));
        assert_eq!(summary.paid_cost, observed_costs(&[], 0, 3));
    }

    #[test]
    fn capture_summary_rejects_late_staleness_and_manifest_mismatch_before_overflow() {
        use SuggestionShadowBatchError::{InvalidCapture, InvalidPopulation, PopulationMismatch};
        use SuggestionShadowValidationError::EmptyPopulation;
        assert_eq!(
            summarize_shadow_capture(&[], &BTreeMap::new()),
            Err(InvalidPopulation(EmptyPopulation))
        );
        let rows = vec![
            capture("one", generation(Some(u64::MAX), None, None)),
            capture("two", generation(Some(1), None, None)),
        ];
        let original = rows.clone();
        let mut bindings = capture_bindings(&rows);
        bindings.get_mut("two").unwrap().source_revision = "changed".into();
        assert_eq!(
            summarize_shadow_capture(&rows, &bindings),
            Err(InvalidCapture(SuggestionShadowCaptureError::SourceChanged))
        );
        bindings = capture_bindings(&rows);
        bindings.insert("outside".into(), rows[0].binding().clone());
        assert_eq!(
            summarize_shadow_capture(&rows, &bindings),
            Err(PopulationMismatch)
        );
        assert_eq!(rows, original);
    }

    #[test]
    fn capture_summary_accepts_max_and_max_plus_zero_but_rejects_each_overflow() {
        type Observe = fn(&mut Generation, u64);
        let axes: [Observe; 4] = [
            |g, value| *g = generation(Some(value), None, None),
            |g, value| *g = generation(None, Some((value, 0)), None),
            |g, value| *g = generation(None, Some((0, value)), None),
            |g, value| *g = generation(None, None, Some((value, "USD"))),
        ];
        for (axis, observe) in axes.into_iter().enumerate() {
            let mut observation = generation(None, None, None);
            observe(&mut observation, u64::MAX);
            let max = capture("one", observation);
            for extra in [None, Some(0), Some(1)] {
                let mut rows = vec![max.clone()];
                if let Some(value) = extra {
                    let mut observation = generation(None, None, None);
                    observe(&mut observation, value);
                    rows.push(capture("two", observation));
                }
                let original = rows.clone();
                let result = summarize_shadow_capture(&rows, &capture_bindings(&rows));
                if extra == Some(1) {
                    assert_eq!(result, Err(SuggestionShadowBatchError::ObservationOverflow));
                } else {
                    let summary = result.unwrap();
                    if axis == 3 {
                        assert_eq!(
                            summary.paid_cost,
                            observed_costs(&[("USD", u64::MAX)], rows.len(), 0)
                        );
                    } else {
                        let totals = [
                            summary.elapsed_ms,
                            summary.input_tokens,
                            summary.output_tokens,
                        ];
                        assert_eq!(totals[axis], observed(u64::MAX, rows.len(), 0));
                    }
                }
                assert_eq!(rows, original);
            }
        }
        for euros in [0, 1, u64::MAX] {
            let rows = vec![
                capture("one", generation(None, None, Some((u64::MAX, "USD")))),
                capture("two", generation(None, None, Some((euros, "EUR")))),
            ];
            let summary = summarize_shadow_capture(&rows, &capture_bindings(&rows)).unwrap();
            assert_eq!(
                summary.paid_cost,
                observed_costs(&[("USD", u64::MAX), ("EUR", euros)], 2, 0)
            );
        }
    }
}
