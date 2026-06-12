[English](./gui-session-acceptance-matrix.md) | [한국어](./gui-session-acceptance-matrix.ko.md)

# GUI Session Acceptance Matrix

This document defines the cross-OS GUI session acceptance matrix for Maekon
automation benchmarks.

## Contract version

- Matrix payload: `automation.gui.acceptance_matrix.v1`
- Canonical machine-readable matrix:
  `docs/contracts/gui-session-acceptance-matrix.v1.json`

## Domain types

The canonical Rust contract lives in:

- `maekon_core::models::gui::GuiSessionAcceptanceMatrix`
- `maekon_core::models::gui::GuiSessionAcceptanceCase`

`GuiSessionAcceptanceMatrix::validate_contract_coverage()` verifies the matrix
contains required lifecycle, geometry, execution-mode, and negative-path cases.

## Lifecycle coverage

The matrix covers the primary ADR-002 session flow:

| Stage | Required coverage |
|-------|-------------------|
| `propose` | Current-scene proposal with privacy-safe candidates. |
| `highlight` | Overlay geometry under scaling, multi-monitor layouts, negative origins, and primary-monitor fallback. |
| `confirm` | Focus revalidation before ticket issuance. |
| `execute` | `noop`, `dry_run_worker`, `sandboxed_real_input`, `direct_real_input`, and `unsupported` modes. |
| `verify` | Safe before/after state evidence or explicit degraded/unsupported status. |
| `audit` | Session, policy, and diagnostic evidence. |
| `timeout` | Session expiry and ticket issuance denial. |
| `cancel` | Terminal cancellation and later-call rejection. |

## Benchmark rules

- Default benchmark runs include only low-risk session-based cases.
- High-risk direct input cases are `excluded` unless an operator explicitly opts in.
- Legacy direct execution is separated from primary session success metrics.
- Real-input execution cases must include `before_after_state` evidence.
- Unsupported or degraded environments must report that state instead of faking parity.

## Negative cases

The matrix includes stale scene, stale bounding box, coordinate drift,
focus-window mismatch, and ticket/capability failure cases. These cases must
fail before real input is sent.

## Privacy rule

Evidence is display-safe. It may reference stable case IDs, session IDs, policy
decisions, geometry summaries, and diagnostic codes. It must not include raw
window titles, element text, screenshots, file paths, or user-entered text.
