//! Pinned Gateway HTTP transport. Construction and request encoding grant no
//! authority; the trusted caller must provide consent, audit and budget guards.

use std::time::Duration;

use async_trait::async_trait;
use maekon_core::models::candidate_decision::{
    DecisionText, DecisionUnavailable, SuitabilityEvidence,
};
use maekon_core::models::candidate_gateway::{
    GatewayChoice, GatewayFailure, GatewayObservation, GatewayResponse, GATEWAY_ENDPOINT,
};
use maekon_core::ports::candidate_assessment::CandidateAttemptGuard;
use maekon_core::ports::candidate_gateway::GatewayCandidateBackendPort;
use maekon_http_core::outbound::{hardened_client_builder, TransportPolicy};
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE};

use crate::gateway_candidate_request::{
    encode_choice, encode_suitability, EncodedGatewayCandidateRequest,
};
use crate::gateway_candidate_response::{
    decode_choice, decode_suitability, MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);

fn build_client(policy: TransportPolicy) -> Result<reqwest::Client, GatewayFailure> {
    hardened_client_builder(policy)
        .no_proxy()
        .retry(reqwest::retry::never())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(3))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| GatewayFailure::new(DecisionUnavailable::Transport, false))
}

/// No production endpoint, retry, credential or timeout override.
pub struct GatewayCandidateDecisionClient {
    client: reqwest::Client,
    endpoint: String,
    deadline: Duration,
}

impl GatewayCandidateDecisionClient {
    pub fn new() -> Result<Self, GatewayFailure> {
        Ok(Self {
            client: build_client(TransportPolicy::HttpsOnly)?,
            endpoint: GATEWAY_ENDPOINT.into(),
            deadline: REQUEST_TIMEOUT,
        })
    }

    async fn request(
        &self,
        prepared: EncodedGatewayCandidateRequest,
        key: &str,
        guard: &dyn CandidateAttemptGuard,
    ) -> Result<(Vec<u8>, String), GatewayFailure> {
        if key.is_empty() || key.len() > 4096 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(GatewayFailure::new(
                DecisionUnavailable::CredentialUnavailable,
                false,
            ));
        }
        let mut auth = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| GatewayFailure::new(DecisionUnavailable::CredentialUnavailable, false))?;
        auth.set_sensitive(true);
        let (body, hash) = prepared.into_parts();
        let request = self
            .client
            .post(&self.endpoint)
            .header(AUTHORIZATION, auth)
            .header(CONTENT_TYPE, "application/json")
            .body(body)
            .build()
            .map_err(|_| GatewayFailure::new(DecisionUnavailable::Transport, false))?;
        let mut attempted = false;
        let operation = async {
            guard
                .checkpoint()
                .await
                .map_err(|reason| GatewayFailure::new(reason, false))?;
            // Attempted means handed to the HTTP client, not proof that the
            // provider received bytes. Reservations are never refunded here.
            attempted = true;
            let response = self.client.execute(request).await;
            guard
                .checkpoint()
                .await
                .map_err(|reason| GatewayFailure::new(reason, true))?;
            let mut response =
                response.map_err(|error| GatewayFailure::new(transport_reason(error), true))?;
            if response.status().as_u16() != 200 {
                // Never expose remote error text, request bodies or credentials.
                return Err(GatewayFailure::new(
                    match response.status().as_u16() {
                        401 | 403 => DecisionUnavailable::Unauthorized,
                        402 => DecisionUnavailable::BudgetExceeded,
                        429 => DecisionUnavailable::RateLimited,
                        529 => DecisionUnavailable::Overloaded,
                        _ => DecisionUnavailable::Rejected,
                    },
                    true,
                ));
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES as u64)
            {
                return Err(GatewayFailure::new(
                    DecisionUnavailable::ResponseTooLarge,
                    true,
                ));
            }
            let mut bytes = Vec::new();
            loop {
                guard
                    .checkpoint()
                    .await
                    .map_err(|reason| GatewayFailure::new(reason, true))?;
                let chunk = response.chunk().await;
                guard
                    .checkpoint()
                    .await
                    .map_err(|reason| GatewayFailure::new(reason, true))?;
                let Some(chunk) =
                    chunk.map_err(|error| GatewayFailure::new(transport_reason(error), true))?
                else {
                    break;
                };
                if chunk.len() > MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES.saturating_sub(bytes.len()) {
                    return Err(GatewayFailure::new(
                        DecisionUnavailable::ResponseTooLarge,
                        true,
                    ));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        };
        // Includes guard awaits: reqwest's timeout alone does not bound them.
        let result = tokio::time::timeout(self.deadline, operation)
            .await
            .unwrap_or_else(|_| Err(GatewayFailure::new(DecisionUnavailable::Timeout, attempted)));
        result
            .map(|bytes| (bytes, hash.clone()))
            .map_err(|mut failure| {
                failure.request_hash = Some(hash);
                failure
            })
    }
}

fn transport_reason(error: reqwest::Error) -> DecisionUnavailable {
    if error.is_timeout() {
        DecisionUnavailable::Timeout
    } else {
        DecisionUnavailable::Transport
    }
}

fn bind<T>(
    decoded: Result<(T, GatewayObservation), GatewayFailure>,
    hash: String,
) -> Result<GatewayResponse<T>, GatewayFailure> {
    decoded
        .map(|(value, observation)| GatewayResponse {
            value,
            observation,
            request_hash: hash.clone(),
        })
        .map_err(|mut failure| {
            failure.request_hash = Some(hash);
            failure
        })
}

#[async_trait]
impl GatewayCandidateBackendPort for GatewayCandidateDecisionClient {
    async fn choose(
        &self,
        text: &DecisionText,
        key: &str,
        guard: &dyn CandidateAttemptGuard,
    ) -> Result<GatewayResponse<GatewayChoice>, GatewayFailure> {
        let (bytes, hash) = self.request(encode_choice(text)?, key, guard).await?;
        bind(decode_choice(&bytes, text), hash)
    }

    async fn assess_selected(
        &self,
        text: &DecisionText,
        selected: &str,
        key: &str,
        guard: &dyn CandidateAttemptGuard,
    ) -> Result<GatewayResponse<SuitabilityEvidence>, GatewayFailure> {
        let (bytes, hash) = self
            .request(encode_suitability(text, selected)?, key, guard)
            .await?;
        bind(decode_suitability(&bytes, text, selected), hash)
    }
}

#[cfg(test)]
#[path = "gateway_candidate_decision_tests.rs"]
mod tests;
