[English](./gui-readiness-contract.md) | [한국어](./gui-readiness-contract.ko.md)

# GUI Readiness Contract

This document defines the OS-neutral GUI readiness and capability snapshot
contract for Maekon Client benchmark gates and diagnostics.

## Contract version

- Snapshot payload: `automation.gui.readiness.v1`

## Domain types

The canonical Rust contract lives in:

- `maekon_core::models::gui::GuiReadinessSnapshot`
- `maekon_api_contracts::automation_gui::GuiReadinessResponse`

## Capability states

Every capability uses the same state vocabulary:

| State | Meaning |
|-------|---------|
| `available` | Capability is present and usable. |
| `degraded` | Capability is usable with known limitations. |
| `unavailable` | Capability is expected on the platform but not currently available. |
| `denied` | OS, policy, or user consent blocks the capability. |
| `unsupported` | Capability is not supported for the platform/runtime. |

## Snapshot fields

| Field | Purpose |
|-------|---------|
| `platform` | `macos`, `windows`, `linux`, or `unknown`. |
| `automation_enabled` | Runtime config allows GUI automation. |
| `controller_built` | The automation controller exists in this process. |
| `gui_service_configured` | GUI ticket/service configuration is present. |
| `hmac_secret_present` | `MAEKON_GUI_TICKET_HMAC_SECRET` or equivalent secret is available. |
| `input_execution_mode` | `noop`, `dry_run_worker`, `sandboxed_real_input`, `direct_real_input`, `unsupported`, or `unknown`. |
| `input_execution_reason` | Why that effective mode was selected, for example `sandbox_worker_dry_run`, `sandbox_worker_real_input`, `direct_native_input`, `permission_denied`, or `unsupported_platform`. |
| `execution_verification_mode` | `none`, `command_accepted`, `observable_state_change`, or `unknown`. |
| `session_constraints` | Typed platform constraints such as `foreground_only` or `locked_session_unsupported`. |
| `capabilities` | Capability matrix for screen visibility, accessibility extraction, OCR fallback, overlay, input execution, permissions, sandbox support, audit, and privacy policy. |
| `diagnostics` | Privacy-safe display rows for UI and benchmark reports. |

## Benchmark decision

Consumers can use `GuiReadinessSnapshot::benchmark_decision()`:

| Decision | Meaning |
|----------|---------|
| `run` | Real-input benchmark can run and can prove observable UI state change. |
| `skip` | The environment cannot prove real GUI input safely, but this is not a policy/permission failure. |
| `fail` | Required config, consent, policy, or permission is missing or denied. |

`command_accepted` is not enough for `run`. A real-input benchmark can run only
when the effective input mode is `sandboxed_real_input` or `direct_real_input`
and verification is `observable_state_change`.

## Privacy rule

Diagnostics are display-only and MUST NOT contain raw window titles, element
text, file paths, screenshots, or user-entered text. Use stable diagnostic
codes plus localization/remediation keys instead.
