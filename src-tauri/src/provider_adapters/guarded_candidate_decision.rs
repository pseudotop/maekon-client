//! Default-off shadow composition. No execution tickets or production UI consumer.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use maekon_core::config::AiAccessMode;
use maekon_core::models::candidate_decision::{
    digest_bytes, BackendFailure, BackendResponse, CandidateDecision, CandidateDecisionRequest,
    CandidateDecisionResult, DecisionAttempt, DecisionAuditContext, DecisionAuditPhase,
    DecisionAuditRecord, DecisionBinding, DecisionOption, DecisionStage, DecisionText,
    DecisionUnavailable, DecisionUsage, DECISION_RUBRIC, DECISION_SCHEMA, DELEGATE_OPTION,
    JEV_ENDPOINT, JEV_MODEL, NONE_OPTION,
};
use maekon_core::ports::candidate_decision::{
    CandidateDecisionAuditPort, CandidateDecisionBackendPort, CandidateDecisionPort,
};
use maekon_core::ports::secret_store::{provider_api_key_secret_ref, SecretStore};
use parking_lot::Mutex;
use uuid::Uuid;

use super::ExternalOcrPrivacyGuard;

/// Trusted composition evidence, deliberately not deserializable from IPC/config.
/// Issuing this value does not replace provider/account/experiment approval.
#[derive(Clone)]
pub struct CandidateShadowApproval {
    pub approval_reference: String,
    pub principal_namespace: String,
    pub credential_profile: String,
    pub credential_revision: String,
    pub policy_revision: String,
    pub price_reference: String,
    pub input_microusd_per_million: u64,
    pub output_microusd_per_million: u64,
    pub budget_microusd: u64,
    pub max_attempts: u32,
    pub expires_at: Instant,
}

impl CandidateShadowApproval {
    fn validate(&self, now: Instant) -> Result<(), DecisionUnavailable> {
        if [
            &self.approval_reference,
            &self.principal_namespace,
            &self.credential_profile,
            &self.credential_revision,
            &self.policy_revision,
            &self.price_reference,
        ]
        .iter()
        .any(|value| value.trim().is_empty() || value.len() > 256)
            || self.budget_microusd == 0
            || self.max_attempts == 0
            || self.max_attempts > 1000
            || self.expires_at <= now
            || self.expires_at.duration_since(now) > Duration::from_secs(3600)
            || self
                .cost(DecisionUsage {
                    input_tokens: 65_536,
                    output_tokens: 65_536,
                })
                .is_none()
            || provider_api_key_secret_ref("typesafe", &self.credential_profile).is_err()
        {
            return Err(DecisionUnavailable::ApprovalMissing);
        }
        Ok(())
    }

    fn cost(&self, usage: DecisionUsage) -> Option<u64> {
        let total = u128::from(usage.input_tokens)
            .checked_mul(u128::from(self.input_microusd_per_million))?
            .checked_add(
                u128::from(usage.output_tokens)
                    .checked_mul(u128::from(self.output_microusd_per_million))?,
            )?;
        u64::try_from(total.div_ceil(1_000_000)).ok()
    }
}

#[derive(Default)]
struct State {
    epoch: u64,
    revoked: bool,
    approval: Option<CandidateShadowApproval>,
    binding: Option<DecisionBinding>,
    credential_value: Option<String>,
    remaining_budget: u64,
    remaining_attempts: u32,
    cache: Option<(String, CandidateDecisionResult)>,
}

impl State {
    // Called under the same lock as publication: an earlier authority check
    // cannot protect against a concurrent bind/revoke before this write.
    fn cache_completed(
        &mut self,
        epoch: u64,
        key: String,
        result: &CandidateDecisionResult,
    ) -> Result<(), DecisionUnavailable> {
        if self.epoch != epoch || self.revoked {
            return Err(DecisionUnavailable::Stale);
        }
        self.cache = Some((key, result.clone()));
        Ok(())
    }

    fn advance(&mut self) -> Result<(), DecisionUnavailable> {
        if self.revoked {
            return Err(DecisionUnavailable::Cancelled);
        }
        self.epoch = self.epoch.checked_add(1).ok_or_else(|| {
            self.revoked = true;
            self.credential_value = None;
            self.cache = None;
            DecisionUnavailable::Cancelled
        })?;
        Ok(())
    }
}

/// A default control is off. Keep this handle in trusted composition, separate
/// from advisory consumers. Revocation is permanent, including across clones.
#[derive(Clone, Default)]
pub struct CandidateDecisionControl {
    state: Arc<Mutex<State>>,
}

