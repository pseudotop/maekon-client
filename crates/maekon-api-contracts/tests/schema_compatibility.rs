//! Frozen-schema regression tests for the schemars-generated OpenAPI bodies
//! (E20-17 #4809).
//!
//! The GitHub Actions diff gate is intentionally billing-disabled
//! (`project_ci_disabled_for_cost`), so these `cargo test` assertions are the
//! regression substitute alongside `scripts/verify-http-openapi-sync.sh`. They
//! freeze a representative slice of the emitted JSON Schema so that:
//!
//!   * a forgotten `#[cfg_attr(... derive(JsonSchema))]` (schema disappears),
//!   * a cross-crate containment gap (a `maekon-core` type leaks a full schema
//!     instead of an opaque object), or
//!   * an accidental field rename / type change
//!
//! is caught at test time rather than silently corrupting the public snapshot.
//!
//! Only built under `--features schema` (the feature that the OpenAPI generator
//! uses). Normal client builds never compile this.
#![cfg(feature = "schema")]

use schemars::schema_for;
use serde_json::{json, Value};

/// `CreateAnnotationRequest` embeds the cross-crate `maekon-core`
/// `AnnotationType` string enum. Containment must render it as an opaque
/// string (NOT a `$ref` into a leaked `AnnotationType` definition).
#[test]
fn create_annotation_request_contains_annotation_type_as_opaque_string() {
    let schema = schema_for!(maekon_api_contracts::annotations::CreateAnnotationRequest);
    let value = serde_json::to_value(&schema).expect("schema serializes");

    let annotation_type = &value["properties"]["annotation_type"];
    assert_eq!(
        annotation_type["type"], "string",
        "annotation_type must be an opaque string (build containment), got: {annotation_type}"
    );
    // The opaque field must NOT introduce a `$ref` into maekon-core.
    assert!(
        annotation_type.get("$ref").is_none(),
        "annotation_type must not $ref a leaked maekon-core schema"
    );

    // The crate-local scalar fields keep their real shape.
    assert_eq!(value["properties"]["x"]["type"], "number");
    assert_eq!(value["properties"]["y"]["type"], "number");
}

/// `PresetListDto` embeds `Vec<WorkflowPreset>` (a `maekon-core` type).
/// Containment must render an array of opaque objects.
#[test]
fn preset_list_dto_presets_is_array_of_opaque_objects() {
    let schema = schema_for!(maekon_api_contracts::automation::PresetListDto);
    let value = serde_json::to_value(&schema).expect("schema serializes");

    let presets = &value["properties"]["presets"];
    assert_eq!(presets["type"], "array", "presets must be an array");
    assert_eq!(
        presets["items"]["type"], "object",
        "presets items must be opaque objects (build containment), got: {}",
        presets["items"]
    );
}

/// A fully crate-local DTO must emit its real structure (no containment).
/// Freezes the `ConnectionStatusDto` shape as a representative wire contract.
#[test]
fn connection_status_dto_emits_full_structure() {
    let schema = schema_for!(maekon_api_contracts::bug_report::ConnectionStatusDto);
    let value = serde_json::to_value(&schema).expect("schema serializes");

    assert_eq!(value["type"], "object");
    let props = &value["properties"];
    assert_eq!(props["server_reachable"]["type"], "boolean");
    assert_eq!(props["grpc_enabled"]["type"], "boolean");
    assert_eq!(props["websocket_connected"]["type"], "boolean");
    // `last_sync_at: Option<String>` → nullable-ish string union; just assert it
    // is present and references string somewhere (avoid over-freezing the exact
    // schemars Option encoding).
    assert!(
        props.get("last_sync_at").is_some(),
        "last_sync_at must be present in the schema"
    );

    // `bug_report` embeds `PiiFilterLevel` on the bundle, not here — sanity that
    // this DTO has no leaked maekon-core ref.
    assert!(
        !value.to_string().contains("PiiFilterLevel"),
        "ConnectionStatusDto must not reference maekon-core PiiFilterLevel"
    );
}

/// `IntegrationSessionSummary` embeds three `maekon-core` string enums
/// (status / transport_kind / auth_scheme). All three must be opaque strings.
#[test]
fn integration_session_summary_enums_are_opaque_strings() {
    let schema = schema_for!(maekon_api_contracts::integration::IntegrationSessionSummary);
    let value = serde_json::to_value(&schema).expect("schema serializes");
    let props = &value["properties"];

    for field in ["status", "transport_kind", "auth_scheme"] {
        assert_eq!(
            props[field]["type"], "string",
            "{field} must be an opaque string (build containment), got: {}",
            props[field]
        );
        assert!(
            props[field].get("$ref").is_none(),
            "{field} must not $ref a leaked maekon-core schema"
        );
    }
}

/// Guard the exact opaque-object containment description so a future refactor
/// of `schema_support` that silently widens the schema (e.g. to `true`) is
/// noticed. This is the single most load-bearing containment marker.
#[test]
fn opaque_containment_marker_is_stable() {
    let schema = schema_for!(maekon_api_contracts::automation::ExecuteIntentHintResponse);
    let value = serde_json::to_value(&schema).expect("schema serializes");

    // `planned_intent: AutomationIntent` and `result: IntentResult` are both
    // contained as opaque objects.
    for field in ["planned_intent", "result"] {
        let f = &value["properties"][field];
        assert_eq!(
            *f,
            json!({
                "type": "object",
                "description": "Opaque maekon-core type (build containment; not derived to avoid a cross-crate schemars dependency)."
            }),
            "containment schema for `{field}` drifted"
        );
    }
}

/// The emitted schema must be valid JSON (defensive — `schema_for!` could in
/// principle produce a non-object root if the derive regressed).
#[test]
fn representative_schema_is_a_json_object() {
    let schema = schema_for!(maekon_api_contracts::settings::AppSettings);
    let value: Value = serde_json::to_value(&schema).expect("schema serializes");
    assert!(value.is_object(), "root schema must be a JSON object");
    assert_eq!(value["type"], "object");
}
