//! Local advisory runtime. No execution capabilities or production UI wiring.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use maekon_core::config::AiAccessMode;
use maekon_core::models::candidate_assessment::*;
use maekon_core::models::candidate_decision::{
    digest_bytes, CandidateDecisionRequest, DecisionBinding, DecisionOption, DecisionText,
    DecisionUnavailable,
};
use maekon_core::models::candidate_decision_policy::*;
use maekon_core::ports::candidate_assessment::{
    CandidateAssessmentAuditPort, CandidateAssessmentPort, CandidateAttemptGuard,
    LocalCandidateModelPort,
};
use parking_lot::Mutex;
use uuid::Uuid;

use super::{local_exact_label_choice as exact_label_choice, ExternalOcrPrivacyGuard};

/// Trusted composition evidence; never persisted as automatic restart permission.
#[derive(Clone)]
pub struct LocalAssessmentApproval {
    pub approval_reference: String,
    pub principal_namespace: String,
    pub policy_revision: String,
    pub configuration_revision: String,
    pub trigger: CandidateTrigger,
    pub max_attempts: u32,
    pub expires_at: Instant,
    pub model: Option<LocalModelApproval>,
}

impl LocalAssessmentApproval {
    fn validate(&self, now: Instant) -> Result<(), DecisionUnavailable> {
        if [
            &self.approval_reference,
            &self.principal_namespace,
            &self.policy_revision,
            &self.configuration_revision,
        ]
        .iter()
        .any(|v| v.trim().is_empty() || v.len() > 256)
            || self.max_attempts == 0
            || self.max_attempts > 1000
            || now >= self.expires_at
            || self.expires_at.duration_since(now) > Duration::from_secs(3600)
        {
            return Err(DecisionUnavailable::ApprovalMissing);
        }
        if let Some(model) = &self.model {
            model.validate(now)?;
        }
        Ok(())
    }
}

/// Instant does not guarantee that suspended time elapses. Keep the original
/// wall-clock bound as well; binding and cache hits must never renew this anchor.
struct ClockFence {
    monotonic: Instant,
    wall: SystemTime,
    last_wall: SystemTime,
}

impl Default for ClockFence {
    fn default() -> Self {
        let wall = SystemTime::now();
        Self {
            monotonic: Instant::now(),
            wall,
            last_wall: wall,
        }
    }
}

impl ClockFence {
    fn wall_at(&self, instant: Instant) -> Option<SystemTime> {
        if instant >= self.monotonic {
            self.wall
                .checked_add(instant.duration_since(self.monotonic))
        } else {
            self.wall
                .checked_sub(self.monotonic.duration_since(instant))
        }
    }

    fn check(&mut self, deadline: Instant, now: SystemTime) -> Result<(), DecisionUnavailable> {
        let deadline = self.wall_at(deadline).ok_or(DecisionUnavailable::Expired)?;
        if now < self.last_wall || now >= deadline {
            return Err(DecisionUnavailable::Expired);
        }
        self.last_wall = now;
        Ok(())
    }
}

#[derive(Default)]
struct State {
    clock: ClockFence,
    epoch: u64,
    revoked: bool,
    approval: Option<LocalAssessmentApproval>,
    binding: Option<DecisionBinding>,
    remaining_attempts: u32,
    authority_revision: Option<String>,
    cache: Option<(String, CandidateAssessmentResult)>,
}

impl State {
    fn advance(&mut self) -> Result<(), DecisionUnavailable> {
        if self.revoked {
            return Err(DecisionUnavailable::Cancelled);
        }
        self.epoch = self.epoch.checked_add(1).ok_or_else(|| {
            self.revoked = true;
            self.cache = None;
            DecisionUnavailable::Cancelled
        })?;
        Ok(())
    }

    fn check(
        &mut self,
        request: &CandidateDecisionRequest,
        epoch: u64,
        now: Instant,
    ) -> Result<(), DecisionUnavailable> {
        self.check_at(request, epoch, now, SystemTime::now())
    }

    fn check_at(
        &mut self,
        request: &CandidateDecisionRequest,
        epoch: u64,
        now: Instant,
        wall: SystemTime,
    ) -> Result<(), DecisionUnavailable> {
        if self.revoked {
            return Err(DecisionUnavailable::Cancelled);
        }
        let approval = self.approval.as_ref().ok_or(DecisionUnavailable::Off)?;
        if self.epoch != epoch || self.binding.as_ref() != Some(request.binding()) {
            return Err(DecisionUnavailable::Stale);
        }
        if now < request.binding().observed_at
            || now >= request.binding().deadline
            || now >= approval.expires_at
            || approval
                .model
                .as_ref()
                .is_some_and(|model| now >= model.expires_at)
        {
            return Err(DecisionUnavailable::Expired);
        }
        let deadline = request.binding().deadline.min(approval.expires_at).min(
            approval
                .model
                .as_ref()
                .map_or(approval.expires_at, |model| model.expires_at),
        );
        if let Err(reason) = self.clock.check(deadline, wall) {
            self.revoked = true;
            self.cache = None;
            return Err(reason);
        }
        Ok(())
    }
}

