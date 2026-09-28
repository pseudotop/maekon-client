//! One approval per Gateway runtime, with shared, nonrefundable reservations.

use super::{LocalAssessmentApproval, LocalAssessmentControl, LocalAssessmentLease};
use maekon_core::models::candidate_decision::{
    digest_bytes, CandidateDecisionRequest, DecisionUnavailable,
};
use maekon_core::models::candidate_decision_policy::CandidateTrigger;
use maekon_core::models::candidate_gateway::{
    gateway_reference_valid, GatewayAuthority, GatewayFunding, GATEWAY_PROVIDER,
};
use maekon_core::ports::secret_store::provider_api_key_secret_ref;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

/// Trusted composition evidence, never IPC/config or automatic restart permission.
#[derive(Clone)]
pub struct GatewayCandidateApproval {
    pub approval_reference: String,
    pub principal_namespace: String,
    pub policy_revision: String,
    pub max_attempts: u32,
    pub budget_picos: u64,
    pub expires_at: Instant,
    pub authority: GatewayAuthority,
}

#[derive(Default)]
struct State {
    approval: Option<GatewayCandidateApproval>,
    remaining_picos: u64,
    consent_revision: Option<String>,
}

/// All clones share revocation and budget. Bind every scene change, including
/// A -> B -> A. Changed account, key, policy or price requires fresh approval.
#[derive(Clone, Default)]
pub struct GatewayCandidateControl {
    state: Arc<Mutex<State>>,
    lifecycle: LocalAssessmentControl,
}

/// An opaque observation of one binding epoch; not permission to send data.
/// Privacy, account observation and durable audit gates remain separate.
pub struct GatewayCandidateLease {
    approval: GatewayCandidateApproval,
    lifecycle: LocalAssessmentLease,
}

impl GatewayCandidateLease {
    pub fn approval(&self) -> &GatewayCandidateApproval {
        &self.approval
    }
}

impl GatewayCandidateControl {
    pub fn approve(&self, approval: GatewayCandidateApproval) -> Result<(), DecisionUnavailable> {
        approval
            .authority
            .validate(Instant::now(), SystemTime::now())?;
        let profile = &approval.authority.credential_profile;
        if profile != profile.trim()
            || provider_api_key_secret_ref(GATEWAY_PROVIDER, profile).is_err()
            || [
                &approval.approval_reference,
                &approval.principal_namespace,
                &approval.policy_revision,
            ]
            .iter()
            .any(|value| !gateway_reference_valid(value))
            || approval.expires_at > approval.authority.valid_until
            || approval
                .authority
                .price
                .reservation()
                .is_none_or(|amount| amount > approval.budget_picos)
            || matches!(&approval.authority.funding, GatewayFunding::FreeCredits { limit_picos, .. } if approval.budget_picos > *limit_picos)
        {
            return Err(DecisionUnavailable::ApprovalMissing);
        }
        let mut state = self.state.lock();
        self.lifecycle.approve(LocalAssessmentApproval {
            approval_reference: approval.approval_reference.clone(),
            principal_namespace: approval.principal_namespace.clone(),
            policy_revision: approval.policy_revision.clone(),
            configuration_revision: approval.authority.effective_route_reference.clone(),
            trigger: CandidateTrigger::UserRequest,
            max_attempts: approval.max_attempts,
            expires_at: approval.expires_at,
            model: None,
        })?;
        state.remaining_picos = approval.budget_picos;
        state.approval = Some(approval);
        Ok(())
    }

    pub fn bind(&self, request: &CandidateDecisionRequest) -> Result<(), DecisionUnavailable> {
        let _state = self.state.lock();
        self.lifecycle.bind(request)
    }

    pub fn revoke(&self) {
        let _state = self.state.lock();
        self.lifecycle.revoke();
    }

