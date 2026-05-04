# AGENTS.md

Repository guardrails for humans and AI agents.
This project is a 15-package Cargo workspace (14 crates under `crates/` + `src-tauri`) following Hexagonal Architecture (Ports & Adapters). Keep boundaries explicit. (The parent ONESHIM project additionally uses DDD on the server side; this `client-rust` workspace is Hexagonal-only.)

## Scope

These rules apply to the entire repository unless a subdirectory adds stricter guidance.

## Architecture Summary

1. `oneshim-core` is the domain contract layer (ports, models, errors). 57 port files / 95 public traits / 38 `CoreError` variants typed per [ADR-019](docs/architecture/ADR-019-error-code-infrastructure.md).
2. Adapter crates (`oneshim-network`, `oneshim-storage`, `oneshim-monitor`, `oneshim-vision`, `oneshim-suggestion`, `oneshim-automation`, `oneshim-analysis`, `oneshim-embedding`, `oneshim-audio`, `oneshim-web`, `oneshim-api-contracts`) implement ports from `oneshim-core`.
3. `src-tauri/` (package name `oneshim-app`) is the composition root + runtime orchestrator. The former `crates/oneshim-app/` is DEPRECATED and removed from the workspace.
4. `oneshim-sandbox-worker` is a standalone binary crate (stdin JSON → stdout JSON under platform sandbox). Spawned out-of-process by `src-tauri` to execute `AutomationAction` with Windows Job Objects / Linux seccomp+Landlock / macOS App Sandbox constraints — isolates the main process from action-side crashes.
5. `oneshim-lint` is a standalone workspace tool (language-check binary) with no `oneshim-core` dependency.
6. `oneshim-web` is a delivery layer (HTTP handlers + frontend), not a domain layer.
7. `oneshim-automation` enforces policy, sandbox, audit, and action execution flows.

See:
- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `docs/crates/README.md`

## Non-Negotiable Rules

1. Preserve dependency direction.
2. Do not introduce direct adapter-to-adapter coupling outside approved exceptions.
3. Use `oneshim-core` ports/traits for cross-layer access instead of concrete adapter types.
4. Keep domain invariants in domain/application services, not in web handlers or UI code.
5. Keep web handlers thin: transport mapping, validation, and service orchestration only.
6. Keep orchestration responsibilities split; do not centralize unrelated concerns into a single large controller/scheduler method.
7. Privacy by default: external egress must pass policy + privacy gates, consent checks, and audit logging.
8. Do not send raw sensitive source data externally unless explicit policy/consent override paths allow it.
9. Frontend shared UI must use design tokens and shared UI primitives for consistency.
10. Frontend text must be i18n-driven; avoid hardcoded locale strings in test selectors.

## Language Policy

1. Primary language for code comments is English.
2. Primary language for logs (`tracing` and runtime diagnostics) is English.
3. User-facing UI text should be localized through i18n resources.

## Change Checklist

Before finalizing a change, verify:

1. Layer boundaries still match Hexagonal (Ports & Adapters) intent — this workspace is Hexagonal-only; DDD belongs to the parent-project server side.
2. New dependencies do not violate crate direction rules.
3. Policy/privacy/audit paths are preserved for automation and external integrations.
4. UI changes follow tokenized components and i18n conventions.
5. Relevant tests and build checks for touched modules are run.