/// Bind on every observed change, including A -> B -> A. Revocation is permanent
/// across clones. Account/model/configuration/policy changes need a new runtime.
#[derive(Clone, Default)]
pub struct LocalAssessmentControl {
    state: Arc<Mutex<State>>,
}

impl LocalAssessmentControl {
    /// Inject the last observed wall time for cross-adapter rollback tests.
    /// Production callers always observe the real clock through `checkpoint`.
    #[cfg(test)]
    pub(super) fn set_clock_observation_for_test(&self, wall: SystemTime) {
        self.state.lock().clock.last_wall = wall;
    }

    pub fn approve(&self, approval: LocalAssessmentApproval) -> Result<(), DecisionUnavailable> {
        approval.validate(Instant::now())?;
        let mut state = self.state.lock();
        if state.approval.is_some() {
            return Err(DecisionUnavailable::Cancelled);
        }
        state.advance()?;
        state.remaining_attempts = approval.max_attempts;
        state.approval = Some(approval);
        Ok(())
    }

    pub fn bind(&self, request: &CandidateDecisionRequest) -> Result<(), DecisionUnavailable> {
        let mut state = self.state.lock();
        state.advance()?;
        state.binding = Some(request.binding().clone());
        state.cache = None;
        Ok(())
    }

    pub fn revoke(&self) {
        let mut state = self.state.lock();
        state.revoked = true;
        state.epoch = state.epoch.saturating_add(1);
        state.cache = None;
    }
}

/// An opaque lifecycle observation for one binding epoch. This does not grant
/// data access, egress, inference, or execution permission.
#[derive(Clone)]
pub struct LocalAssessmentLease {
    state: Arc<Mutex<State>>,
    epoch: u64,
    approval: LocalAssessmentApproval,
}

impl LocalAssessmentLease {
    pub fn approval(&self) -> &LocalAssessmentApproval {
        &self.approval
    }
}

impl LocalAssessmentControl {
    /// Capture the current approval only when the request is still bound.
    pub fn lease(
        &self,
        request: &CandidateDecisionRequest,
    ) -> Result<LocalAssessmentLease, DecisionUnavailable> {
        let mut state = self.state.lock();
        let epoch = state.epoch;
        state.check(request, epoch, Instant::now())?;
        Ok(LocalAssessmentLease {
            state: self.state.clone(),
            epoch,
            approval: state.approval.clone().ok_or(DecisionUnavailable::Off)?,
        })
    }

    /// Revalidate a lifecycle observation; privacy and audit gates remain separate.
    pub fn checkpoint(
        &self,
        request: &CandidateDecisionRequest,
        lease: &LocalAssessmentLease,
    ) -> Result<(), DecisionUnavailable> {
        if !Arc::ptr_eq(&self.state, &lease.state) {
            return Err(DecisionUnavailable::Stale);
        }
        self.state
            .lock()
            .check(request, lease.epoch, Instant::now())
    }

    /// Reserve once under the lifecycle lock. Cancellation never refunds quota.
    pub fn reserve_attempt(
        &self,
        request: &CandidateDecisionRequest,
        lease: &LocalAssessmentLease,
    ) -> Result<(), DecisionUnavailable> {
        self.checkpoint(request, lease)?;
        let mut state = self.state.lock();
        state.check(request, lease.epoch, Instant::now())?;
        state.remaining_attempts = state
            .remaining_attempts
            .checked_sub(1)
            .ok_or(DecisionUnavailable::BudgetExceeded)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "local_candidate_control_tests.rs"]
mod control_tests;

pub(super) struct LocalCandidateAssessment {
    policy: CandidateDecisionPolicy,
    mode: AiAccessMode,
    privacy: ExternalOcrPrivacyGuard,
    audit: Arc<dyn CandidateAssessmentAuditPort>,
    model: Option<Arc<dyn LocalCandidateModelPort>>,
    control: LocalAssessmentControl,
}

impl LocalCandidateAssessment {
    pub(super) fn new(
        policy: CandidateDecisionPolicy,
        mode: AiAccessMode,
        privacy: ExternalOcrPrivacyGuard,
        audit: Arc<dyn CandidateAssessmentAuditPort>,
        model: Option<Arc<dyn LocalCandidateModelPort>>,
    ) -> (Self, LocalAssessmentControl) {
        let control = LocalAssessmentControl::default();
        (
            Self {
                policy,
                mode,
                privacy,
                audit,
                model,
                control: control.clone(),
            },
            control,
        )
    }

