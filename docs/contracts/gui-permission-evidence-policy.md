[English](./gui-permission-evidence-policy.md) | [한국어](./gui-permission-evidence-policy.ko.md)

# GUI Permission and Evidence Policy

This document defines the cross-OS permission remediation categories and
privacy-safe evidence rules for Maekon GUI automation benchmarks.

## Contract version

- Policy payload: `automation.gui.permission_evidence.v1`
- Canonical machine-readable policy:
  `docs/contracts/gui-permission-evidence-policy.v1.json`

## Domain types

The canonical Rust contract lives in:

- `maekon_core::models::gui::GuiPermissionEvidencePolicy`
- `maekon_core::models::gui::GuiPermissionRemediationRule`
- `maekon_core::models::gui::GuiEvidenceArtifactPolicyRule`
- `maekon_core::models::gui::GuiEvidenceReviewRequest`

`GuiPermissionEvidencePolicy::validate_contract_coverage()` verifies required
permission kinds, platform-specific names, evidence artifact rules, and opt-in
override requirements.

## Permission categories

| Permission | Requirement | Purpose |
|------------|-------------|---------|
| `screen_capture` | `required` | Observe the target GUI state when the benchmark needs visual confirmation. |
| `accessibility` | `required` | Extract local accessibility metadata for matching and focus validation. |
| `automation_input_control` | `required` | Execute approved GUI input through the configured mode. |
| `notifications` | `recommended` | Surface non-blocking benchmark or remediation notices. |
| `local_service_reachability` | `required` | Reach the local GUI ticket or worker service over loopback. |
| `ocr_capability` | `recommended` | Provide an OCR fallback when accessibility metadata is incomplete. |

Each permission rule includes macOS, Windows, and Linux display names plus a
safe remediation key. The policy only describes remediation. It does not grant,
revoke, or bypass OS permissions.

## Evidence artifact rules

| Artifact | Default audience | Shareable by default | Rule |
|----------|------------------|----------------------|------|
| `broad_screenshot` | `local_only` | No | Rejected by benchmark runners unless an explicit opt-in override documents necessity, retention, and deletion. |
| `cropped_region` | `shareable_benchmark_artifact` | Yes | Geometry-only crop that excludes unrelated desktop content. |
| `text_metadata` | `shareable_benchmark_artifact` | Yes | Masked scene labels only. |
| `audit_excerpt` | `audit_only` | No | Diagnostic codes and policy decisions only. |
| `log_excerpt` | `shareable_benchmark_artifact` | Yes | Diagnostic codes only. |
| `worker_log` | `local_only` | No | Requires explicit opt-in override before sharing and must not contain raw UI payloads. |
| `gui_session_event` | `shareable_benchmark_artifact` | Yes | Masked scene labels and stable IDs only. |
| `benchmark_report` | `shareable_benchmark_artifact` | Yes | Masked scene labels, stable IDs, and summary metrics only. |
| `raw_accessibility_label` | `local_only` | No | Local matching only. Never shareable, even with override. |

## Opt-in override requirements

An override for a non-shareable artifact must confirm all of:

| Requirement | Meaning |
|-------------|---------|
| `necessity_justification` | Why a safer artifact cannot prove the benchmark result. |
| `retention_policy` | How long the artifact may exist and where it may be stored. |
| `deletion_plan` | How the artifact will be deleted after review. |

Broad screenshots, raw candidate labels, credentials, account settings, payment
flows, security prompts, and unrelated desktop content are excluded from default
artifacts. Raw labels may be used only for local matching when policy allows.

## Benchmark runner gates

- Required permission denial or unavailability is a benchmark failure unless the
  readiness contract has already classified the environment as unsupported.
- Recommended permission denial may degrade or skip the benchmark, but must not
  be reported as full parity.
- Broad screenshots are rejected unless `opt_in_override` is true and all
  override requirements are confirmed.
- Shareable artifacts must not include raw window titles, raw element text,
  typed user text, file paths, screenshots, credentials, or security prompts.
- Audit records, logs, session events, and reports use masked scene labels,
  diagnostic codes, stable IDs, and geometry summaries by default.