    pub fn lease(
        &self,
        request: &CandidateDecisionRequest,
    ) -> Result<GatewayCandidateLease, DecisionUnavailable> {
        let state = self.state.lock();
        let lifecycle = self.lifecycle.lease(request)?;
        let lease = GatewayCandidateLease {
            approval: state.approval.clone().ok_or(DecisionUnavailable::Off)?,
            lifecycle,
        };
        self.checkpoint_at(request, &lease, SystemTime::now())?;
        Ok(lease)
    }

    pub fn checkpoint(
        &self,
        request: &CandidateDecisionRequest,
        lease: &GatewayCandidateLease,
    ) -> Result<(), DecisionUnavailable> {
        let _state = self.state.lock();
        self.checkpoint_at(request, lease, SystemTime::now())
    }

    // Callers hold the Gateway state lock before acquiring the lifecycle lock.
    fn checkpoint_at(
        &self,
        request: &CandidateDecisionRequest,
        lease: &GatewayCandidateLease,
        wall: SystemTime,
    ) -> Result<(), DecisionUnavailable> {
        self.lifecycle.checkpoint(request, &lease.lifecycle)?;
        if wall >= lease.approval.authority.wall_valid_until {
            self.lifecycle.revoke();
            return Err(DecisionUnavailable::Expired);
        }
        Ok(())
    }

    pub fn pin_privacy(
        &self,
        request: &CandidateDecisionRequest,
        lease: &GatewayCandidateLease,
        revision: &str,
    ) -> Result<(), DecisionUnavailable> {
        let mut state = self.state.lock();
        self.checkpoint_at(request, lease, SystemTime::now())?;
        if !gateway_reference_valid(revision)
            || state
                .consent_revision
                .as_ref()
                .is_some_and(|old| old != revision)
        {
            self.lifecycle.revoke();
            return Err(DecisionUnavailable::ConsentOrPolicyDenied);
        }
        state.consent_revision = Some(revision.into());
        Ok(())
    }

    pub fn observe(
        &self,
        request: &CandidateDecisionRequest,
        lease: &GatewayCandidateLease,
        current: &GatewayAuthority,
    ) -> Result<(), DecisionUnavailable> {
        let _state = self.state.lock();
        self.checkpoint_at(request, lease, SystemTime::now())?;
        if current != &lease.approval.authority
            || current.validate(Instant::now(), SystemTime::now()).is_err()
        {
            self.lifecycle.revoke();
            return Err(DecisionUnavailable::Rejected);
        }
        Ok(())
    }

    pub fn check_secret(
        &self,
        request: &CandidateDecisionRequest,
        lease: &GatewayCandidateLease,
        secret: &str,
    ) -> Result<(), DecisionUnavailable> {
        let _state = self.state.lock();
        self.checkpoint_at(request, lease, SystemTime::now())?;
        if secret.trim().is_empty()
            || secret.len() > 4096
            || secret.chars().any(char::is_control)
            || digest_bytes(secret.as_bytes()) != lease.approval.authority.credential_sha256
        {
            self.lifecycle.revoke();
            return Err(DecisionUnavailable::CredentialUnavailable);
        }
        Ok(())
    }

    /// Reserve the attempt and worst-case amount under one lock. Once reserved,
    /// neither cancellation nor missing usage refunds either limit.
    pub fn reserve(
        &self,
        request: &CandidateDecisionRequest,
        lease: &GatewayCandidateLease,
    ) -> Result<u64, DecisionUnavailable> {
        let mut state = self.state.lock();
        self.checkpoint_at(request, lease, SystemTime::now())?;
        let amount = lease
            .approval
            .authority
            .price
            .reservation()
            .ok_or(DecisionUnavailable::BudgetExceeded)?;
        let remaining = state
            .remaining_picos
            .checked_sub(amount)
            .ok_or(DecisionUnavailable::BudgetExceeded)?;
        self.lifecycle.reserve_attempt(request, &lease.lifecycle)?;
        state.remaining_picos = remaining;
        Ok(amount)
    }
}

#[cfg(test)]
#[path = "gateway_candidate_control_tests.rs"]
mod tests;
