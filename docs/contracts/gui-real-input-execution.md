[English](./gui-real-input-execution.md) | [한국어](./gui-real-input-execution.ko.md)

# GUI Real Input Execution Contract

This document defines the boundary between noop, dry-run worker execution, and
real OS input execution for Maekon GUI automation benchmarks.

## Contract version

- Execution payload: `automation.gui.real_input_execution.v1`
- Canonical machine-readable contract:
  `docs/contracts/gui-real-input-execution.v1.json`

## Domain types

The canonical Rust contract lives in:

- `maekon_core::models::gui::GuiInputExecutionModeReason`
- `maekon_core::models::gui::GuiSandboxWorkerExecutionContract`
- `maekon_core::models::gui::GuiRealInputExecutionContract`

`GuiRealInputExecutionContract::validate_contract_coverage()` verifies that
dry-run, real-input, native-injector delegation, and unsupported worker paths
are all represented.

## Execution modes

| Mode | Meaning |
|------|---------|
| `noop` | The harness intentionally performs no input. It cannot pass real-input cases. |
| `dry_run_worker` | The worker records or audits intended work but does not inject OS input. |
| `sandboxed_real_input` | A sandbox worker executes real input or delegates to an OS-native injector. |
| `direct_real_input` | The client process injects approved OS input directly. |
| `unsupported` | The current platform/runtime cannot execute real input. |
| `unknown` | The runner cannot classify the execution mode. |

## Verification rule

`command_accepted` proves only that dispatch completed. It does not prove that
the GUI changed. Real-input benchmark pass results require:

- `input_execution_mode` of `sandboxed_real_input` or `direct_real_input`
- `execution_verification_mode` of `observable_state_change`
- before/after state evidence, focus binding, and audit outcome
- privacy-safe evidence only

## Sandbox worker contract

| Worker kind | Required mode | Required verification |
|-------------|---------------|-----------------------|
| `dry_run_logging` | `dry_run_worker` | `command_accepted` or `none` |
| `executes_real_input` | `sandboxed_real_input` | `observable_state_change` |
| `delegates_to_native_injector` | `sandboxed_real_input` | `observable_state_change` |
| `unsupported` | `unsupported` | `none` |

Real worker paths must require policy approval and emit audit evidence. Dry-run
paths may be useful for smoke tests, but benchmark results must classify them as
degraded, skipped, blocked, or unsupported when observable state change cannot be
proven.