    fn provider(&self) -> Result<CandidateProvider, DecisionUnavailable> {
        match self.policy.provider {
            None => Err(DecisionUnavailable::Off),
            Some(provider @ (CandidateProvider::LocalRules | CandidateProvider::LocalModel)) => {
                Ok(provider)
            }
            Some(_) => Err(DecisionUnavailable::Rejected),
        }
    }

    fn prepare_text(
        &self,
        request: &CandidateDecisionRequest,
    ) -> Result<(DecisionText, bool), DecisionUnavailable> {
        let sanitize = |text: &str| self.privacy.sanitize_candidate_text(text);
        let snapshot = request.snapshot();
        let text = DecisionText {
            goal: sanitize(&snapshot.goal),
            candidates: snapshot
                .candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| {
                    let element = &candidate.element;
                    DecisionOption {
                        id: format!("c{index}"),
                        text: sanitize(&element.label),
                        role: element.role.as_deref().map(sanitize),
                        intent: element.intent.as_deref().map(sanitize),
                        state: element.state.as_deref().map(sanitize),
                    }
                })
                .collect(),
        };
        text.validate()?;
        // Redaction must not manufacture an exact match between different labels.
        let unchanged = text.goal == snapshot.goal
            && text
                .candidates
                .iter()
                .zip(&snapshot.candidates)
                .all(|(sanitized, raw)| sanitized.text == raw.element.label);
        Ok((text, unchanged))
    }

