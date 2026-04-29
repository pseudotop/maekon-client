[English](./README.md) | [한국어](./README.ko.md)

# Architecture Decision Records (ADR) Registry

Architecture Decision Records capture single, named architectural decisions for the `client-rust` workspace. They are the authoritative record of *what* was decided and *why*. Implementation detail belongs in companion implementation records, not in the ADR itself.

## Writing a new ADR

1. Read [`ADR-TEMPLATE.md`](./ADR-TEMPLATE.md) (or the Korean companion `ADR-TEMPLATE.ko.md`).
2. Copy it to `ADR-XXX-<kebab-case-title>.md` using the next unused ID from the registry below.
3. Fill all required header fields and the Context / Decision / Consequences / Alternatives sections.
4. Open a PR; the `Status` starts at `Draft` or `Proposed`.
5. On approval, change `Status` to `Accepted`. If you implemented it at the same time, note the code pointer in the `Implementation` field.
6. Register the new ADR in the table below.

## Registry

| ID | Title | Status | Scope |
|----|-------|--------|-------|
| [001](./ADR-001-rust-client-architecture-patterns.md) | Rust Client Architecture Patterns | Accepted | Entire workspace |
| [002](./ADR-002-os-gui-interaction-boundary.md) | OS GUI Interaction Boundary and Runtime Split | Accepted | core / automation / vision / web / src-tauri |
| [003](./ADR-003-directory-module-pattern.md) | Directory Module Pattern for Large Source Files | Accepted | All crates |
| [004](./ADR-004-tauri-v2-migration.md) | Tauri v2 Migration (iced → Tauri v2 + WebView) | Accepted | Desktop shell |
| [005](./ADR-005-tauri-governance.md) | Tauri v2 Governance | Accepted | `src-tauri/tauri.conf.json`, permissions |
| [006](./ADR-006-ipc-command-contract.md) | Tauri IPC Command Contract | Accepted | `src-tauri/src/commands/` |
| [007](./ADR-007-async-runtime-safety-patterns.md) | Async Runtime Safety Patterns | Accepted | All tokio-using crates |
| [008](./ADR-008-network-resilience-patterns.md) | Network Resilience Patterns | Accepted | `oneshim-network` |
| [009](./ADR-009-client-architecture-baseline.md) | Client Architecture Baseline | Accepted | `oneshim-app` package, web, integration runtime |
| [010](./ADR-010-local-integration-harness-boundary.md) | Local Integration Harness Boundary | Accepted | Integration harness |
| [011](./ADR-011-standalone-analysis-pipeline.md) | Standalone Analysis Pipeline | Accepted | `oneshim-analysis`, AnalysisProvider port |
| [012](./ADR-012-adaptive-tiered-memory.md) | Adaptive Tiered Memory | Accepted | Adaptive trigger, regime manager |
| [013](./ADR-013-llm-summary-vector-rag.md) | LLM Segment Summary + Vector RAG | Accepted | Embedding pipeline, vector store |
| [014](./ADR-014-tauri-managed-state-boundary.md) | Tauri Managed State Boundary | Accepted | `src-tauri` managed state |
| [015](./ADR-015-frame-storage-port.md) | Frame Storage Port Abstraction | Accepted | core ports, storage adapter |
| [016](./ADR-016-config-change-bus.md) | Config Change Bus | Accepted | `ConfigManager`, runtime subscribers |
| [017](./ADR-017-feedback-signal-sink.md) | FeedbackSignalSink | Accepted | core port, suggestion, analysis |
| [018](./ADR-018-regime-manager-persistence.md) | RegimeManager Persistence | Accepted | core port, storage, analysis |
| [019](./ADR-019-error-code-infrastructure.md) | Error Code Infrastructure + AWS Bedrock Intentional Non-Support | Accepted | All crates — wire-format error codes |

**Next available ID**: `ADR-020`.

## Conventions Summary

See [`ADR-TEMPLATE.md`](./ADR-TEMPLATE.md) for the full authoritative template. Key rules:

1. **Filename**: `ADR-XXX-<kebab-case-title>.md`; Korean companion adds `.ko.md`.
2. **Header fields** (in order): `Status`, `Date`, `Scope`, optional `Supersedes` / `Superseded by` / `Related` / `Implementation`.
3. **Status keywords**: `Draft` → `Proposed` → `Accepted`. Terminal states: `Superseded` (link to new ADR), `Deprecated` (no replacement; rationale required).
4. **Do not silently edit an `Accepted` ADR.** Material changes go in a new ADR (with `Supersedes`) or a `## Update YYYY-MM-DD` sub-section.
5. **Minimum sections**: Context, Decision, Consequences, Alternatives Considered.
6. **Code examples**: show the canonical form, not exhaustive usage. Link to real code via `crates/.../path.rs:line` rather than inlining hundreds of lines.
7. **Korean companion**: required for ADRs that public contributors are likely to read. Ops-internal ADRs may skip the companion if the team is English-first.

## Status Keyword Reference

| Keyword | Meaning |
|---------|---------|
| `Draft` | Author is still iterating; not yet ready for review. |
| `Proposed` | Ready for review; subject to change. |
| `Accepted` | Approved and in force. New decisions in this area must either comply or supersede. |
| `Superseded` | Replaced by a later ADR (link via `Superseded by`). Kept for historical record. |
| `Deprecated` | No longer in force but no direct replacement. Explain rationale in the doc body. |

Avoid `Approved` — it was historically used in a few ADRs but has no consistent meaning distinct from `Accepted`. The ADR-019 drift audit (iter-186) unified everything to `Accepted`.

## Relationship to implementation records

- **`docs/architecture/`** (this directory) — the *what and why* of an architectural decision.
- Companion implementation records — the *how* of a specific change. These may live outside the public-minimal export when they are planning artifacts rather than durable public documentation.

When a new ADR requires implementation, the typical flow is:

```
Draft ADR → Proposed ADR → companion implementation record
                       → Implementation PRs
Accept ADR  (with `Implementation:` pointer to public implementation records + code paths)
```
