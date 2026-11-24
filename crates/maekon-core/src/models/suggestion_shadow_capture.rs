//! Immutable metadata joins for complete suggestion generation opportunities.
//! Caller-supplied bindings do not authenticate an oracle or establish live context,
//! consent, provider authority, UI exposure, savings, or an experiment verdict.

use super::candidate_decision::DecisionUsage;
use super::suggestion::{
    SuggestionShadowBaselineSummary, SuggestionShadowOpportunity, SuggestionShadowValidationError,
    SuggestionSource,
};
use std::collections::{BTreeMap, BTreeSet};

/// Local capture revisions, supplied and rechecked by a future trusted runtime.
/// Identifiers preserve their bytes; they must contain no raw context or credentials.
/// Each identifier has 1..=256 UTF-8 bytes, is nonblank, and has no control characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionShadowCaptureBinding {
    pub protocol_revision: String,
    pub source_id: String,
    pub source_revision: String,
    /// A local scope revision, never a capability, nonce, secret, or consent flag.
    pub withdrawal_epoch: String,
    pub rubric_revision: String,
}

/// Actual generation observations. Missing values remain unknown, including cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionShadowGenerationObservation {
    pub source: SuggestionSource,
    pub elapsed_ms: Option<u64>,
    pub usage: Option<DecisionUsage>,
    pub paid_cost: Option<SuggestionShadowObservedCost>,
}

