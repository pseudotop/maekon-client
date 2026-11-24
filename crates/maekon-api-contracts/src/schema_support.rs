//! Build containment for `schemars` JSON Schema generation.
//!
//! The contract DTOs embed a handful of `maekon-core` types as fields (e.g.
//! `WorkflowPreset`, `IntentResult`, `PiiFilterLevel`, GUI session types, the
//! integration cursor/status types). We deliberately do NOT derive
//! `JsonSchema` on `maekon-core` — that would force a cross-crate `schemars`
//! dependency on the entire core domain crate and pull schemars into the
//! normal build closure of the 14 client crates.
//!
//! Instead, every cross-crate field site is annotated with
//! `#[cfg_attr(feature = "schema", schemars(schema_with = "..."))]` pointing at
//! one of the helpers below. The derived schema for the *owning* contract DTO
//! still emits its full structure (= the intended full disclosure); only the
//! embedded core type is rendered as an opaque object. This is a technical
//! build-containment necessity, NOT a privacy/opacity choice.
//!
//! Schema generation and unit tests compile this module. Normal runtime builds
//! keep schemars out of the dependency graph.

use schemars::{json_schema, Schema, SchemaGenerator};

/// Renders a cross-crate `maekon-core` field as an opaque JSON object
/// (`{ "type": "object" }`). Use on object-shaped fields (structs/maps).
pub fn opaque_object(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "object",
        "description": "Opaque maekon-core type (build containment; not derived to avoid a cross-crate schemars dependency)."
    })
}

/// Renders a cross-crate `maekon-core` field whose JSON form is an array of
/// opaque objects (`{ "type": "array", "items": { "type": "object" } }`).
pub fn opaque_object_array(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "array",
        "items": { "type": "object" },
        "description": "Array of opaque maekon-core types (build containment)."
    })
}

/// Renders a cross-crate `maekon-core` enum that serializes as a plain string
/// (e.g. `PiiFilterLevel`, `AnnotationType`, `IntegrationSessionStatus`,
/// `IntegrationTransportKind`). Kept as `{ "type": "string" }` so consumers
/// still see the wire-level shape without recursing into core.
pub fn opaque_string_enum(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "description": "Opaque maekon-core string enum (build containment)."
    })
}

/// Renders a cross-crate `maekon-core` string enum used in a `Vec<_>` /
/// `Option<Vec<_>>` field as `{ "type": "array", "items": { "type": "string" } }`.
pub fn opaque_string_array(_generator: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "array",
        "items": { "type": "string" },
        "description": "Array of opaque maekon-core string enums (build containment)."
    })
}

/// UTC timestamps serialize as RFC 3339 strings in the public API.
pub fn utc_timestamp_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": "string", "format": "date-time"})
}

pub fn optional_utc_timestamp_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({"type": ["string", "null"], "format": "date-time"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamp_schemas_preserve_format_and_nullability() {
        let mut generator = SchemaGenerator::default();
        assert_eq!(
            utc_timestamp_schema(&mut generator).as_value(),
            &serde_json::json!({"type": "string", "format": "date-time"})
        );
        assert_eq!(
            optional_utc_timestamp_schema(&mut generator).as_value(),
            &serde_json::json!({"type": ["string", "null"], "format": "date-time"})
        );
    }
}
