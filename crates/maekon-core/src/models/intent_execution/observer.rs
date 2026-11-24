//! Execution-owned observations; no provider routing or execution authority.

use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;

use super::{
    IntentActionOutcome, IntentAttemptOutcome, IntentCapabilityObservation,
    IntentConfigurationReason, IntentExecutionEvent, IntentExecutionEventKind,
    IntentExecutionProvenance, IntentObservation, IntentObservationStorage, IntentPlanningOutcome,
    IntentProvenanceVersion, IntentProviderAttempt, IntentProviderClass, IntentRevision,
    IntentSafeReason, IntentSubmission, OpaqueIntentId, MAX_INTENT_ATTEMPTS, MAX_INTENT_EVENTS,
};
use crate::error::CoreError;

struct ObserverState {
    receipt: IntentExecutionProvenance,
    finalized: bool,
    terminal: bool,
    reliable: bool,
    legacy: bool,
    requested_frozen: bool,
}

/// Owned by one execution and initialized before any planning await.
#[derive(Clone)]
pub struct IntentExecutionObserver(Arc<Mutex<ObserverState>>);

impl IntentExecutionObserver {
    pub fn new(submission: IntentSubmission) -> Self {
        let execution_id = OpaqueIntentId::new();
        let receipt = IntentExecutionProvenance {
            schema_version: IntentProvenanceVersion::V1,
            execution_id,
            submission_id: submission.submission_id,
            retry_of: submission.retry_of,
            config_revision: IntentRevision::Unknown,
            requested_provider_class: IntentProviderClass::Unknown,
            configuration_reason: None,
            attempts: Vec::new(),
            planning_outcome: IntentPlanningOutcome::Unknown,
            action_outcome: IntentActionOutcome::Unknown,
            observation: IntentObservation::Partial,
            observation_reason: None,
            storage: IntentObservationStorage::ProcessMemory,
            events: Vec::new(),
        };
        let observer = Self(Arc::new(Mutex::new(ObserverState {
            receipt,
            finalized: false,
            terminal: false,
            reliable: true,
            legacy: false,
            requested_frozen: false,
        })));
        observer
            .0
            .lock()
            .push_event(IntentExecutionEventKind::Started {});
        observer
    }

    pub fn execution_id(&self) -> OpaqueIntentId {
        self.0.lock().receipt.execution_id
    }

    pub fn requested_provider(&self, provider: IntentProviderClass) {
        self.configure(provider, None);
    }

    pub fn configure(
        &self,
        provider: IntentProviderClass,
        reason: Option<IntentConfigurationReason>,
    ) {
        let mut state = self.0.lock();
        if state.terminal
            || (state.requested_frozen
                && (state.receipt.requested_provider_class != provider
                    || state.receipt.configuration_reason != reason))
        {
            state.mark_partial();
            return;
        }
        state.receipt.requested_provider_class = provider;
        state.receipt.configuration_reason = reason;
        state.requested_frozen = true;
    }

    pub fn mark_legacy(&self) {
        self.0.lock().legacy = true;
    }

    pub fn mark_partial(&self) {
        self.0.lock().mark_partial();
    }

    pub fn begin_attempt(
        &self,
        provider: IntentProviderClass,
        capabilities: IntentCapabilityObservation,
    ) -> Option<OpaqueIntentId> {
        let mut state = self.0.lock();
        if state.terminal || state.receipt.attempts.len() >= MAX_INTENT_ATTEMPTS {
            state.mark_partial();
            return None;
        }
        let attempt_id = OpaqueIntentId::new();
        let predecessor = state
            .receipt
            .attempts
            .last()
            .map(|attempt| attempt.attempt_id);
        state.receipt.attempts.push(IntentProviderAttempt {
            attempt_id,
            predecessor,
            effective_provider_class: provider,
            model_revision: if provider == IntentProviderClass::Rule {
                IntentRevision::NotApplicable
            } else {
                IntentRevision::Unknown
            },
            outcome: IntentAttemptOutcome::Unknown,
            reason: None,
            capabilities,
        });
        state.push_event(IntentExecutionEventKind::AttemptStarted { attempt_id });
        drop(state);
        Some(attempt_id)
    }

