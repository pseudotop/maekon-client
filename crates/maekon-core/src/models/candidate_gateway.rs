//! Gateway-only evidence. Public prices and login state cannot issue an approval.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use uuid::Uuid;

use super::candidate_decision::{DecisionStage, DecisionUnavailable, DecisionUsage};
use super::candidate_decision_policy::CandidateBilling;

pub const GATEWAY_ENDPOINT: &str = "https://ai-gateway.vercel.sh/typesafe/v1/systemone";
pub const GATEWAY_MODEL: &str = "typesafe-ai/jev";
pub const GATEWAY_PROVIDER: &str = "vercel-ai-gateway";
pub const GATEWAY_RUBRIC: &str = "candidate-gateway.choice-selected-noul.v1";
pub const GATEWAY_SCHEMA: &str = "candidate-gateway.audit.v1";
pub const GATEWAY_MAX_TOKENS: u64 = 65_536;

/// Exact decimal USD, bounded to twelve fractional digits. Never use binary
/// floating point, exponent notation, a negative value or a rounded zero.
pub fn usd_picos(value: &str) -> Option<u64> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if (whole.len() > 1 && whole.starts_with('0'))
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 12
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let whole = whole.parse::<u64>().ok()?.checked_mul(1_000_000_000_000)?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u64>()
            .ok()?
            .checked_mul(10_u64.pow(12 - fraction.len() as u32))?
    };
    whole.checked_add(fraction)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayPrice {
    pub input_picos_per_token: u64,
    pub output_picos_per_token: u64,
    /// Includes effective account/provider allowlist fees, even for a promotion.
    pub request_fee_picos: u64,
}

impl GatewayPrice {
    pub fn estimate(&self, usage: DecisionUsage) -> Option<u64> {
        if usage.input_tokens > GATEWAY_MAX_TOKENS || usage.output_tokens > GATEWAY_MAX_TOKENS {
            return None;
        }
        let cost = u128::from(usage.input_tokens)
            .checked_mul(u128::from(self.input_picos_per_token))?
            .checked_add(
                u128::from(usage.output_tokens)
                    .checked_mul(u128::from(self.output_picos_per_token))?,
            )?
            .checked_add(u128::from(self.request_fee_picos))?;
        u64::try_from(cost).ok()
    }

