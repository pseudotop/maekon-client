# oneshim-api-contracts

`oneshim-api-contracts` centralizes transport-layer DTOs for the local web API.

## Scope

- Shared request/response structs for web handlers and services.
- Serde defaults used for backward-compatible deserialization.
- Versioned contract modules consumed by OpenAPI snapshot automation.
- OpenAPI standardization bridge via interface manifest:
  - `docs/contracts/http-interface-manifest.v1.json`
  - `docs/contracts/oneshim-web.v1.openapi.yaml`
  - `docs/contracts/http-api-standardization.md`

## Current Modules (44 total)

- **Infrastructure**: `common` (shared query/pagination), `error` (API error payloads), `lib` (crate root).
- **Settings & Providers**: `settings` (app settings, storage stats, remote provider endpoints), `ai_providers` (provider preset catalog + model discovery), `provider_specs/` (directory module — AI provider specifications: enums, models, helpers, parsers, queries, resolvers, validation, tests).
- **Core telemetry**: `metrics`, `processes`, `frames`, `events`, `stats`, `reports`, `search`, `sessions`, `idle`.
- **Tagging & presentation**: `tags`, `timeline`, `focus`, `data`, `export`, `backup`, `onboarding`, `support`.
- **Automation & GUI**: `automation`, `automation_gui`, `stream`, `update`.
- **AI pipeline**: `ai_session`, `annotations`, `coaching`, `daily_digest`, `digests`, `suggestions`.
- **Integrations & workflows**: `bug_report`, `dashboard`, `integration`, `playbooks`, `pomodoro`, `recalibration`.

## Design Rules

1. Keep business/domain logic out of this crate.
2. Keep dependencies minimal (`serde` only by default).
3. Use additive schema evolution for compatibility.
4. Treat this crate as the transport SSOT for `oneshim-web` APIs.
5. Keep `oneshim-web/src/handlers` free of public DTO definitions (enforced by `scripts/verify-web-contract-boundary.sh`).
6. Keep route/interface manifest sync green in CI (`scripts/verify-http-interface-manifest.sh`).
7. Keep generated OpenAPI snapshot sync green in CI (`scripts/verify-http-openapi-sync.sh`).