    pub fn end_attempt(&self, attempt_id: Option<OpaqueIntentId>, result: Result<(), &CoreError>) {
        let Some(attempt_id) = attempt_id else {
            return;
        };
        let mut state = self.0.lock();
        if state.terminal {
            state.mark_partial();
            return;
        }
        let Some(attempt) = state
            .receipt
            .attempts
            .iter_mut()
            .find(|a| a.attempt_id == attempt_id)
        else {
            state.mark_partial();
            return;
        };
        if attempt.outcome != IntentAttemptOutcome::Unknown {
            state.mark_partial();
            return;
        }
        match result {
            Ok(()) => attempt.outcome = IntentAttemptOutcome::Succeeded,
            Err(error) => {
                attempt.outcome = IntentAttemptOutcome::from_error(error);
                attempt.reason = Some(IntentSafeReason::from_error(error));
            }
        }
        state.push_event(IntentExecutionEventKind::AttemptFinished { attempt_id });
    }

    pub fn planning_succeeded(&self) {
        let provider = self
            .0
            .lock()
            .receipt
            .attempts
            .iter()
            .rev()
            .find(|a| a.outcome == IntentAttemptOutcome::Succeeded)
            .map(|a| a.effective_provider_class);
        let outcome = match provider {
            Some(IntentProviderClass::Rule) => IntentPlanningOutcome::Rule,
            Some(IntentProviderClass::LocalModel | IntentProviderClass::RemoteModel) => {
                IntentPlanningOutcome::Llm
            }
            _ => IntentPlanningOutcome::Unknown,
        };
        self.planning_finished(outcome, None);
    }

    pub fn planning_finished(
        &self,
        outcome: IntentPlanningOutcome,
        reason: Option<IntentSafeReason>,
    ) {
        let mut state = self.0.lock();
        if state.terminal {
            state.mark_partial();
            return;
        }
        state.receipt.planning_outcome = outcome;
        if state.reliable {
            state.receipt.observation_reason = reason;
        }
        state.push_event(IntentExecutionEventKind::PlanningFinished { outcome });
    }

    pub fn action_started(&self) {
        let mut state = self.0.lock();
        if state.terminal {
            state.mark_partial();
            return;
        }
        state.push_event(IntentExecutionEventKind::ActionStarted {});
    }

    pub fn finish(&self, outcome: IntentActionOutcome, reason: Option<IntentSafeReason>) {
        self.record_outcome(outcome, reason);
        self.finalize();
    }

    /// Terminal action observation precedes the fallible audit/finalization await.
    pub fn record_outcome(&self, outcome: IntentActionOutcome, reason: Option<IntentSafeReason>) {
        let mut state = self.0.lock();
        if state.terminal {
            state.mark_partial();
            return;
        }
        if state
            .receipt
            .attempts
            .iter()
            .any(|a| a.outcome == IntentAttemptOutcome::Unknown)
        {
            state.mark_partial();
        }
        state.receipt.action_outcome = outcome;
        if reason.is_some() && state.reliable {
            state.receipt.observation_reason = reason;
        }
        state.push_event(IntentExecutionEventKind::ActionFinished { outcome });
        state.terminal = true;
    }

    pub fn finalize(&self) {
        let mut state = self.0.lock();
        if !state.terminal {
            state.mark_partial();
            return;
        }
        state.finalized = true;
    }

    pub fn snapshot(&self) -> IntentExecutionProvenance {
        let state = self.0.lock();
        let mut receipt = state.receipt.clone();
        receipt.observation = if !state.reliable || !state.finalized {
            IntentObservation::Partial
        } else if state.legacy {
            IntentObservation::LegacyUnknown
        } else {
            IntentObservation::Complete
        };
        receipt
    }
}

impl ObserverState {
    fn mark_partial(&mut self) {
        self.reliable = false;
        self.receipt.observation_reason = Some(IntentSafeReason::ObservationUnavailable);
    }

