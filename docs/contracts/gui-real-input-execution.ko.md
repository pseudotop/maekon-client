[English](./gui-real-input-execution.md) | [한국어](./gui-real-input-execution.ko.md)

# GUI Real Input Execution 계약

이 문서는 Maekon GUI automation benchmark에서 noop, dry-run worker execution,
real OS input execution의 경계를 정의한다.

## 계약 버전

- Execution payload: `automation.gui.real_input_execution.v1`
- Canonical machine-readable contract:
  `docs/contracts/gui-real-input-execution.v1.json`

## 도메인 타입

정식 Rust 계약은 다음 타입에 있다.

- `maekon_core::models::gui::GuiInputExecutionModeReason`
- `maekon_core::models::gui::GuiSandboxWorkerExecutionContract`
- `maekon_core::models::gui::GuiRealInputExecutionContract`

`GuiRealInputExecutionContract::validate_contract_coverage()`는 dry-run, real-input,
native-injector delegation, unsupported worker path가 모두 표현되는지 검증한다.

## Execution mode

| Mode | 의미 |
|------|------|
| `noop` | Harness가 의도적으로 input을 수행하지 않는다. Real-input case를 pass할 수 없다. |
| `dry_run_worker` | Worker가 의도된 작업을 기록하거나 audit하지만 OS input을 inject하지 않는다. |
| `sandboxed_real_input` | Sandbox worker가 real input을 실행하거나 OS-native injector에 위임한다. |
| `direct_real_input` | Client process가 승인된 OS input을 직접 inject한다. |
| `unsupported` | 현재 platform/runtime이 real input을 실행할 수 없다. |
| `unknown` | Runner가 execution mode를 분류할 수 없다. |

## Verification 규칙

`command_accepted`는 dispatch 완료만 증명한다. GUI가 바뀌었다는 증명은 아니다.
Real-input benchmark pass result에는 다음이 필요하다.

- `sandboxed_real_input` 또는 `direct_real_input` input_execution_mode
- `observable_state_change` execution_verification_mode
- before/after state evidence, focus binding, audit outcome
- privacy-safe evidence만 사용

## Sandbox worker 계약

| Worker kind | 필수 mode | 필수 verification |
|-------------|-----------|-------------------|
| `dry_run_logging` | `dry_run_worker` | `command_accepted` 또는 `none` |
| `executes_real_input` | `sandboxed_real_input` | `observable_state_change` |
| `delegates_to_native_injector` | `sandboxed_real_input` | `observable_state_change` |
| `unsupported` | `unsupported` | `none` |

Real worker path는 policy approval이 필요하고 audit evidence를 emit해야 한다.
Dry-run path는 smoke test에는 유용할 수 있지만 observable state change를 증명할 수
없으면 benchmark result를 degraded, skipped, blocked, 또는 unsupported로 분류해야 한다.