/// A supplied observed charge, without inferred pricing, conversion, or savings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionShadowObservedCost {
    pub amount_minor: u64,
    /// Exactly three uppercase ASCII letters, preserved without normalization.
    pub currency: String,
    pub receipt_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionShadowAdmissionObservation {
    Admitted,
    NotAdmitted(SuggestionShadowRejectionReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionShadowRejectionReason {
    ReasonNotObserved,
    QueueDisabled,
    DuplicateNotBetter,
    CapacityPriority,
    OrderingCollision,
    CapacityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SuggestionShadowCaptureError {
    #[error("invalid shadow opportunity: {0}")]
    InvalidOpportunity(#[from] SuggestionShadowValidationError),
    #[error("invalid shadow capture binding")]
    InvalidBinding,
    #[error("invalid observed generation cost")]
    InvalidCostObservation,
    #[error("admission observations are incomplete or unavailable")]
    IncompleteAdmissionObservations,
    #[error("admission observation names a candidate outside this opportunity")]
    UnknownAdmissionCandidate,
    #[error("admission observation differs from the captured admission set")]
    AdmissionObservationMismatch,
    #[error("capture protocol revision changed")]
    ProtocolChanged,
    #[error("capture rubric revision changed")]
    RubricChanged,
    #[error("capture source ID or revision changed")]
    SourceChanged,
    #[error("capture withdrawal epoch changed")]
    WithdrawalChanged,
}

/// A known subtotal with explicit coverage over the declared opportunity population.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuggestionShadowObservedTotal {
    pub subtotal: u64,
    pub observed_opportunities: usize,
    pub unknown_opportunities: usize,
}

impl SuggestionShadowObservedTotal {
    /// Add one supplied observation; a subtotal overflow leaves this total unchanged.
    pub fn add_observation(
        &mut self,
        value: Option<u64>,
    ) -> Result<(), SuggestionShadowBatchError> {
        match value {
            Some(value) => {
                self.subtotal = self
                    .subtotal
                    .checked_add(value)
                    .ok_or(SuggestionShadowBatchError::ObservationOverflow)?;
                self.observed_opportunities += 1;
            }
            None => self.unknown_opportunities += 1,
        }
        Ok(())
    }
}

/// Supplied generation charges stay separate by currency, including observed zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuggestionShadowObservedCostSummary {
    pub subtotals_minor: BTreeMap<String, u64>,
    pub observed_opportunities: usize,
    pub unknown_opportunities: usize,
}

/// Raw declared-population evidence, not authenticated billing, savings, or a verdict.
/// These observations describe original generation, not additional Jev evaluation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuggestionShadowCaptureSummary {
    pub protocol_revision: String,
    pub baseline: SuggestionShadowBaselineSummary,
    pub elapsed_ms: SuggestionShadowObservedTotal,
    pub input_tokens: SuggestionShadowObservedTotal,
    pub output_tokens: SuggestionShadowObservedTotal,
    pub paid_cost: SuggestionShadowObservedCostSummary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SuggestionShadowBatchError {
    #[error("invalid shadow population: {0}")]
    InvalidPopulation(#[from] SuggestionShadowValidationError),
    #[error("invalid shadow capture: {0}")]
    InvalidCapture(#[from] SuggestionShadowCaptureError),
    #[error("capture population differs from the declared population")]
    PopulationMismatch,
    #[error("capture population mixes protocol revisions")]
    MixedProtocol,
    #[error("observed generation subtotal overflowed")]
    ObservationOverflow,
}

/// Validated owned evidence, retained unchanged even when its binding becomes stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuggestionShadowCaptureReceipt {
    opportunity: SuggestionShadowOpportunity,
    binding: SuggestionShadowCaptureBinding,
    generation: SuggestionShadowGenerationObservation,
    admission_reasons: Option<BTreeMap<String, SuggestionShadowAdmissionObservation>>,
}

impl SuggestionShadowCaptureReceipt {
    /// Validate S1 population, metadata, observed cost, then complete admission bindings.
    pub fn new(
        opportunity: SuggestionShadowOpportunity,
        binding: SuggestionShadowCaptureBinding,
        generation: SuggestionShadowGenerationObservation,
        admission_reasons: Option<BTreeMap<String, SuggestionShadowAdmissionObservation>>,
    ) -> Result<Self, SuggestionShadowCaptureError> {
        validate_capture_inputs(&opportunity, &binding, &generation, &admission_reasons)?;
        Ok(Self {
            opportunity,
            binding,
            generation,
            admission_reasons,
        })
    }

    pub fn binding(&self) -> &SuggestionShadowCaptureBinding {
        &self.binding
    }

    /// Original evidence only; this does not establish current binding, freshness, or consent.
    pub fn opportunity(&self) -> &SuggestionShadowOpportunity {
        &self.opportunity
    }

    pub fn generation(&self) -> &SuggestionShadowGenerationObservation {
        &self.generation
    }

    /// `None` is unknown; an observed map covers every original generated candidate.
    pub fn admission_reasons(
        &self,
    ) -> Option<&BTreeMap<String, SuggestionShadowAdmissionObservation>> {
        self.admission_reasons.as_ref()
    }

    /// Validate expected metadata first; compare protocol, rubric, source, then withdrawal.
    /// Exact equality is a local evidence join, not authentication of the expected binding.
    pub fn validate_current_binding(
        &self,
        expected: &SuggestionShadowCaptureBinding,
    ) -> Result<(), SuggestionShadowCaptureError> {
        use SuggestionShadowCaptureError::*;
        expected.validate()?;
        if self.binding.protocol_revision != expected.protocol_revision {
            return Err(ProtocolChanged);
        }
        if self.binding.rubric_revision != expected.rubric_revision {
            return Err(RubricChanged);
        }
        if self.binding.source_id != expected.source_id
            || self.binding.source_revision != expected.source_revision
        {
            return Err(SourceChanged);
        }
        if self.binding.withdrawal_epoch != expected.withdrawal_epoch {
            return Err(WithdrawalChanged);
        }
        Ok(())
    }

    /// Project every original candidate and label without filtering stale evidence away.
    pub fn validated_baseline(
        &self,
        expected: &SuggestionShadowCaptureBinding,
    ) -> Result<&SuggestionShadowOpportunity, SuggestionShadowCaptureError> {
        self.validate_current_binding(expected)?;
        Ok(&self.opportunity)
    }

    /// Join every receipt to caller-declared current data without filtering any row.
    /// Check row bindings and protocol, S1 population, then exact expected coverage.
    /// The map proves neither authentication nor the actual runtime population.
    pub fn validated_population(
        receipts: &[Self],
        expected: &BTreeMap<String, SuggestionShadowCaptureBinding>,
    ) -> Result<Vec<SuggestionShadowOpportunity>, SuggestionShadowBatchError> {
        use SuggestionShadowBatchError::*;
        let protocol = receipts.first().map(|row| &row.binding.protocol_revision);
        let mut population = Vec::with_capacity(receipts.len());
        for receipt in receipts {
            let binding = expected
                .get(&receipt.opportunity.opportunity_id)
                .ok_or(PopulationMismatch)?;
            population.push(receipt.validated_baseline(binding)?.clone());
            if Some(&receipt.binding.protocol_revision) != protocol {
                return Err(MixedProtocol);
            }
        }
        SuggestionShadowOpportunity::validate_population(&population)?;
        if population.len() != expected.len() {
            return Err(PopulationMismatch);
        }
        Ok(population)
    }
}

/// Check complete borrowed evidence before the receipt takes ownership.
fn validate_capture_inputs(
    opportunity: &SuggestionShadowOpportunity,
    binding: &SuggestionShadowCaptureBinding,
    generation: &SuggestionShadowGenerationObservation,
    admission_reasons: &Option<BTreeMap<String, SuggestionShadowAdmissionObservation>>,
) -> Result<(), SuggestionShadowCaptureError> {
    use SuggestionShadowCaptureError::*;
    SuggestionShadowOpportunity::validate_population(std::slice::from_ref(opportunity))?;
    binding.validate()?;
    if binding.rubric_revision != opportunity.rubric_revision {
        return Err(InvalidBinding);
    }
    if let Some(cost) = &generation.paid_cost {
        if cost.currency.len() != 3 || !cost.currency.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(InvalidCostObservation);
        }
        validate_identifier(&cost.receipt_revision).map_err(|_| InvalidCostObservation)?;
    }
    match (
        &opportunity.baseline_admitted_candidate_ids,
        admission_reasons,
    ) {
        (None, None) => {}
        (Some(admitted), Some(reasons)) => {
            let generated: BTreeSet<_> = opportunity.generated_candidate_ids.iter().collect();
            // Unknown keys take precedence, including equal-cardinality substitutions.
            if reasons.keys().any(|id| !generated.contains(id)) {
                return Err(UnknownAdmissionCandidate);
            }
            if reasons.len() != generated.len() {
                return Err(IncompleteAdmissionObservations);
            }
            for (id, observation) in reasons {
                if matches!(observation, SuggestionShadowAdmissionObservation::Admitted)
                    != admitted.contains(id)
                {
                    return Err(AdmissionObservationMismatch);
                }
            }
        }
        _ => return Err(IncompleteAdmissionObservations),
    }
    Ok(())
}

impl SuggestionShadowCaptureBinding {
    fn validate(&self) -> Result<(), SuggestionShadowCaptureError> {
        for id in [
            &self.protocol_revision,
            &self.source_id,
            &self.source_revision,
            &self.withdrawal_epoch,
            &self.rubric_revision,
        ] {
            validate_identifier(id)?;
        }
        Ok(())
    }
}

fn validate_identifier(value: &str) -> Result<(), SuggestionShadowCaptureError> {
    if value.len() > 256 || value.trim().is_empty() || value.chars().any(char::is_control) {
        return Err(SuggestionShadowCaptureError::InvalidBinding);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use SuggestionShadowAdmissionObservation::{Admitted, NotAdmitted};
    use SuggestionShadowCaptureError::*;
    use SuggestionShadowRejectionReason::*;

    #[derive(Clone)]
    struct Inputs {
        opportunity: SuggestionShadowOpportunity,
        binding: SuggestionShadowCaptureBinding,
        generation: SuggestionShadowGenerationObservation,
        reasons: Option<BTreeMap<String, SuggestionShadowAdmissionObservation>>,
    }

    impl Inputs {
        fn capture(self) -> Result<SuggestionShadowCaptureReceipt, SuggestionShadowCaptureError> {
            SuggestionShadowCaptureReceipt::new(
                self.opportunity,
                self.binding,
                self.generation,
                self.reasons,
            )
        }

        fn assert_capture(self) {
            let expected = self.clone();
            let receipt = self.capture().unwrap();
            assert_eq!(receipt.binding(), &expected.binding);
            assert_eq!(receipt.opportunity(), &expected.opportunity);
            assert_eq!(receipt.generation(), &expected.generation);
            assert_eq!(receipt.admission_reasons(), expected.reasons.as_ref());
            assert_eq!(receipt.validate_current_binding(&expected.binding), Ok(()));
            assert_eq!(
                receipt.validated_baseline(&expected.binding),
                Ok(&expected.opportunity)
            );
        }
    }

    fn inputs() -> Inputs {
        Inputs {
            opportunity: SuggestionShadowOpportunity {
                opportunity_id: "one".into(),
                rubric_revision: "r1".into(),
                generated_candidate_ids: vec!["A".into(), "B".into(), "C".into()],
                baseline_admitted_candidate_ids: Some(vec!["A".into()]),
                independent_usefulness: Some(BTreeMap::from([
                    ("A".into(), true),
                    ("B".into(), true),
                    ("C".into(), false),
                ])),
            },
            binding: SuggestionShadowCaptureBinding {
                protocol_revision: "p1".into(),
                source_id: "source".into(),
                source_revision: "g1".into(),
                withdrawal_epoch: "w1".into(),
                rubric_revision: "r1".into(),
            },
            generation: SuggestionShadowGenerationObservation {
                source: SuggestionSource::LlmServer,
                elapsed_ms: None,
                usage: None,
                paid_cost: None,
            },
            reasons: Some(BTreeMap::from([
                ("A".into(), Admitted),
                ("B".into(), NotAdmitted(ReasonNotObserved)),
                ("C".into(), NotAdmitted(CapacityPriority)),
            ])),
        }
    }

    fn observed_cost() -> SuggestionShadowObservedCost {
        SuggestionShadowObservedCost {
            amount_minor: 0,
            currency: "USD".into(),
            receipt_revision: "bill1".into(),
        }
    }

    #[test]
    fn complete_projection_preserves_observed_values_and_useful_rejection() {
        let mut input = inputs();
        input.generation = SuggestionShadowGenerationObservation {
            source: SuggestionSource::LlmLocal,
            elapsed_ms: Some(18),
            usage: Some(DecisionUsage {
                input_tokens: 37,
                output_tokens: 11,
            }),
            paid_cost: Some(SuggestionShadowObservedCost {
                amount_minor: u64::MAX,
                ..observed_cost()
            }),
        };
        input.assert_capture();
    }

    #[test]
    fn complete_empty_and_all_rejected_opportunities_are_retained() {
        let mut empty = inputs();
        empty.opportunity.generated_candidate_ids.clear();
        empty.opportunity.baseline_admitted_candidate_ids = Some(vec![]);
        empty.opportunity.independent_usefulness = Some(BTreeMap::new());
        empty.reasons = Some(BTreeMap::new());
        empty.assert_capture();
        let mut rejected = inputs();
        rejected.opportunity.baseline_admitted_candidate_ids = Some(vec![]);
        rejected
            .reasons
            .as_mut()
            .unwrap()
            .insert("A".into(), NotAdmitted(QueueDisabled));
        rejected.assert_capture();
    }

    #[test]
    fn unknown_observations_are_distinct_from_explicit_zero() {
        let mut unknown = inputs();
        unknown.opportunity.baseline_admitted_candidate_ids = None;
        unknown.reasons = None;
        unknown.assert_capture();
        let mut zero = inputs();
        zero.generation.elapsed_ms = Some(0);
        zero.generation.usage = Some(DecisionUsage {
            input_tokens: 0,
            output_tokens: 0,
        });
        zero.generation.paid_cost = Some(observed_cost());
        zero.assert_capture();
    }

    type BindingField = fn(&mut SuggestionShadowCaptureBinding) -> &mut String;
    fn binding_fields() -> [BindingField; 5] {
        [
            |b| &mut b.protocol_revision,
            |b| &mut b.source_id,
            |b| &mut b.source_revision,
            |b| &mut b.withdrawal_epoch,
            |b| &mut b.rubric_revision,
        ]
    }

    #[test]
    fn metadata_is_bounded_in_utf8_bytes_without_normalization() {
        let receipt = inputs().capture().unwrap();
        for field in binding_fields() {
            for invalid in [
                "".into(),
                " \u{2003}".into(),
                "a\n".into(),
                "a\u{0080}".into(),
                "x".repeat(257),
                "é".repeat(129),
            ] {
                let mut input = inputs();
                *field(&mut input.binding) = invalid;
                assert_eq!(
                    receipt.validate_current_binding(&input.binding),
                    Err(InvalidBinding)
                );
                assert_eq!(input.capture(), Err(InvalidBinding));
            }
            for valid in [
                "x".into(),
                "x".repeat(256),
                "é".repeat(128),
                " source ".into(),
            ] {
                let mut input = inputs();
                *field(&mut input.binding) = valid;
                input.opportunity.rubric_revision = input.binding.rubric_revision.clone();
                let expected = input.binding.clone();
                assert_eq!(input.capture().unwrap().binding(), &expected);
            }
        }
        let mut input = inputs();
        input.binding.rubric_revision = "r2".into();
        assert_eq!(input.capture(), Err(InvalidBinding));
        let mut expected = receipt.binding().clone();
        expected.source_id = " source ".into();
        assert_eq!(
            receipt.validate_current_binding(&expected),
            Err(SourceChanged)
        );
    }

    #[test]
    fn cost_currency_and_receipt_validation_never_infers_a_price() {
        for currency in ["", "US", "USDD", "usd", "U1D", "ÉA"] {
            let mut input = inputs();
            input.generation.paid_cost = Some(SuggestionShadowObservedCost {
                currency: currency.into(),
                ..observed_cost()
            });
            assert_eq!(input.capture(), Err(InvalidCostObservation));
        }
        for revision in ["".into(), " ".into(), "a\n".into(), "x".repeat(257)] {
            let mut input = inputs();
            input.generation.paid_cost = Some(SuggestionShadowObservedCost {
                receipt_revision: revision,
                ..observed_cost()
            });
            assert_eq!(input.capture(), Err(InvalidCostObservation));
        }
    }

    #[test]
    fn malformed_opportunities_keep_the_exact_s1_error() {
        type Corrupt = fn(&mut SuggestionShadowOpportunity);
        use SuggestionShadowValidationError::*;
        let cases: [(SuggestionShadowValidationError, Corrupt); 4] = [
            (DuplicateGeneratedCandidateId, |o| {
                o.generated_candidate_ids.push("A".into())
            }),
            (UnknownAdmittedCandidateId, |o| {
                o.baseline_admitted_candidate_ids = Some(vec!["X".into()])
            }),
            (IncompleteOracle, |o| {
                o.independent_usefulness.as_mut().unwrap().remove("B");
            }),
            (UnknownOracleCandidateId, |o| {
                let labels = o.independent_usefulness.as_mut().unwrap();
                labels.remove("B");
                labels.insert("X".into(), true);
            }),
        ];
        for (error, corrupt) in cases {
            let mut input = inputs();
            corrupt(&mut input.opportunity);
            assert_eq!(input.capture(), Err(InvalidOpportunity(error)));
        }
    }

    #[test]
    fn admission_observations_require_exact_keys_and_membership() {
        type Corrupt = fn(&mut Inputs);
        let cases: [(SuggestionShadowCaptureError, Corrupt); 8] = [
            (IncompleteAdmissionObservations, |i| i.reasons = None),
            (IncompleteAdmissionObservations, |i| {
                i.opportunity.baseline_admitted_candidate_ids = None
            }),
            (IncompleteAdmissionObservations, |i| {
                i.reasons.as_mut().unwrap().remove("B");
            }),
            (UnknownAdmissionCandidate, |i| {
                i.reasons
                    .as_mut()
                    .unwrap()
                    .insert("X".into(), NotAdmitted(ReasonNotObserved));
            }),
            (UnknownAdmissionCandidate, |i| {
                let reasons = i.reasons.as_mut().unwrap();
                reasons.remove("B");
                reasons.insert("X".into(), NotAdmitted(ReasonNotObserved));
            }),
            (AdmissionObservationMismatch, |i| {
                i.reasons
                    .as_mut()
                    .unwrap()
                    .insert("A".into(), NotAdmitted(DuplicateNotBetter));
            }),
            (AdmissionObservationMismatch, |i| {
                i.reasons.as_mut().unwrap().insert("B".into(), Admitted);
            }),
            (AdmissionObservationMismatch, |i| {
                i.opportunity.baseline_admitted_candidate_ids = Some(vec!["B".into()])
            }),
        ];
        for (error, corrupt) in cases {
            let mut input = inputs();
            corrupt(&mut input);
            assert_eq!(input.capture(), Err(error));
        }
    }

    #[test]
    fn changed_bindings_reject_projection_without_mutating_original_evidence() {
        let mut input = inputs();
        input.generation.paid_cost = Some(observed_cost());
        let receipt = input.capture().unwrap();
        let original = receipt.clone();
        let mut expected = receipt.binding().clone();
        let mut captured = receipt.binding().clone();
        for field in binding_fields() {
            *field(&mut expected) = "changed".into();
        }
        for (field, error) in [
            (0, ProtocolChanged),
            (4, RubricChanged),
            (1, SourceChanged),
            (2, SourceChanged),
            (3, WithdrawalChanged),
        ] {
            let mut single_change = receipt.binding().clone();
            *binding_fields()[field](&mut single_change) = "changed".into();
            assert_eq!(receipt.validate_current_binding(&single_change), Err(error));
            assert_eq!(receipt.validated_baseline(&single_change), Err(error));
            assert_eq!(receipt.validate_current_binding(&expected), Err(error));
            *binding_fields()[field](&mut expected) =
                binding_fields()[field](&mut captured).clone();
            assert_eq!(receipt.opportunity(), original.opportunity());
            assert_eq!(receipt.generation(), original.generation());
            assert_eq!(receipt.admission_reasons(), original.admission_reasons());
            assert_eq!(receipt, original);
        }
        assert_eq!(receipt.validate_current_binding(&expected), Ok(()));
        assert_eq!(receipt, original);
    }

    #[test]
    fn observed_total_preserves_unknown_zero_and_atomic_overflow() {
        let mut total = SuggestionShadowObservedTotal::default();
        assert_eq!(total.add_observation(None), Ok(()));
        assert_eq!(
            total,
            SuggestionShadowObservedTotal {
                subtotal: 0,
                observed_opportunities: 0,
                unknown_opportunities: 1,
            }
        );
        assert_eq!(total.add_observation(Some(0)), Ok(()));
        assert_eq!(
            total,
            SuggestionShadowObservedTotal {
                subtotal: 0,
                observed_opportunities: 1,
                unknown_opportunities: 1,
            }
        );
        assert_eq!(total.add_observation(Some(u64::MAX)), Ok(()));
        let full = SuggestionShadowObservedTotal {
            subtotal: u64::MAX,
            observed_opportunities: 2,
            unknown_opportunities: 1,
        };
        assert_eq!(total, full);
        assert_eq!(
            total.add_observation(Some(1)),
            Err(SuggestionShadowBatchError::ObservationOverflow)
        );
        assert_eq!(total, full);
        assert_eq!(total.add_observation(Some(0)), Ok(()));
        assert_eq!(
            total,
            SuggestionShadowObservedTotal {
                observed_opportunities: 3,
                ..full
            }
        );
    }

    fn population_bindings(
        receipts: &[SuggestionShadowCaptureReceipt],
    ) -> BTreeMap<String, SuggestionShadowCaptureBinding> {
        receipts
            .iter()
            .map(|row| {
                (
                    row.opportunity().opportunity_id.clone(),
                    row.binding().clone(),
                )
            })
            .collect()
    }

    #[test]
    fn batch_population_keeps_empty_unknown_and_local_candidate_ids() {
        use SuggestionShadowBatchError::PopulationMismatch;
        let mut unknown = inputs();
        unknown.opportunity.opportunity_id = "two".into();
        unknown.opportunity.baseline_admitted_candidate_ids = None;
        unknown.opportunity.independent_usefulness = None;
        unknown.reasons = None;
        let mut empty = inputs();
        empty.opportunity.opportunity_id = "empty".into();
        empty.opportunity.generated_candidate_ids.clear();
        empty.opportunity.baseline_admitted_candidate_ids = Some(vec![]);
        empty.opportunity.independent_usefulness = Some(BTreeMap::new());
        empty.reasons = Some(BTreeMap::new());
        let rows = vec![
            inputs().capture().unwrap(),
            unknown.capture().unwrap(),
            empty.capture().unwrap(),
        ];
        let original = rows.clone();
        let mut expected = population_bindings(&rows);
        let validate = SuggestionShadowCaptureReceipt::validated_population;
        assert_eq!(
            validate(&rows, &expected),
            Ok(rows.iter().map(|row| row.opportunity().clone()).collect())
        );
        assert_eq!(validate(&rows[..2], &expected), Err(PopulationMismatch));
        assert_eq!(
            validate(&[rows[0].clone(), rows[2].clone()], &expected),
            Err(PopulationMismatch)
        );
        expected.remove("empty");
        assert_eq!(validate(&rows, &expected), Err(PopulationMismatch));
        expected.insert("outside".into(), rows[2].binding().clone());
        assert_eq!(validate(&rows, &expected), Err(PopulationMismatch));
        assert_eq!(rows, original);
    }

    #[test]
    fn batch_population_preserves_exact_s1_errors_and_mixed_protocol_guard() {
        use SuggestionShadowBatchError::{InvalidPopulation, MixedProtocol};
        use SuggestionShadowValidationError::{
            DuplicateOpportunityId, EmptyPopulation, RubricRevisionMismatch,
        };
        let validate = SuggestionShadowCaptureReceipt::validated_population;
        assert_eq!(
            validate(&[], &BTreeMap::new()),
            Err(InvalidPopulation(EmptyPopulation))
        );
        let one = inputs().capture().unwrap();
        let duplicates = vec![one.clone(), one.clone()];
        assert_eq!(
            validate(&duplicates, &population_bindings(&duplicates)),
            Err(InvalidPopulation(DuplicateOpportunityId))
        );
        for (protocol, rubric, error) in [
            ("p2", "r1", MixedProtocol),
            ("p1", "r2", InvalidPopulation(RubricRevisionMismatch)),
        ] {
            let mut two = inputs();
            two.opportunity.opportunity_id = "two".into();
            two.opportunity.rubric_revision = rubric.into();
            two.binding.rubric_revision = rubric.into();
            two.binding.protocol_revision = protocol.into();
            let rows = vec![one.clone(), two.capture().unwrap()];
            let original = rows.clone();
            assert_eq!(validate(&rows, &population_bindings(&rows)), Err(error));
            assert_eq!(rows, original);
        }
    }

    #[test]
    fn batch_rechecks_each_current_binding_before_population_projection() {
        use SuggestionShadowBatchError::{InvalidCapture, PopulationMismatch};
        let mut two = inputs();
        two.opportunity.opportunity_id = "two".into();
        let rows = vec![inputs().capture().unwrap(), two.capture().unwrap()];
        let original = rows.clone();
        let validate = SuggestionShadowCaptureReceipt::validated_population;
        for (field, error) in [
            (0, ProtocolChanged),
            (4, RubricChanged),
            (1, SourceChanged),
            (2, SourceChanged),
            (3, WithdrawalChanged),
        ] {
            let mut expected = population_bindings(&rows);
            *binding_fields()[field](expected.get_mut("two").unwrap()) = "changed".into();
            assert_eq!(validate(&rows, &expected), Err(InvalidCapture(error)));
            expected.insert("outside".into(), rows[0].binding().clone());
            assert_eq!(validate(&rows, &expected), Err(InvalidCapture(error)));
            assert_eq!(rows, original);
        }
        let mut expected = population_bindings(&rows);
        expected.get_mut("two").unwrap().source_id.clear();
        assert_eq!(
            validate(&rows, &expected),
            Err(InvalidCapture(InvalidBinding))
        );
        expected = population_bindings(&rows);
        expected.insert("outside".into(), rows[0].binding().clone());
        assert_eq!(validate(&rows, &expected), Err(PopulationMismatch));
    }
}