    fn push_event(&mut self, kind: IntentExecutionEventKind) {
        if self.receipt.events.len() >= MAX_INTENT_EVENTS {
            self.mark_partial();
            return;
        }
        self.receipt.events.push(IntentExecutionEvent {
            execution_id: self.receipt.execution_id,
            event_id: OpaqueIntentId::new(),
            sequence: self.receipt.events.len() as u32,
            observed_at: Utc::now(),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::{reconcile_intent_events, IntentSkillContextKind, IntentSkillUse};
    use super::*;
    use crate::error_codes::NetworkCode;

    fn observer() -> IntentExecutionObserver {
        IntentExecutionObserver::new(IntentSubmission::default())
    }

    fn successful_attempt(
        observer: &IntentExecutionObserver,
        provider: IntentProviderClass,
    ) -> OpaqueIntentId {
        let attempt = observer
            .begin_attempt(provider, IntentCapabilityObservation::default())
            .expect("attempt admitted");
        observer.end_attempt(Some(attempt), Ok(()));
        attempt
    }

    #[test]
    fn identity_is_created_before_planning_and_clones_share_only_their_execution() {
        let submission = IntentSubmission {
            submission_id: Some(OpaqueIntentId::new()),
            retry_of: Some(OpaqueIntentId::new()),
        };
        let first = IntentExecutionObserver::new(submission);
        let second = IntentExecutionObserver::new(submission);
        let receipt = first.snapshot();
        assert_eq!(first.execution_id(), receipt.execution_id);
        assert_ne!(first.execution_id(), second.execution_id());
        assert_eq!(receipt.submission_id, submission.submission_id);
        assert_eq!(receipt.retry_of, submission.retry_of);
        assert_eq!(receipt.storage, IntentObservationStorage::ProcessMemory);
        assert_eq!(receipt.config_revision, IntentRevision::Unknown);
        assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Unknown);
        assert_eq!(receipt.action_outcome, IntentActionOutcome::Unknown);
        assert_eq!(receipt.observation, IntentObservation::Partial);
        assert_eq!(receipt.events.len(), 1);
        assert_eq!(receipt.events[0].execution_id, first.execution_id());
        assert_eq!(receipt.events[0].sequence, 0);
        assert_eq!(receipt.events[0].kind, IntentExecutionEventKind::Started {});
        first
            .clone()
            .requested_provider(IntentProviderClass::RemoteModel);
        assert_eq!(
            first.snapshot().requested_provider_class,
            IntentProviderClass::RemoteModel
        );
        assert_eq!(
            second.snapshot().requested_provider_class,
            IntentProviderClass::Unknown
        );
    }

    #[test]
    fn primary_success_preserves_observed_capabilities_and_stage_order() {
        for provider in [
            IntentProviderClass::LocalModel,
            IntentProviderClass::RemoteModel,
        ] {
            let observer = observer();
            observer.requested_provider(provider);
            let capabilities = IntentCapabilityObservation {
                skills_requested: true,
                skills_context_forwarded: Some(true),
                skill_context_kind: IntentSkillContextKind::Activated,
                skills_effective: IntentSkillUse::Unknown,
            };
            let attempt = observer
                .begin_attempt(provider, capabilities)
                .expect("primary attempt");
            observer.end_attempt(Some(attempt), Ok(()));
            observer.planning_succeeded();
            observer.action_started();
            observer.finish(IntentActionOutcome::Completed, None);
            let receipt = observer.snapshot();
            assert_eq!(receipt.requested_provider_class, provider);
            assert_eq!(receipt.attempts.len(), 1);
            assert_eq!(
                receipt.attempts[0],
                IntentProviderAttempt {
                    attempt_id: attempt,
                    predecessor: None,
                    effective_provider_class: provider,
                    model_revision: IntentRevision::Unknown,
                    outcome: IntentAttemptOutcome::Succeeded,
                    reason: None,
                    capabilities,
                }
            );
            assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Llm);
            assert_eq!(receipt.action_outcome, IntentActionOutcome::Completed);
            assert_eq!(receipt.observation, IntentObservation::Complete);
            assert_eq!(receipt.observation_reason, None);
            assert_eq!(
                receipt
                    .events
                    .iter()
                    .map(|event| event.kind.clone())
                    .collect::<Vec<_>>(),
                vec![
                    IntentExecutionEventKind::Started {},
                    IntentExecutionEventKind::AttemptStarted {
                        attempt_id: attempt
                    },
                    IntentExecutionEventKind::AttemptFinished {
                        attempt_id: attempt
                    },
                    IntentExecutionEventKind::PlanningFinished {
                        outcome: IntentPlanningOutcome::Llm
                    },
                    IntentExecutionEventKind::ActionStarted {},
                    IntentExecutionEventKind::ActionFinished {
                        outcome: IntentActionOutcome::Completed
                    },
                ]
            );
            let (ordered, quality) = reconcile_intent_events(receipt.execution_id, &receipt.events);
            assert_eq!(ordered, receipt.events);
            assert_eq!(quality, IntentObservation::Complete);
            assert_eq!(
                receipt
                    .events
                    .iter()
                    .map(|event| event.event_id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                6
            );
        }
    }

    #[test]
    fn rule_fallback_keeps_the_failed_primary_and_safe_failure_reason() {
        let observer = observer();
        observer.requested_provider(IntentProviderClass::LocalModel);
        let first = observer
            .begin_attempt(
                IntentProviderClass::LocalModel,
                IntentCapabilityObservation::default(),
            )
            .expect("primary");
        let error = CoreError::Network {
            code: NetworkCode::Generic,
            message: "CANARY_TOKEN https://secret.local /private/key OCR_CANARY".into(),
        };
        observer.end_attempt(Some(first), Err(&error));
        let capabilities = IntentCapabilityObservation {
            skills_requested: true,
            skills_context_forwarded: Some(false),
            skills_effective: IntentSkillUse::Unsupported,
            ..Default::default()
        };
        let second = observer
            .begin_attempt(IntentProviderClass::Rule, capabilities)
            .expect("fallback");
        observer.end_attempt(Some(second), Ok(()));
        observer.planning_succeeded();
        observer.finish(IntentActionOutcome::Completed, None);
        let receipt = observer.snapshot();
        assert_eq!(receipt.planning_outcome, IntentPlanningOutcome::Rule);
        assert_eq!(receipt.observation, IntentObservation::Complete);
        assert_eq!(receipt.attempts.len(), 2);
        assert_eq!(
            receipt.attempts[0].outcome,
            IntentAttemptOutcome::Unavailable
        );
        assert_eq!(
            receipt.attempts[0].reason,
            Some(IntentSafeReason::Unavailable)
        );
        assert_eq!(receipt.attempts[1].predecessor, Some(first));
        assert_eq!(
            receipt.attempts[1].model_revision,
            IntentRevision::NotApplicable
        );
        assert_eq!(receipt.attempts[1].capabilities, capabilities);
        let wire = serde_json::to_string(&receipt).expect("receipt JSON");
        for forbidden in [
            "CANARY",
            "secret.local",
            "/private",
            "endpoint",
            "message",
            "prompt",
        ] {
            assert!(
                !wire.contains(forbidden),
                "forbidden observation field: {forbidden}"
            );
        }
    }

    #[test]
    fn configuration_is_frozen_without_rejecting_identical_observations() {
        let reason = Some(IntentConfigurationReason::RuleOnlyDegradation);
        let same = observer();
        same.configure(IntentProviderClass::LocalModel, reason);
        same.configure(IntentProviderClass::LocalModel, reason);
        same.finish(IntentActionOutcome::NotStarted, None);
        let receipt = same.snapshot();
        assert_eq!(
            receipt.requested_provider_class,
            IntentProviderClass::LocalModel
        );
        assert_eq!(receipt.configuration_reason, reason);
        assert_eq!(receipt.observation, IntentObservation::Complete);
        for (provider, changed_reason) in [
            (IntentProviderClass::Rule, reason),
            (IntentProviderClass::LocalModel, None),
        ] {
            let changed = observer();
            changed.configure(IntentProviderClass::LocalModel, reason);
            changed.configure(provider, changed_reason);
            changed.finish(IntentActionOutcome::NotStarted, None);
            let receipt = changed.snapshot();
            assert_eq!(
                receipt.requested_provider_class,
                IntentProviderClass::LocalModel
            );
            assert_eq!(receipt.configuration_reason, reason);
            assert_eq!(receipt.observation, IntentObservation::Partial);
            assert_eq!(
                receipt.observation_reason,
                Some(IntentSafeReason::ObservationUnavailable)
            );
        }
    }

    #[test]
    fn terminal_action_is_partial_until_finalization_and_legacy_stays_distinct() {
        let observer = observer();
        observer.record_outcome(IntentActionOutcome::Denied, Some(IntentSafeReason::Denied));
        let before = observer.snapshot();
        assert_eq!(before.action_outcome, IntentActionOutcome::Denied);
        assert_eq!(before.observation, IntentObservation::Partial);
        assert_eq!(before.observation_reason, Some(IntentSafeReason::Denied));
        observer.finalize();
        assert_eq!(observer.snapshot().observation, IntentObservation::Complete);
        observer.mark_legacy();
        assert_eq!(
            observer.snapshot().observation,
            IntentObservation::LegacyUnknown
        );
        observer.mark_partial();
        let partial = observer.snapshot();
        assert_eq!(partial.observation, IntentObservation::Partial);
        assert_eq!(
            partial.observation_reason,
            Some(IntentSafeReason::ObservationUnavailable)
        );
        assert_eq!(partial.events, before.events);
    }

    #[test]
    fn early_finalization_and_unfinished_attempts_cannot_claim_complete() {
        let early = observer();
        early.finalize();
        assert_eq!(
            early.snapshot().action_outcome,
            IntentActionOutcome::Unknown
        );
        early.finish(IntentActionOutcome::Completed, None);
        assert_eq!(early.snapshot().observation, IntentObservation::Partial);
        let interrupted = observer();
        interrupted
            .begin_attempt(
                IntentProviderClass::RemoteModel,
                IntentCapabilityObservation::default(),
            )
            .expect("attempt");
        interrupted.finish(
            IntentActionOutcome::Failed,
            Some(IntentSafeReason::ActionFailed),
        );
        let receipt = interrupted.snapshot();
        assert_eq!(receipt.attempts[0].outcome, IntentAttemptOutcome::Unknown);
        assert_eq!(receipt.action_outcome, IntentActionOutcome::Failed);
        assert_eq!(receipt.observation, IntentObservation::Partial);
        assert_eq!(
            receipt.observation_reason,
            Some(IntentSafeReason::ObservationUnavailable)
        );
    }

    #[test]
    fn attempt_limit_keeps_all_admitted_attempts_without_appending_an_overflow() {
        let observer = observer();
        let mut prior = None;
        for index in 0..MAX_INTENT_ATTEMPTS {
            let attempt = successful_attempt(&observer, IntentProviderClass::Rule);
            assert_eq!(observer.snapshot().attempts[index].predecessor, prior);
            prior = Some(attempt);
        }
        let before = observer.snapshot();
        assert_eq!(
            observer.begin_attempt(
                IntentProviderClass::Rule,
                IntentCapabilityObservation::default()
            ),
            None
        );
        assert_eq!(observer.snapshot().attempts, before.attempts);
        assert_eq!(observer.snapshot().events, before.events);
        observer.finish(IntentActionOutcome::Completed, None);
        assert_eq!(observer.snapshot().observation, IntentObservation::Partial);
        assert_eq!(observer.snapshot().attempts.len(), MAX_INTENT_ATTEMPTS);
    }

    #[test]
    fn duplicate_and_foreign_completions_preserve_the_original_attempt() {
        let duplicate = observer();
        let id = successful_attempt(&duplicate, IntentProviderClass::LocalModel);
        let before = duplicate.snapshot();
        let timeout = CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 10,
        };
        duplicate.end_attempt(Some(id), Err(&timeout));
        assert_eq!(duplicate.snapshot().attempts, before.attempts);
        assert_eq!(duplicate.snapshot().events, before.events);
        duplicate.finish(IntentActionOutcome::Completed, None);
        assert_eq!(duplicate.snapshot().observation, IntentObservation::Partial);

        let foreign = observer();
        let id = foreign
            .begin_attempt(
                IntentProviderClass::LocalModel,
                IntentCapabilityObservation::default(),
            )
            .expect("owned attempt");
        let before = foreign.snapshot();
        foreign.end_attempt(Some(OpaqueIntentId::new()), Ok(()));
        assert_eq!(foreign.snapshot().attempts, before.attempts);
        assert_eq!(foreign.snapshot().events, before.events);
        foreign.end_attempt(Some(id), Ok(()));
        foreign.finish(IntentActionOutcome::Completed, None);
        assert_eq!(foreign.snapshot().observation, IntentObservation::Partial);
        assert_eq!(
            foreign.snapshot().attempts[0].outcome,
            IntentAttemptOutcome::Succeeded
        );
    }

    #[test]
    fn absent_attempt_completion_changes_nothing() {
        let observer = observer();
        let before = observer.snapshot();
        observer.end_attempt(None, Ok(()));
        assert_eq!(observer.snapshot(), before);
        observer.finish(IntentActionOutcome::NotStarted, None);
        assert_eq!(observer.snapshot().observation, IntentObservation::Complete);
    }

    #[test]
    fn late_updates_cannot_rewrite_terminal_outcomes_or_append_events() {
        let late_updates: [fn(&IntentExecutionObserver); 6] = [
            |observer| observer.configure(IntentProviderClass::LocalModel, None),
            |observer| {
                assert_eq!(
                    observer.begin_attempt(
                        IntentProviderClass::Rule,
                        IntentCapabilityObservation::default()
                    ),
                    None
                );
            },
            |observer| {
                observer.end_attempt(Some(observer.snapshot().attempts[0].attempt_id), Ok(()))
            },
            |observer| {
                observer.planning_finished(
                    IntentPlanningOutcome::Denied,
                    Some(IntentSafeReason::Denied),
                )
            },
            IntentExecutionObserver::action_started,
            |observer| {
                observer.record_outcome(
                    IntentActionOutcome::Failed,
                    Some(IntentSafeReason::ActionFailed),
                )
            },
        ];
        for late_update in late_updates {
            let observer = observer();
            observer.requested_provider(IntentProviderClass::LocalModel);
            successful_attempt(&observer, IntentProviderClass::LocalModel);
            observer.planning_succeeded();
            observer.finish(IntentActionOutcome::Completed, None);
            let mut expected = observer.snapshot();
            expected.observation = IntentObservation::Partial;
            expected.observation_reason = Some(IntentSafeReason::ObservationUnavailable);
            late_update(&observer);
            assert_eq!(observer.snapshot(), expected);
        }
    }

    #[test]
    fn planning_uses_the_latest_successful_observed_provider() {
        let observer = observer();
        successful_attempt(&observer, IntentProviderClass::RemoteModel);
        let failed = observer
            .begin_attempt(
                IntentProviderClass::Rule,
                IntentCapabilityObservation::default(),
            )
            .expect("later attempt");
        observer.end_attempt(
            Some(failed),
            Err(&CoreError::RequestTimeout {
                code: NetworkCode::Timeout,
                timeout_ms: 10,
            }),
        );
        observer.planning_succeeded();
        assert_eq!(
            observer.snapshot().planning_outcome,
            IntentPlanningOutcome::Llm
        );
        let unknown = IntentExecutionObserver::new(IntentSubmission::default());
        unknown.planning_succeeded();
        assert_eq!(
            unknown.snapshot().planning_outcome,
            IntentPlanningOutcome::Unknown
        );
    }

    #[test]
    fn reason_retention_does_not_hide_planning_failure_or_observation_loss() {
        let failed = observer();
        failed.planning_finished(
            IntentPlanningOutcome::Error,
            Some(IntentSafeReason::Unavailable),
        );
        failed.finish(IntentActionOutcome::NotStarted, None);
        assert_eq!(
            failed.snapshot().observation_reason,
            Some(IntentSafeReason::Unavailable)
        );
        assert_eq!(
            failed.snapshot().planning_outcome,
            IntentPlanningOutcome::Error
        );
        assert_eq!(failed.snapshot().observation, IntentObservation::Complete);

        let partial = observer();
        partial.mark_partial();
        partial.planning_finished(
            IntentPlanningOutcome::Denied,
            Some(IntentSafeReason::Denied),
        );
        partial.finish(IntentActionOutcome::Denied, Some(IntentSafeReason::Denied));
        assert_eq!(
            partial.snapshot().observation_reason,
            Some(IntentSafeReason::ObservationUnavailable)
        );
        assert_eq!(partial.snapshot().observation, IntentObservation::Partial);
    }

    #[test]
    fn event_limit_preserves_the_bounded_prefix_and_reports_observation_loss() {
        let observer = observer();
        for _ in 1..MAX_INTENT_EVENTS {
            observer.action_started();
        }
        let before = observer.snapshot();
        assert_eq!(before.events.len(), MAX_INTENT_EVENTS);
        assert_eq!(before.observation_reason, None);
        observer.action_started();
        observer.finish(IntentActionOutcome::Completed, None);
        let receipt = observer.snapshot();
        assert_eq!(receipt.events, before.events);
        assert_eq!(receipt.action_outcome, IntentActionOutcome::Completed);
        assert_eq!(receipt.observation, IntentObservation::Partial);
        assert_eq!(
            receipt.observation_reason,
            Some(IntentSafeReason::ObservationUnavailable)
        );
    }
}
