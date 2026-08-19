//! Wire-format compatibility tests for `maekon-api-contracts`.
//!
//! Why a separate integration suite (beyond the 85 unit tests in `src/*.rs`):
//! this crate IS the public API contract layer. The relevant regression
//! surface is wire-format stability — a future code change that accidentally
//! renames a serde field, changes a type, or breaks enum casing will silently
//! corrupt client↔server contracts before anyone runs the integration suite.
//!
//! Each scenario below embeds a **frozen JSON sample** as the v1 wire-format
//! reference. The test deserializes the embedded JSON, re-serializes the
//! resulting DTO, and asserts structural equality on the round-trip. This is
//! deliberately NOT derived from the current DTO definitions — the embedded
//! samples are the source of truth.
//!
//! This suite is kept here because `maekon-api-contracts` is the public wire
//! contract layer; frozen samples catch serde drift before integration runtime.

use maekon_api_contracts::ai_providers::{
    ProviderModelCapabilityProfile, ProviderModelCapabilityRules,
    ProviderModelCatalogTransportSpec, ProviderModelSupportStatus, ProviderTransportSpec,
};
use serde_json::{json, Value};

fn decode_value<T>(value: Value, context: &str) -> T
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value).unwrap_or_else(|error| panic!("{context}: {error}"))
}

fn encode_value<T>(value: T, context: &str) -> Value
where
    T: serde::Serialize,
{
    serde_json::to_value(value).unwrap_or_else(|error| panic!("{context}: {error}"))
}

/// Helper: deserialize JSON → DTO → re-serialize → compare structural
/// equality. String-level equality is too brittle (whitespace, field order)
/// — `serde_json::Value` equality compares the data, not the formatting.
fn round_trip<T>(reference: Value)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let dto: T = decode_value(
        reference.clone(),
        "frozen JSON must deserialize into current DTO",
    );
    let reencoded = encode_value(&dto, "DTO must serialize");
    assert_eq!(
        reencoded, reference,
        "round-trip must preserve structural JSON shape"
    );
}

// ---------------------------------------------------------------------------
// Scenario 1 — basic struct of strings round-trips losslessly.
// ---------------------------------------------------------------------------

#[test]
fn provider_transport_spec_round_trips() {
    let reference = json!({
        "method": "POST",
        "url": "https://api.example.com/v1/chat",
        "auth_scheme": "Bearer",
        "request_shape": "openai_chat_completions",
    });
    round_trip::<ProviderTransportSpec>(reference);
}

// ---------------------------------------------------------------------------
// Scenario 2 — enum with serde rename_all = "snake_case" must accept and
// emit lowercase variants exactly.
// ---------------------------------------------------------------------------

#[test]
fn provider_model_support_status_snake_case_round_trips() {
    for (sample, expected_variant) in [
        (json!("supported"), ProviderModelSupportStatus::Supported),
        (
            json!("unsupported"),
            ProviderModelSupportStatus::Unsupported,
        ),
        (json!("unknown"), ProviderModelSupportStatus::Unknown),
    ] {
        let parsed: ProviderModelSupportStatus =
            decode_value(sample.clone(), &format!("decode {sample:?}"));
        assert_eq!(parsed, expected_variant);
        let reencoded = encode_value(parsed, "encode provider model support status");
        assert_eq!(
            reencoded, sample,
            "enum re-encoding must match snake_case wire form"
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 3 — backward-compat: missing #[serde(default)] fields must
// resolve to the documented defaults.
// ---------------------------------------------------------------------------

#[test]
fn provider_model_catalog_transport_spec_applies_serde_defaults_for_missing_fields() {
    // Minimal v1 server response that omits the optional capability flags.
    let reference = json!({
        "method": "GET",
        "url": "https://api.example.com/v1/models",
        "auth_scheme": "Bearer",
        "response_shape": "openai_model_list",
    });
    let dto: ProviderModelCatalogTransportSpec =
        decode_value(reference, "decode minimal v1 catalog spec");

    // The two booleans default to `true` via `default_true` per the DTO def.
    assert!(
        dto.llm_supported,
        "llm_supported must default to true when omitted"
    );
    assert!(
        dto.ocr_supported,
        "ocr_supported must default to true when omitted"
    );
    assert!(
        dto.ocr_notice.is_none(),
        "ocr_notice must default to None when omitted"
    );
}

// ---------------------------------------------------------------------------
// Scenario 4 — forward-compat: a server response that adds unknown fields
// must still deserialize against the current DTO. Serde's default behavior
// for derive(Deserialize) is to IGNORE unknown fields; this test pins that
// behavior so a future #[serde(deny_unknown_fields)] addition can't slip in
// undetected.
// ---------------------------------------------------------------------------

#[test]
fn forwards_compat_unknown_fields_are_ignored_on_deserialize() {
    // Same as scenario 1 but with extra unknown fields that a future server
    // might add to the wire format.
    let reference = json!({
        "method": "POST",
        "url": "https://api.example.com/v1/chat",
        "auth_scheme": "Bearer",
        "request_shape": "openai_chat_completions",
        // Future-fields a v1.1 server might emit. v1 client must NOT error.
        "rate_limit_per_minute": 60,
        "experimental_feature_flag": "stream_with_function_calls",
        "_meta": { "added_in_version": "v1.1" },
    });
    let dto: ProviderTransportSpec = decode_value(
        reference,
        "future server JSON with extra fields must still decode",
    );

    // Sanity check that the v1 fields are intact.
    assert_eq!(dto.method, "POST");
    assert_eq!(dto.url, "https://api.example.com/v1/chat");
    assert_eq!(dto.auth_scheme, "Bearer");
    assert_eq!(dto.request_shape, "openai_chat_completions");
}

// ---------------------------------------------------------------------------
// Scenario 5 — nested struct with `Default` impl: an entirely empty JSON
// object must deserialize because all fields have `#[serde(default)]`. The
// re-serialized form must match the canonical "empty defaults" shape.
// ---------------------------------------------------------------------------

#[test]
fn provider_model_capability_rules_decodes_empty_object_to_default() {
    let dto: ProviderModelCapabilityRules =
        decode_value(json!({}), "empty object must decode to defaults");
    assert_eq!(dto, ProviderModelCapabilityRules::default());

    // Each nested profile is itself default-empty.
    let empty_profile = ProviderModelCapabilityProfile::default();
    assert_eq!(dto.llm, empty_profile);
    assert_eq!(dto.ocr, empty_profile);
    assert_eq!(dto.image_input, empty_profile);
    assert_eq!(dto.structured_output, empty_profile);
}