    async fn evaluate(
        &self,
        request: &CandidateDecisionRequest,
        result: &mut CandidateAssessmentResult,
    ) -> Result<(), DecisionUnavailable> {
        let provider = self.provider()?;
        let lease = self.control.lease(request)?;
        let epoch = lease.epoch;
        let approval = lease.approval.clone();
        let ready = provider == CandidateProvider::LocalRules || self.model.is_some();
        self.policy
            .check_eligibility(
                CandidateProviderObservation {
                    provider,
                    readiness: if ready {
                        CandidateReadiness::Ready
                    } else {
                        CandidateReadiness::Unavailable
                    },
                    authentication: CandidateAuthentication::NotRequired,
                    billing: CandidateBilling::Local,
                },
                CandidatePolicyContext {
                    access_mode: self.mode,
                    trigger: approval.trigger,
                    permitted: true,
                },
                Instant::now(),
            )
            .map_err(|_| DecisionUnavailable::Rejected)?;
        if provider == CandidateProvider::LocalModel {
            approval
                .model
                .as_ref()
                .ok_or(DecisionUnavailable::ApprovalMissing)?
                .validate(Instant::now())?;
        }
        let revision = self.privacy.candidate_consent_revision()?;
        {
            let mut state = self.control.state.lock();
            state.check(request, epoch, Instant::now())?;
            if state
                .authority_revision
                .as_ref()
                .is_some_and(|pinned| pinned != &revision)
            {
                state.revoked = true;
                state.cache = None;
                return Err(DecisionUnavailable::ConsentOrPolicyDenied);
            }
            state.authority_revision = Some(revision.clone());
        }
        let context = Evaluation {
            runtime: self,
            request,
            epoch,
            revision: &revision,
        };
        context.checkpoint().await?;
        let (text, unchanged) = self.prepare_text(request)?;
        let bytes = serde_json::to_vec(&text).map_err(|_| DecisionUnavailable::InvalidInput)?;
        result.sanitized_state_hash = Some(digest_bytes(&bytes));
        let namespace = digest_bytes(
            &serde_json::to_vec(&(
                ASSESSMENT_SCHEMA,
                provider,
                request.binding().digest.as_str(),
                epoch,
                &revision,
                &text,
                &approval.approval_reference,
                &approval.principal_namespace,
                &approval.policy_revision,
                &approval.configuration_revision,
                approval.model.as_ref().map(|model| {
                    (
                        &model.daemon_reference,
                        &model.configuration_revision,
                        &model.endpoint_origin,
                        &model.model,
                        &model.model_digest,
                    )
                }),
            ))
            .map_err(|_| DecisionUnavailable::InvalidInput)?,
        );

        // Model output is deliberately not cached: a daemon/model observation is
        // live evidence, not a lease that a cache hit can silently renew.
        let cached = if provider == CandidateProvider::LocalRules {
            self.control
                .state
                .lock()
                .cache
                .as_ref()
                .filter(|(key, _)| key == &namespace)
                .map(|(_, cached)| cached.clone())
        } else {
            None
        };
        if let Some(mut cached) = cached {
            let source = cached.decision_id;
            cached.decision_id = result.decision_id;
            cached.cache_source = Some(source);
            cached.attempts.clear();
            self.record(
                &context,
                &namespace,
                provider,
                &cached,
                AssessmentAuditPhase::CacheHit,
                None,
            )
            .await?;
            context.checkpoint().await?;
            self.publish(&context, &namespace, &cached, false)?;
            *result = cached;
            return Ok(());
        }

        context.checkpoint().await?;
        self.control.reserve_attempt(request, &lease)?;
        let mut attempt = LocalAssessmentAttempt {
            id: Uuid::new_v4(),
            request_hash: result.sanitized_state_hash.clone(),
            attempted: false,
            elapsed_ms: 0,
            usage: None,
            failure: None,
        };
        result
            .attempts
            .push(AssessmentAttempt::Local(attempt.clone()));
        self.record(
            &context,
            &namespace,
            provider,
            result,
            AssessmentAuditPhase::BeforeAttempt,
            Some(attempt.clone()),
        )
        .await?;
        context.checkpoint().await?;
        let started = Instant::now();
        let decision = if provider == CandidateProvider::LocalRules {
            result.provenance = Some(AssessmentProvenance::local_rules());
            let (choice, matches) = exact_label_choice(&text, unchanged);
            result.evidence = AssessmentEvidence::LocalHeuristic {
                exact_matches: matches,
            };
            attempt.attempted = true;
            Ok(choice)
        } else {
            let model = self.model.as_ref().ok_or(DecisionUnavailable::Rejected)?;
            let approval = approval
                .model
                .as_ref()
                .ok_or(DecisionUnavailable::ApprovalMissing)?;
            // Conservatively retain a pending provider invocation if cancelled.
            // A returned failure refines whether the inference POST was attempted.
            attempt.attempted = true;
            result.attempts[0] = AssessmentAttempt::Local(attempt.clone());
            match model.infer(&text, approval, &context).await {
                Ok(answer) => {
                    attempt.request_hash = Some(answer.request_hash);
                    attempt.usage = answer.usage;
                    if answer.observed_model != approval.model
                        || answer.model_digest != approval.model_digest
                    {
                        Err(DecisionUnavailable::InvalidResponse)
                    } else {
                        result.provenance = Some(AssessmentProvenance {
                            provider,
                            transport: AssessmentTransport::LoopbackHttp,
                            inference_provider: "ollama".into(),
                            requested_model: Some(approval.model.clone()),
                            observed_model: Some(answer.observed_model),
                            model_revision: Some(answer.model_digest),
                            rubric_revision: ASSESSMENT_SCHEMA.into(),
                        });
                        result.evidence = AssessmentEvidence::ModelReported;
                        Ok(answer.choice)
                    }
                }
                Err(error) => {
                    attempt.request_hash = error.request_hash;
                    attempt.usage = error.usage;
                    attempt.attempted = error.attempted;
                    Err(error.reason)
                }
            }
        };
        let decision = decision.and_then(|choice| map_choice(request, &text, choice));
        attempt.elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        attempt.failure = decision.as_ref().err().copied();
        result.attempts[0] = AssessmentAttempt::Local(attempt.clone());
        // Commit observed attempt metadata even when authority changed during it.
        // This never permits a new inference or selection publication.
        self.audit
            .record_assessment(audit_record(
                &namespace,
                provider,
                result,
                AssessmentAuditPhase::AfterAttempt,
                Some(attempt),
            ))
            .await
            .map_err(|_| DecisionUnavailable::AuditUnavailable)?;
        context.checkpoint().await?;
        result.outcome = decision?;
        self.publish(
            &context,
            &namespace,
            result,
            provider == CandidateProvider::LocalRules,
        )?;
        Ok(())
    }

    async fn record(
        &self,
        context: &Evaluation<'_>,
        namespace: &str,
        provider: CandidateProvider,
        result: &CandidateAssessmentResult,
        phase: AssessmentAuditPhase,
        attempt: Option<LocalAssessmentAttempt>,
    ) -> Result<(), DecisionUnavailable> {
        context.checkpoint().await?;
        self.audit
            .record_assessment(audit_record(namespace, provider, result, phase, attempt))
            .await
            .map_err(|_| DecisionUnavailable::AuditUnavailable)?;
        context.checkpoint().await
    }

