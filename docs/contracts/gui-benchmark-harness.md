[English](./gui-benchmark-harness.md) | [한국어](./gui-benchmark-harness.ko.md)

# GUI Benchmark Harness Contract

This document defines the common benchmark harness contract for Maekon GUI
automation execution-plane ports and OS launchers.

## Contract version

- Harness payload: `automation.gui.benchmark_harness.v1`
- Canonical machine-readable catalog:
  `docs/contracts/gui-benchmark-harness.v1.json`

## Domain types

The canonical Rust contract lives in:

- `maekon_core::models::gui::GuiBenchmarkHarnessCatalog`
- `maekon_core::models::gui::GuiBenchmarkCase`
- `maekon_core::models::gui::GuiBenchmarkResult`
- `maekon_core::ports::gui_benchmark::GuiBenchmarkAdapter`

`GuiBenchmarkHarnessCatalog::validate_contract_coverage()` verifies the shared
case catalog. `GuiBenchmarkHarnessCatalog::validate_result()` rejects pass
results that have empty evidence or claim real-input success without observable
state-change verification.

## Shared case stages

| Stage | Required coverage |
|-------|-------------------|
| `launcher_readiness` | OS launcher discovery, process cleanup policy, and WebDriver readiness. |
| `focus` | Focus probe and execution binding validation. |
| `scene_extraction` | Scene extraction with masked or cropped evidence only. |
| `candidate_extraction` | Candidate ranking with latency, confidence, and failure mode. |
| `overlay_lifecycle` | Show and clear overlay using geometry-only evidence. |
| `input_action` | Approved input execution with observable state-change verification. |
| `verification` | Before/after state proof without raw sensitive UI payloads. |
| `audit` | Audit/session evidence using diagnostic codes and masked scene labels. |

## Result schema

Every case uses the same result fields:

| Field | Purpose |
|-------|---------|
| `outcome` | `pass`, `fail`, `skip`, `degraded`, `blocked`, or `unsupported`. |
| `latency_ms` | Case latency when measured. |
| `confidence` | Adapter confidence when applicable. |
| `failure_mode` | Typed failure reason for non-pass outcomes. |
| `evidence_path` | Stable artifact reference. Empty evidence cannot pass. |
| `privacy_status` | `safe`, `redacted`, `local_only`, or `rejected`. |
| `input_execution_mode` | Effective mode from the readiness contract. |
| `verification_mode` | How execution was verified. |
| `launcher_platform` | `macos`, `windows`, `linux`, or `unknown`. |

## Adapter rule

OS-specific implementations expose `GuiBenchmarkAdapter` and run shared
`GuiBenchmarkCase` values. The harness depends on the adapter trait and shared
models only. Adapters must not call or import each other.

## Runner gates

- Required evidence paths and artifact kinds must be present before a case can
  be summarized as pass.
- `noop`, `dry_run_worker`, `unsupported`, and `unknown` execution modes cannot
  pass a case that requires observable state change.
- `input_action` pass results must use `observable_state_change` verification.
- Launcher cases must record `launcher_platform`, including Windows
  `maekon.exe` discovery and WebDriver readiness.
- Unix-only process assumptions such as `pkill`, `SIGTERM`, `SIGKILL`, or
  extensionless binary names are not valid cross-OS evidence.
- Shareable artifacts must follow the GUI permission and evidence policy.
