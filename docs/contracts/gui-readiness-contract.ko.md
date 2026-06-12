[English](./gui-readiness-contract.md) | [한국어](./gui-readiness-contract.ko.md)

# GUI 준비 상태 계약

이 문서는 Maekon Client 벤치마크 게이트와 진단에 사용하는 OS 중립
GUI 준비 상태 및 capability snapshot 계약을 정의한다.

## 계약 버전

- Snapshot payload: `automation.gui.readiness.v1`

## 도메인 타입

정식 Rust 계약은 다음 타입에 있다.

- `maekon_core::models::gui::GuiReadinessSnapshot`
- `maekon_api_contracts::automation_gui::GuiReadinessResponse`

## Capability 상태

모든 capability는 같은 상태 어휘를 사용한다.

| State | 의미 |
|-------|------|
| `available` | capability가 존재하며 사용할 수 있다. |
| `degraded` | capability를 사용할 수 있지만 알려진 제약이 있다. |
| `unavailable` | 해당 플랫폼에서 기대되는 capability지만 현재 사용할 수 없다. |
| `denied` | OS, 정책, 또는 사용자 동의가 capability를 차단한다. |
| `unsupported` | 해당 플랫폼 또는 런타임에서 capability를 지원하지 않는다. |

## Snapshot 필드

| Field | 목적 |
|-------|------|
| `platform` | `macos`, `windows`, `linux`, 또는 `unknown`. |
| `automation_enabled` | 런타임 설정이 GUI automation을 허용한다. |
| `controller_built` | 현재 프로세스에 automation controller가 생성되어 있다. |
| `gui_service_configured` | GUI ticket/service 설정이 존재한다. |
| `hmac_secret_present` | `MAEKON_GUI_TICKET_HMAC_SECRET` 또는 동등한 secret이 존재한다. |
| `input_execution_mode` | `noop`, `dry_run_worker`, `sandboxed_real_input`, `direct_real_input`, `unsupported`, 또는 `unknown`. |
| `input_execution_reason` | 해당 effective mode가 선택된 이유. 예: `sandbox_worker_dry_run`, `sandbox_worker_real_input`, `direct_native_input`, `permission_denied`, `unsupported_platform`. |
| `execution_verification_mode` | `none`, `command_accepted`, `observable_state_change`, 또는 `unknown`. |
| `session_constraints` | `foreground_only`, `locked_session_unsupported` 같은 typed platform constraint. |
| `capabilities` | screen visibility, accessibility extraction, OCR fallback, overlay, input execution, permissions, sandbox support, audit, privacy policy capability matrix. |
| `diagnostics` | UI와 benchmark report에 표시할 privacy-safe 진단 행. |

## Benchmark 결정

Consumer는 `GuiReadinessSnapshot::benchmark_decision()`을 사용할 수 있다.

| Decision | 의미 |
|----------|------|
| `run` | Real-input benchmark를 실행할 수 있고 관측 가능한 UI 상태 변경을 증명할 수 있다. |
| `skip` | 환경이 real GUI input을 안전하게 증명할 수 없지만 정책/권한 실패는 아니다. |
| `fail` | 필수 설정, 동의, 정책, 또는 권한이 없거나 거부되었다. |

`command_accepted`만으로는 `run`이 될 수 없다. Real-input benchmark는 effective
input mode가 `sandboxed_real_input` 또는 `direct_real_input`이고 verification이
`observable_state_change`일 때만 run 가능하다.

## Privacy 규칙

Diagnostics는 표시 전용이며 raw window title, element text, file path,
screenshot, user-entered text를 포함하면 안 된다. 안정적인 diagnostic code와
localization/remediation key를 사용한다.