    pub fn reservation(&self) -> Option<u64> {
        self.estimate(DecisionUsage {
            input_tokens: GATEWAY_MAX_TOKENS,
            output_tokens: GATEWAY_MAX_TOKENS,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayFunding {
    Unknown,
    ZeroPricePromotion,
    /// An account-scoped reserved allocation, not a public free-credit offer or
    /// a transient account balance shared with unrelated consumers.
    FreeCredits {
        allocation_reference: String,
        limit_picos: u64,
        cash_fallback_disabled: bool,
    },
    PaidApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayFundingKind {
    Promotion,
    FreeCredits,
    PaidApi,
}

/// No regional guarantee is inferred from a catalog without regional evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayRegion {
    ApprovedGlobal,
    RequiredRegionUnsupported,
}

/// Trusted, fresh account observation. Deliberately not deserializable from
/// config/IPC, nor constructible from the public model catalog alone.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayAuthority {
    pub account_reference: String,
    pub credential_profile: String,
    pub credential_revision: String,
    pub credential_sha256: String,
    pub price_reference: String,
    pub price: GatewayPrice,
    pub funding: GatewayFunding,
    pub routing: GatewayRouting,
    pub effective_route_reference: String,
    pub retention_reference: String,
    pub region: GatewayRegion,
    /// Must refer to effective restrictions on the TypeSafe-compatible endpoint;
    /// restrictions documented only for /v1/evaluate are insufficient.
    pub typesafe_route_verified: bool,
    pub gateway_key_only: bool,
    pub fallback_disabled: bool,
    pub valid_until: Instant,
    pub wall_valid_until: SystemTime,
}

/// References are opaque evidence, not paths or account authorization. Keep the
/// original value; reject blank, control-bearing and oversized references.
pub fn gateway_reference_valid(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

impl GatewayAuthority {
    /// Check observed evidence without issuing permission to send. Runtime must
    /// still bind approval, reobserve changes and reserve attempts and budget.
    pub fn validate(&self, now: Instant, wall: SystemTime) -> Result<(), DecisionUnavailable> {
        let references = [
            &self.account_reference,
            &self.credential_profile,
            &self.credential_revision,
            &self.price_reference,
            &self.effective_route_reference,
            &self.retention_reference,
        ];
        let remaining = [
            self.valid_until.checked_duration_since(now),
            self.wall_valid_until.duration_since(wall).ok(),
        ];
        if references
            .into_iter()
            .any(|reference| !gateway_reference_valid(reference))
            || !gateway_digest_valid(&self.credential_sha256)
            || ![
                self.typesafe_route_verified,
                self.gateway_key_only,
                self.fallback_disabled,
            ]
            .into_iter()
            .all(|verified| verified)
            || self.region != GatewayRegion::ApprovedGlobal
            || remaining.into_iter().any(|remaining| {
                remaining.is_none_or(|duration| {
                    duration.is_zero() || duration > Duration::from_secs(3600)
                })
            })
        {
            return Err(DecisionUnavailable::ApprovalMissing);
        }
        self.routing
            .validate()
            .map_err(|_| DecisionUnavailable::ApprovalMissing)?;
        let reservation = self
            .price
            .reservation()
            .ok_or(DecisionUnavailable::ApprovalMissing)?;
        match &self.funding {
            GatewayFunding::Unknown => Err(DecisionUnavailable::ApprovalMissing),
            GatewayFunding::ZeroPricePromotion if reservation != 0 => {
                Err(DecisionUnavailable::Rejected)
            }
            GatewayFunding::FreeCredits {
                allocation_reference,
                limit_picos,
                cash_fallback_disabled,
            } if !gateway_reference_valid(allocation_reference)
                || *limit_picos == 0
                || *limit_picos < reservation
                || !cash_fallback_disabled =>
            {
                Err(DecisionUnavailable::Rejected)
            }
            _ => Ok(()),
        }
    }

    /// A credit allocation is metered and must retain the API budget gate.
    pub fn billing(&self) -> CandidateBilling {
        match self.funding {
            GatewayFunding::Unknown => CandidateBilling::Unknown,
            GatewayFunding::ZeroPricePromotion => CandidateBilling::Promotion {
                valid_until: self.valid_until,
            },
            GatewayFunding::FreeCredits { .. } | GatewayFunding::PaidApi => {
                CandidateBilling::MeteredApi
            }
        }
    }

    pub fn funding_kind(&self) -> Option<GatewayFundingKind> {
        match self.funding {
            GatewayFunding::Unknown => None,
            GatewayFunding::ZeroPricePromotion => Some(GatewayFundingKind::Promotion),
            GatewayFunding::FreeCredits { .. } => Some(GatewayFundingKind::FreeCredits),
            GatewayFunding::PaidApi => Some(GatewayFundingKind::PaidApi),
        }
    }
}

/// Preserve each observed identity; none is an attestation of a weights revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GatewayRouting {
    pub response_model: String,
    pub original_model_id: String,
    pub canonical_slug: String,
    pub resolved_provider: String,
    pub final_provider: String,
}

impl GatewayRouting {
    pub fn validate(&self) -> Result<(), DecisionUnavailable> {
        let identities = [
            (self.response_model.as_str(), GATEWAY_MODEL),
            (self.original_model_id.as_str(), GATEWAY_MODEL),
            (self.canonical_slug.as_str(), GATEWAY_MODEL),
            (self.resolved_provider.as_str(), "typesafe-ai"),
            (self.final_provider.as_str(), "typesafe-ai"),
        ];
        if identities
            .into_iter()
            .all(|(actual, expected)| actual == expected)
        {
            Ok(())
        } else {
            Err(DecisionUnavailable::InvalidResponse)
        }
    }
}

/// Exact provider-reported USD strings. Missing fields remain unknown; credits
/// consumed and a reported cost are not evidence of a cash invoice or zero cost.
#[derive(Debug, Clone, Default, Serialize)]
pub struct GatewayReportedCosts {
    pub cost: Option<String>,
    pub market_cost: Option<String>,
    pub surcharge_cost: Option<String>,
    pub gateway_cost: Option<String>,
}

impl GatewayReportedCosts {
    pub fn validate(&self) -> Result<(), DecisionUnavailable> {
        if [
            &self.cost,
            &self.market_cost,
            &self.surcharge_cost,
            &self.gateway_cost,
        ]
        .into_iter()
        .flatten()
        .any(|value| usd_picos(value).is_none())
        {
            Err(DecisionUnavailable::InvalidResponse)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayObservation {
    pub routing: GatewayRouting,
    pub usage: Option<DecisionUsage>,
    pub costs: GatewayReportedCosts,
    pub generation_hash: Option<String>,
}

pub fn gateway_digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

impl GatewayObservation {
    /// Missing usage, cost and generation identity remain unknown.
    pub fn validate(&self) -> Result<(), DecisionUnavailable> {
        self.routing.validate()?;
        self.costs.validate()?;
        if self.usage.is_some_and(|usage| {
            [usage.input_tokens, usage.output_tokens]
                .into_iter()
                .any(|tokens| tokens > GATEWAY_MAX_TOKENS)
        }) || self
            .generation_hash
            .as_deref()
            .is_some_and(|hash| !gateway_digest_valid(hash))
        {
            Err(DecisionUnavailable::InvalidResponse)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub struct GatewayChoice {
    pub selected: String,
    pub probabilities: BTreeMap<String, f64>,
    /// Absent confidence is not replaced with the maximum option probability.
    pub confidence: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct GatewayResponse<T> {
    pub value: T,
    pub request_hash: String,
    pub observation: GatewayObservation,
}

#[derive(Debug, Clone)]
pub struct GatewayFailure {
    pub reason: DecisionUnavailable,
    pub attempted: bool,
    pub request_hash: Option<String>,
    pub observation: Option<Box<GatewayObservation>>,
}

impl GatewayFailure {
    pub fn new(reason: DecisionUnavailable, attempted: bool) -> Self {
        Self {
            reason,
            attempted,
            request_hash: None,
            observation: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewaySendState {
    Pending,
    NotSent,
    Attempted,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayAttempt {
    pub id: Uuid,
    pub stage: DecisionStage,
    pub request_hash: Option<String>,
    pub send_state: GatewaySendState,
    pub elapsed_ms: u64,
    /// Retained on cancellation, unknown usage and failure; never a cost claim.
    pub reserved_picos: u64,
    pub estimated_cost_picos: Option<u64>,
    pub funding: GatewayFundingKind,
    pub observation: Option<Box<GatewayObservation>>,
    pub failure: Option<DecisionUnavailable>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayAuditPhase {
    BeforeAttempt,
    AfterAttempt,
}

#[derive(Debug, Clone, Serialize)]
pub struct GatewayAuditRecord {
    pub id: Uuid,
    pub decision_id: Uuid,
    pub namespace_hash: String,
    pub schema: &'static str,
    pub phase: GatewayAuditPhase,
    pub attempt: GatewayAttempt,
}

#[cfg(test)]
#[path = "candidate_gateway_tests.rs"]
mod tests;
