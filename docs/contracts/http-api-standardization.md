[English](./http-api-standardization.md) | [한국어](./http-api-standardization.ko.md)

# HTTP API Standardization (OpenAPI Track)

This document defines how ONESHIM standardizes local web interfaces with OpenAPI-compatible governance.

## Current SSOT

- Transport DTO SSOT: `crates/oneshim-api-contracts`
- Runtime route SSOT: `crates/oneshim-web/src/routes.rs`
- Machine-readable interface snapshot: `docs/contracts/http-interface-manifest.v1.json`
- Generated OpenAPI snapshot: `docs/contracts/oneshim-web.v1.openapi.yaml`

## Decision

1. Keep transport DTO ownership in `oneshim-api-contracts`; handlers must not define public DTOs.
2. Keep route ownership in `oneshim-web/src/routes.rs`.
3. Maintain a versioned interface manifest (`http.interface.manifest.v1`) as the bridge artifact.
4. Generate OpenAPI snapshot from manifest (`scripts/generate-http-openapi.sh`).
5. Enforce route/manifest/openapi sync in CI:
   - `scripts/verify-http-interface-manifest.sh`
   - `scripts/verify-http-openapi-sync.sh`
6. Use additive evolution and explicit version bumps for breaking HTTP contract changes.

## Why this shape

- Preserves DDD/Hexagonal boundaries without pushing web-only metadata into domain crates.
- Gives machine-checkable interface governance and reproducible OpenAPI snapshots.
- Supports provider growth (AI, automation, update, diagnostics) with explicit endpoint/module ownership.

## Automation status (implemented)

1. Build-time snapshot generation:
   - `./scripts/generate-http-openapi.sh`
2. CI sync gate for PR/push:
   - `./scripts/verify-http-openapi-sync.sh`
3. CI OpenAPI artifact publication:
   - artifact name: `oneshim-web-v1-openapi`
4. Release attachment publication:
   - file: `oneshim-web.v1.openapi.yaml`

## Non-goals (current phase)

1. No runtime dependency on OpenAPI generator.
2. No adapter/domain coupling changes.
3. No handler-level manual schema duplication.

## Follow-up improvements

1. Expand OpenAPI schema fidelity from generic object placeholders to DTO-level typed schema references.
2. Add explicit breaking-change classification policy for OpenAPI diff outputs.
