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
//! The module itself is gated at the `lib.rs` declaration
//! (`#[cfg(feature = "schema")] pub mod schema_support;`), so no inner
//! `#![cfg(...)]` is needed here.

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
