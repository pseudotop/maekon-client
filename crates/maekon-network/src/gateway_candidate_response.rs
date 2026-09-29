//! Bounded decoding for the pinned TypeSafe-compatible Gateway endpoint.
//! No transport, account authorization, retries or executable actions.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::marker::PhantomData;

use maekon_core::models::candidate_decision::{
    digest_bytes, DecisionText, DecisionUnavailable, DecisionUsage, SuitabilityEvidence,
    DELEGATE_OPTION, NONE_OPTION,
};
use maekon_core::models::candidate_gateway::{
    GatewayChoice, GatewayFailure, GatewayObservation, GatewayReportedCosts, GatewayRouting,
};
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

/// Transport and decoding must enforce the same byte limit.
pub const MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES: usize = 32_768;
pub(crate) const GATEWAY_CANDIDATE_QUESTION: &str = "decision";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response<A> {
    model: String,
    answers: A,
    usage: Option<Usage>,
    provider_metadata: ProviderMetadata,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderMetadata {
    gateway: GatewayMetadata,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GatewayMetadata {
    routing: Routing,
    cost: Option<String>,
    market_cost: Option<String>,
    surcharge_cost: Option<String>,
    gateway_cost: Option<String>,
    generation_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Routing {
    original_model_id: String,
    canonical_slug: String,
    resolved_provider: String,
    final_provider: String,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Answer {
    Choice {
        choice: String,
        probabilities: UniqueMap<f64>,
        confidence: Option<f64>,
    },
    Noul {
        noul: f64,
    },
}

// Map keys are decoded before comparison, so escaped duplicate keys are also
// rejected. A regular BTreeMap deserializer would silently keep the last value.
struct UniqueMap<V>(BTreeMap<String, V>);
impl<'de, V: Deserialize<'de>> Deserialize<'de> for UniqueMap<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor<V>(PhantomData<V>);
        impl<'de, V: Deserialize<'de>> Visitor<'de> for UniqueVisitor<V> {
            type Value = UniqueMap<V>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Self::Value, M::Error> {
                let mut map = BTreeMap::new();
                while let Some((key, value)) = access.next_entry::<String, V>()? {
                    if map.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                }
                Ok(UniqueMap(map))
            }
        }
        deserializer.deserialize_map(UniqueVisitor(PhantomData))
    }
}

fn invalid(observation: Option<GatewayObservation>) -> GatewayFailure {
    GatewayFailure {
        reason: DecisionUnavailable::InvalidResponse,
        attempted: true,
        request_hash: None,
        observation: observation.map(Box::new),
    }
}

fn decode(bytes: &[u8]) -> Result<(Answer, GatewayObservation), GatewayFailure> {
    if bytes.len() > MAX_GATEWAY_CANDIDATE_RESPONSE_BYTES {
        return Err(GatewayFailure::new(
            DecisionUnavailable::ResponseTooLarge,
            true,
        ));
    }
    // Validate metadata independently so an invalid answer does not erase a
    // valid usage/cost observation. Both passes consume the same bounded bytes.
    let envelope: Response<Option<IgnoredAny>> =
        serde_json::from_slice(bytes).map_err(|_| invalid(None))?;
    let metadata = envelope.provider_metadata.gateway;
    if metadata.generation_id.as_ref().is_some_and(|id| {
        id.is_empty()
            || id.len() > 256
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-:.".contains(&byte))
    }) {
        return Err(invalid(None));
    }
    let observation = GatewayObservation {
        routing: GatewayRouting {
            response_model: envelope.model,
            original_model_id: metadata.routing.original_model_id,
            canonical_slug: metadata.routing.canonical_slug,
            resolved_provider: metadata.routing.resolved_provider,
            final_provider: metadata.routing.final_provider,
        },
        usage: envelope.usage.map(|usage| DecisionUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        }),
        costs: GatewayReportedCosts {
            cost: metadata.cost,
            market_cost: metadata.market_cost,
            surcharge_cost: metadata.surcharge_cost,
            gateway_cost: metadata.gateway_cost,
        },
        generation_hash: metadata.generation_id.map(|id| digest_bytes(id.as_bytes())),
    };
    observation.validate().map_err(|_| invalid(None))?;
    let response: Response<UniqueMap<Answer>> =
        serde_json::from_slice(bytes).map_err(|_| invalid(Some(observation.clone())))?;
    if response
        .answers
        .0
        .keys()
        .map(String::as_str)
        .ne([GATEWAY_CANDIDATE_QUESTION])
    {
        return Err(invalid(Some(observation)));
    }
    let answer = response
        .answers
        .0
        .into_values()
        .next()
        .ok_or_else(|| invalid(Some(observation.clone())))?;
    Ok((answer, observation))
}

/// Decode an already-attempted choice response. Missing confidence remains
/// unknown; the runtime must enforce its confidence, privacy and budget gates.
pub fn decode_choice(
    bytes: &[u8],
    text: &DecisionText,
) -> Result<(GatewayChoice, GatewayObservation), GatewayFailure> {
    text.validate()
        .map_err(|reason| GatewayFailure::new(reason, true))?;
    let (answer, observation) = decode(bytes)?;
    let Answer::Choice {
        choice,
        probabilities,
        confidence,
    } = answer
    else {
        return Err(invalid(Some(observation)));
    };
    let expected: BTreeSet<_> = text
        .candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .chain([NONE_OPTION, DELEGATE_OPTION])
        .collect();
    if confidence.is_some_and(|value| !(0.0..=1.0).contains(&value))
        || probabilities.0.keys().map(String::as_str).ne(expected)
        || probabilities
            .0
            .values()
            .any(|value| !(0.0..=1.0).contains(value))
        || !(1.0 - 1e-6..=1.0 + 1e-6).contains(&probabilities.0.values().sum::<f64>())
        || probabilities
            .0
            .get(&choice)
            .is_none_or(|chosen| probabilities.0.values().any(|value| value > chosen))
    {
        return Err(invalid(Some(observation)));
    }
    Ok((
        GatewayChoice {
            selected: choice,
            probabilities: probabilities.0,
            confidence,
        },
        observation,
    ))
}

/// Decode only the supplied candidate's suitability, never a general assertion
/// that some candidate could work. Parsing cannot authorize or undo a send.
pub fn decode_suitability(
    bytes: &[u8],
    text: &DecisionText,
    selected: &str,
) -> Result<(SuitabilityEvidence, GatewayObservation), GatewayFailure> {
    text.validate()
        .map_err(|reason| GatewayFailure::new(reason, true))?;
    if !text
        .candidates
        .iter()
        .any(|candidate| candidate.id == selected)
    {
        return Err(GatewayFailure::new(DecisionUnavailable::InvalidInput, true));
    }
    let (answer, observation) = decode(bytes)?;
    let Answer::Noul { noul } = answer else {
        return Err(invalid(Some(observation)));
    };
    if !(0.0..=1.0).contains(&noul) {
        return Err(invalid(Some(observation)));
    }
    Ok((
        SuitabilityEvidence {
            selected: selected.into(),
            score: noul,
        },
        observation,
    ))
}

#[cfg(test)]
#[path = "gateway_candidate_response_tests.rs"]
mod tests;