impl CandidateDecisionControl {
    pub fn approve_shadow(
        &self,
        approval: CandidateShadowApproval,
    ) -> Result<(), DecisionUnavailable> {
        approval.validate(Instant::now())?;
        let mut state = self.state.lock();
        // Approval is single-use; a valid replacement cannot resurrect a session.
        if state.approval.is_some() {
            return Err(DecisionUnavailable::Cancelled);
        }
        state.advance()?;
        state.remaining_budget = approval.budget_microusd;
        state.remaining_attempts = approval.max_attempts;
        state.approval = Some(approval);
        Ok(())
    }

    /// Bind every focus/scene/goal change, even when it returns to an old snapshot.
    pub fn bind(&self, request: &CandidateDecisionRequest) -> Result<(), DecisionUnavailable> {
        let mut state = self.state.lock();
        state.advance()?;
        state.binding = Some(request.binding().clone());
        state.cache = None;
        Ok(())
    }

    /// Policy, account or credential changes require a fresh approved runtime.
    pub fn revoke(&self) {
        let mut state = self.state.lock();
        state.revoked = true;
        state.epoch = state.epoch.saturating_add(1);
        state.credential_value = None;
        state.cache = None;
    }
}

/// Read-only preparation data, never an egress ticket. The advisory port accepts
/// the original request and checks live authority again before sending or returning.
pub struct PreparedCandidateDecision {
    pub text: DecisionText,
    pub epoch: u64,
    pub authority_revision: String,
}