    fn publish(
        &self,
        context: &Evaluation<'_>,
        namespace: &str,
        result: &CandidateAssessmentResult,
        cache: bool,
    ) -> Result<(), DecisionUnavailable> {
        let mut state = self.control.state.lock();
        state.check(context.request, context.epoch, Instant::now())?;
        if self.privacy.candidate_consent_revision()? != context.revision {
            return Err(DecisionUnavailable::ConsentOrPolicyDenied);
        }
        if cache {
            state.cache = Some((namespace.into(), result.clone()));
        }
        Ok(())
    }
}

struct Evaluation<'a> {
    runtime: &'a LocalCandidateAssessment,
    request: &'a CandidateDecisionRequest,
    epoch: u64,
    revision: &'a str,
}

impl Evaluation<'_> {
    fn check_sync(&self) -> Result<(), DecisionUnavailable> {
        self.runtime
            .control
            .state
            .lock()
            .check(self.request, self.epoch, Instant::now())?;
        if self.runtime.privacy.candidate_consent_revision()? != self.revision {
            return Err(DecisionUnavailable::ConsentOrPolicyDenied);
        }
        Ok(())
    }
}

#[async_trait]
impl CandidateAttemptGuard for Evaluation<'_> {
    async fn checkpoint(&self) -> Result<(), DecisionUnavailable> {
        self.check_sync()?;
        let (revision, window) = self.runtime.privacy.local_candidate_authority().await?;
        self.check_sync()?;
        if revision != self.revision {
            return Err(DecisionUnavailable::ConsentOrPolicyDenied);
        }
        if window != self.request.binding().window_digest {
            return Err(DecisionUnavailable::Stale);
        }
        Ok(())
    }
}

#[async_trait]
impl CandidateAssessmentPort for LocalCandidateAssessment {
    async fn assess(&self, request: &CandidateDecisionRequest) -> CandidateAssessmentResult {
        let mut result =
            CandidateAssessmentResult::unavailable(request.binding(), DecisionUnavailable::Off);
        let outcome = tokio::time::timeout_at(
            request.binding().deadline.into(),
            self.evaluate(request, &mut result),
        )
        .await;
        let failure = match outcome {
            Ok(Ok(())) => None,
            Ok(Err(reason)) => Some(reason),
            Err(_) => Some(DecisionUnavailable::Timeout),
        };
        if let Some(reason) = failure {
            result.outcome = AssessmentOutcome::Unavailable(reason);
            if let Some(AssessmentAttempt::Local(attempt)) = result.attempts.last_mut() {
                if attempt.failure.is_none() {
                    attempt.failure = Some(reason);
                }
            }
        }
        result
    }
}

fn audit_record(
    namespace: &str,
    provider: CandidateProvider,
    result: &CandidateAssessmentResult,
    phase: AssessmentAuditPhase,
    attempt: Option<LocalAssessmentAttempt>,
) -> AssessmentAuditRecord {
    AssessmentAuditRecord {
        id: Uuid::new_v4(),
        decision_id: result.decision_id,
        attempt_id: attempt.as_ref().map(|attempt| attempt.id),
        phase,
        namespace_hash: namespace.into(),
        provider,
        transport: if provider == CandidateProvider::LocalRules {
            AssessmentTransport::InProcess
        } else {
            AssessmentTransport::LoopbackHttp
        },
        schema_revision: ASSESSMENT_SCHEMA,
        attempt,
        cache_source: result.cache_source,
    }
}

fn map_choice(
    request: &CandidateDecisionRequest,
    text: &DecisionText,
    choice: LocalAssessmentChoice,
) -> Result<AssessmentOutcome, DecisionUnavailable> {
    match choice {
        LocalAssessmentChoice::Selected(id) => {
            let index = text
                .candidates
                .iter()
                .position(|candidate| candidate.id == id)
                .ok_or(DecisionUnavailable::InvalidResponse)?;
            let candidate = request
                .snapshot()
                .candidates
                .get(index)
                .ok_or(DecisionUnavailable::Stale)?;
            Ok(AssessmentOutcome::Selected {
                candidate_id: candidate.element.element_id.clone(),
            })
        }
        LocalAssessmentChoice::None => Ok(AssessmentOutcome::None),
        LocalAssessmentChoice::Delegate => Ok(AssessmentOutcome::Delegate),
    }
}

#[cfg(test)]
#[path = "local_candidate_assessment_tests.rs"]
mod tests;