/// Local input preparation with canonical consent/window and credential checks.
/// It owns no HTTP/backend port and reserves no send budget.
pub struct CandidateDecisionPreparer {
    privacy: ExternalOcrPrivacyGuard,
    secrets: Arc<dyn SecretStore>,
    mode: AiAccessMode,
    state: Arc<Mutex<State>>,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl CandidateDecisionPreparer {
    pub fn new(
        privacy: ExternalOcrPrivacyGuard,
        secrets: Arc<dyn SecretStore>,
        mode: AiAccessMode,
        control: &CandidateDecisionControl,
    ) -> Self {
        Self {
            privacy,
            secrets,
            mode,
            state: control.state.clone(),
            now: Arc::new(Instant::now),
        }
    }

    fn local_check(
        &self,
        request: &CandidateDecisionRequest,
        epoch: u64,
        revision: Option<&str>,
    ) -> Result<(), DecisionUnavailable> {
        let now = (self.now)();
        let state = self.state.lock();
        if state.revoked {
            return Err(DecisionUnavailable::Cancelled);
        }
        let approval = state.approval.as_ref().ok_or(DecisionUnavailable::Off)?;
        if self.mode != AiAccessMode::ProviderApiKey {
            return Err(DecisionUnavailable::LocalOnly);
        }
        if state.epoch != epoch || state.binding.as_ref() != Some(request.binding()) {
            return Err(DecisionUnavailable::Stale);
        }
        if now < request.binding().observed_at
            || now >= request.binding().deadline
            || now >= approval.expires_at
        {
            return Err(DecisionUnavailable::Expired);
        }
        drop(state);
        let current = self.privacy.candidate_consent_revision()?;
        if revision.is_some_and(|expected| current != expected) {
            return Err(DecisionUnavailable::ConsentOrPolicyDenied);
        }
        Ok(())
    }

    async fn checkpoint(
        &self,
        request: &CandidateDecisionRequest,
        epoch: u64,
        revision: &str,
    ) -> Result<(), DecisionUnavailable> {
        self.local_check(request, epoch, Some(revision))?;
        let (current, window) = self.privacy.candidate_authority().await?;
        self.local_check(request, epoch, Some(revision))?;
        if current != revision {
            return Err(DecisionUnavailable::ConsentOrPolicyDenied);
        }
        if window != request.binding().window_digest {
            return Err(DecisionUnavailable::Stale);
        }
        Ok(())
    }

    fn sanitize(
        &self,
        request: &CandidateDecisionRequest,
    ) -> Result<DecisionText, DecisionUnavailable> {
        let sanitize = |text: &str| self.privacy.sanitize_candidate_text(text);
        let text = DecisionText {
            goal: sanitize(&request.snapshot().goal),
            candidates: request
                .snapshot()
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
        Ok(text)
    }

    async fn credential(
        &self,
        approval: &CandidateShadowApproval,
    ) -> Result<String, DecisionUnavailable> {
        let (namespace, key) =
            provider_api_key_secret_ref("typesafe", &approval.credential_profile)
                .map_err(|_| DecisionUnavailable::CredentialUnavailable)?;
        let secret = self
            .secrets
            .retrieve(&namespace, key)
            .await
            .map_err(|_| DecisionUnavailable::CredentialUnavailable)?
            .filter(|value| !value.is_empty())
            .ok_or(DecisionUnavailable::CredentialUnavailable)?;
        if secret.len() > 4096 {
            return Err(DecisionUnavailable::CredentialUnavailable);
        }
        let mut state = self.state.lock();
        // An await must not repopulate a secret after another handle revoked it.
        if state.revoked {
            return Err(DecisionUnavailable::Cancelled);
        }
        if state
            .credential_value
            .as_ref()
            .is_some_and(|old| old != &secret)
        {
            state.revoked = true;
            state.credential_value = None;
            state.cache = None;
            return Err(DecisionUnavailable::CredentialUnavailable);
        }
        state.credential_value = Some(secret.clone());
        Ok(secret)
    }

    pub async fn prepare(
        &self,
        request: &CandidateDecisionRequest,
    ) -> Result<PreparedCandidateDecision, DecisionUnavailable> {
        let epoch = self.state.lock().epoch;
        self.local_check(request, epoch, None)?;
        let (revision, window) = self.privacy.candidate_authority().await?;
        if window != request.binding().window_digest {
            return Err(DecisionUnavailable::Stale);
        }
        self.checkpoint(request, epoch, &revision).await?;
        let approval = self
            .state
            .lock()
            .approval
            .clone()
            .ok_or(DecisionUnavailable::Off)?;
        drop(self.credential(&approval).await?);
        self.checkpoint(request, epoch, &revision).await?;
        Ok(PreparedCandidateDecision {
            text: self.sanitize(request)?,
            epoch,
            authority_revision: revision,
        })
    }
}

pub(super) struct GuardedCandidateDecision {
    backend: Arc<dyn CandidateDecisionBackendPort>,
    audit: Arc<dyn CandidateDecisionAuditPort>,
    preparer: CandidateDecisionPreparer,
}

impl GuardedCandidateDecision {
    pub(super) fn new(
        backend: Arc<dyn CandidateDecisionBackendPort>,
        audit: Arc<dyn CandidateDecisionAuditPort>,
        secrets: Arc<dyn SecretStore>,
        privacy: ExternalOcrPrivacyGuard,
        mode: AiAccessMode,
    ) -> (Self, CandidateDecisionControl) {
        let control = CandidateDecisionControl::default();
        let preparer = CandidateDecisionPreparer::new(privacy, secrets, mode, &control);
        (
            Self {
                backend,
                audit,
                preparer,
            },
            control,
        )
    }

    fn cache_key(
        request: &CandidateDecisionRequest,
        epoch: u64,
        revision: &str,
        text: &DecisionText,
        approval: &CandidateShadowApproval,
    ) -> Result<String, DecisionUnavailable> {
        let bytes = serde_json::to_vec(&(
            DECISION_SCHEMA,
            DECISION_RUBRIC,
            "pii-before-wire.v1",
            JEV_ENDPOINT,
            JEV_MODEL,
            &request.binding().digest,
            epoch,
            revision,
            text,
            &approval.principal_namespace,
            &approval.credential_profile,
            &approval.credential_revision,
            &approval.policy_revision,
            &approval.price_reference,
            &approval.approval_reference,
        ))
        .map_err(|_| DecisionUnavailable::InvalidInput)?;
        Ok(digest_bytes(&bytes))
    }

    async fn begin(
        &self,
        context: &Evaluation<'_>,
        decision_id: Uuid,
        stage: DecisionStage,
    ) -> Result<(Uuid, String, u64), DecisionUnavailable> {
        let Evaluation {
            request,
            epoch,
            revision,
            approval,
            ..
        } = *context;
        self.preparer.checkpoint(request, epoch, revision).await?;
        let secret = self.preparer.credential(approval).await?;
        self.preparer.checkpoint(request, epoch, revision).await?;
        let reservation = approval
            .cost(DecisionUsage {
                input_tokens: 65_536,
                output_tokens: 65_536,
            })
            .ok_or(DecisionUnavailable::BudgetExceeded)?;
        {
            let mut state = self.preparer.state.lock();
            let attempts = state
                .remaining_attempts
                .checked_sub(1)
                .ok_or(DecisionUnavailable::BudgetExceeded)?;
            let budget = state
                .remaining_budget
                .checked_sub(reservation)
                .ok_or(DecisionUnavailable::BudgetExceeded)?;
            // Commit both balances only when both checked debits succeed.
            state.remaining_attempts = attempts;
            state.remaining_budget = budget;
        }
        let id = Uuid::new_v4();
        // If cancelled after this durable intent, the attempt remains unresolved;
        // never refund its reservation or claim zero billed cost without evidence.
        self.audit
            .record(DecisionAuditRecord {
                id: Uuid::new_v4(),
                decision_id,
                attempt_id: Some(id),
                phase: DecisionAuditPhase::BeforeSend,
                stage,
                attempt: None,
                context: context.audit.clone(),
                cache_source: None,
            })
            .await
            .map_err(|_| DecisionUnavailable::AuditUnavailable)?;
        self.preparer.checkpoint(request, epoch, revision).await?;
        drop(secret);
        let secret = self.preparer.credential(approval).await?;
        self.preparer.checkpoint(request, epoch, revision).await?;
        Ok((id, secret, reservation))
    }

    async fn finish<T>(
        &self,
        response: Result<BackendResponse<T>, BackendFailure>,
        pending: PendingAttempt,
        context: &Evaluation<'_>,
        result: &mut CandidateDecisionResult,
    ) -> Result<T, DecisionUnavailable> {
        let Evaluation {
            request,
            epoch,
            revision,
            approval,
            ..
        } = *context;
        let (usage, attempted, failure, request_hash) = match &response {
            Ok(r) => (Some(r.usage), true, None, Some(r.request_hash.clone())),
            Err(e) => (e.usage, e.attempted, Some(e.reason), e.request_hash.clone()),
        };
        let cost = usage.and_then(|u| approval.cost(u));
        let receipt = DecisionAttempt {
            request_hash,
            id: pending.id,
            stage: pending.stage,
            attempted,
            elapsed_ms: u64::try_from(
                (self.preparer.now)()
                    .saturating_duration_since(pending.started)
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            usage,
            estimated_cost_microusd: cost,
            failure,
        };
        result.attempts.push(receipt.clone());
        self.audit
            .record(DecisionAuditRecord {
                id: Uuid::new_v4(),
                decision_id: result.decision_id,
                attempt_id: Some(pending.id),
                phase: DecisionAuditPhase::AfterSend,
                stage: pending.stage,
                attempt: Some(receipt),
                context: context.audit.clone(),
                cache_source: None,
            })
            .await
            .map_err(|_| DecisionUnavailable::AuditUnavailable)?;
        self.preparer.checkpoint(request, epoch, revision).await?;
        drop(self.preparer.credential(approval).await?);
        self.preparer.checkpoint(request, epoch, revision).await?;
        let response = response.map_err(|e| e.reason)?;
        if response.observed_model != JEV_MODEL || cost.is_none_or(|c| c > pending.reservation) {
            return Err(DecisionUnavailable::InvalidResponse);
        }
        // A durable but invalid response is not evidence for releasing budget
        // at this approval's model/price. Keep its reservation conservatively.
        if let Some(refund) = cost.and_then(|cost| pending.reservation.checked_sub(cost)) {
            let mut state = self.preparer.state.lock();
            state.remaining_budget = state.remaining_budget.saturating_add(refund);
        }
        result.observed_model = Some(response.observed_model);
        Ok(response.value)
    }

    async fn evaluate(
        &self,
        request: &CandidateDecisionRequest,
        result: &mut CandidateDecisionResult,
    ) -> Result<(), DecisionUnavailable> {
        let prepared = self.preparer.prepare(request).await?;
        let epoch = prepared.epoch;
        let revision = prepared.authority_revision;
        let text = prepared.text;
        self.preparer.checkpoint(request, epoch, &revision).await?;
        let approval = self
            .preparer
            .state
            .lock()
            .approval
            .clone()
            .ok_or(DecisionUnavailable::Off)?;
        result.sanitized_state_hash = Some(digest_bytes(
            &serde_json::to_vec(&text).map_err(|_| DecisionUnavailable::InvalidInput)?,
        ));
        let cache_key = Self::cache_key(request, epoch, &revision, &text, &approval)?;
        let audit = DecisionAuditContext {
            namespace_hash: cache_key.clone(),
            provider: result.provider,
            requested_model: JEV_MODEL,
            schema_revision: DECISION_SCHEMA,
            rubric_revision: DECISION_RUBRIC,
        };
        let context = Evaluation {
            request,
            epoch,
            revision: &revision,
            approval: &approval,
            audit: &audit,
        };
        let cached = self
            .preparer
            .state
            .lock()
            .cache
            .as_ref()
            .filter(|(key, _)| key == &cache_key)
            .map(|(_, r)| r.clone());
        if let Some(mut cached) = cached {
            drop(self.preparer.credential(&approval).await?);
            self.preparer.checkpoint(request, epoch, &revision).await?;
            self.audit
                .record(DecisionAuditRecord {
                    id: Uuid::new_v4(),
                    decision_id: result.decision_id,
                    attempt_id: None,
                    phase: DecisionAuditPhase::CacheHit,
                    stage: DecisionStage::Choice,
                    attempt: None,
                    context: audit.clone(),
                    cache_source: Some(cached.decision_id),
                })
                .await
                .map_err(|_| DecisionUnavailable::AuditUnavailable)?;
            self.preparer.checkpoint(request, epoch, &revision).await?;
            drop(self.preparer.credential(&approval).await?);
            self.preparer.checkpoint(request, epoch, &revision).await?;
            cached.cache_source = Some(cached.decision_id);
            cached.decision_id = result.decision_id;
            cached.attempts.clear();
            *result = cached;
            return Ok(());
        }
        let (id, secret, reservation) = self
            .begin(&context, result.decision_id, DecisionStage::Choice)
            .await?;
        let pending = PendingAttempt {
            id,
            reservation,
            stage: DecisionStage::Choice,
            started: (self.preparer.now)(),
        };
        let response = self.backend.choose(&text, &secret).await;
        drop(secret);
        let choice = self.finish(response, pending, &context, result).await?;
        let selected = choice.selected.clone();
        let selected_probability = *choice
            .probabilities
            .get(&selected)
            .ok_or(DecisionUnavailable::InvalidResponse)?;
        result.choice = Some(choice);
        result.decision = match selected.as_str() {
            NONE_OPTION => CandidateDecision::None,
            DELEGATE_OPTION => CandidateDecision::Delegate,
            _ => {
                let index = text
                    .candidates
                    .iter()
                    .position(|c| c.id == selected)
                    .ok_or(DecisionUnavailable::InvalidResponse)?;
                let (id, secret, reservation) = self
                    .begin(&context, result.decision_id, DecisionStage::Suitability)
                    .await?;
                let pending = PendingAttempt {
                    id,
                    reservation,
                    stage: DecisionStage::Suitability,
                    started: (self.preparer.now)(),
                };
                let response = self
                    .backend
                    .assess_selected(&text, &selected, &secret)
                    .await;
                drop(secret);
                let assessment = self.finish(response, pending, &context, result).await?;
                if assessment.selected != selected
                    || !assessment.score.is_finite()
                    || !(0.0..=1.0).contains(&assessment.score)
                {
                    return Err(DecisionUnavailable::InvalidResponse);
                }
                CandidateDecision::Selected {
                    candidate_id: request.snapshot().candidates[index]
                        .element
                        .element_id
                        .clone(),
                    selected_probability,
                    suitability: assessment.score,
                }
            }
        };
        self.preparer.local_check(request, epoch, Some(&revision))?;
        self.preparer
            .state
            .lock()
            .cache_completed(epoch, cache_key, result)
    }
}

#[derive(Clone, Copy)]
struct Evaluation<'a> {
    request: &'a CandidateDecisionRequest,
    epoch: u64,
    revision: &'a str,
    approval: &'a CandidateShadowApproval,
    audit: &'a DecisionAuditContext,
}

struct PendingAttempt {
    id: Uuid,
    stage: DecisionStage,
    started: Instant,
    reservation: u64,
}

#[async_trait]
impl CandidateDecisionPort for GuardedCandidateDecision {
    async fn decide(&self, request: &CandidateDecisionRequest) -> CandidateDecisionResult {
        let mut result =
            CandidateDecisionResult::unavailable(request.binding(), DecisionUnavailable::Off);
        // Bound secret/window/audit awaits too. Cancelling the future does not
        // prove that an already submitted request was unbilled; its durable
        // intent and reserved budget remain pending.
        let outcome = tokio::time::timeout_at(
            tokio::time::Instant::from_std(request.binding().deadline),
            self.evaluate(request, &mut result),
        )
        .await
        .unwrap_or(Err(DecisionUnavailable::Expired));
        if let Err(reason) = outcome {
            result.decision = CandidateDecision::Unavailable(reason);
        }
        result
    }
}

#[cfg(test)]
#[path = "guarded_candidate_decision_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "guarded_candidate_runtime_tests.rs"]
mod runtime_tests;
