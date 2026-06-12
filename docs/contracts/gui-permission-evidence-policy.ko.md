[English](./gui-permission-evidence-policy.md) | [한국어](./gui-permission-evidence-policy.ko.md)

# GUI 권한 및 Evidence 정책

이 문서는 Maekon GUI automation benchmark에 사용하는 cross-OS 권한 보정
분류와 privacy-safe evidence 규칙을 정의한다.

## 계약 버전

- Policy payload: `automation.gui.permission_evidence.v1`
- Canonical machine-readable policy:
  `docs/contracts/gui-permission-evidence-policy.v1.json`

## 도메인 타입

정식 Rust 계약은 다음 타입에 있다.

- `maekon_core::models::gui::GuiPermissionEvidencePolicy`
- `maekon_core::models::gui::GuiPermissionRemediationRule`
- `maekon_core::models::gui::GuiEvidenceArtifactPolicyRule`
- `maekon_core::models::gui::GuiEvidenceReviewRequest`

`GuiPermissionEvidencePolicy::validate_contract_coverage()`는 필수 권한 종류,
플랫폼별 권한 이름, evidence artifact 규칙, opt-in override requirement를
검증한다.

## 권한 분류

| Permission | Requirement | 목적 |
|------------|-------------|------|
| `screen_capture` | `required` | benchmark가 visual confirmation을 요구할 때 대상 GUI 상태를 관측한다. |
| `accessibility` | `required` | local accessibility metadata로 matching과 focus validation을 수행한다. |
| `automation_input_control` | `required` | 설정된 mode를 통해 승인된 GUI input을 실행한다. |
| `notifications` | `recommended` | benchmark 또는 remediation 알림을 non-blocking으로 표시한다. |
| `local_service_reachability` | `required` | loopback의 local GUI ticket 또는 worker service에 접근한다. |
| `ocr_capability` | `recommended` | accessibility metadata가 부족할 때 OCR fallback을 제공한다. |

각 permission rule은 macOS, Windows, Linux 표시 이름과 safe remediation key를
포함한다. 이 정책은 remediation을 설명할 뿐이며 OS 권한을 자동으로 부여,
철회, 우회하지 않는다.

## Evidence artifact 규칙

| Artifact | Default audience | 기본 공유 | 규칙 |
|----------|------------------|-----------|------|
| `broad_screenshot` | `local_only` | No | necessity, retention, deletion을 문서화한 명시적 opt-in override가 없으면 benchmark runner가 거부한다. |
| `cropped_region` | `shareable_benchmark_artifact` | Yes | unrelated desktop content를 제외한 geometry-only crop. |
| `text_metadata` | `shareable_benchmark_artifact` | Yes | masked scene label만 허용한다. |
| `audit_excerpt` | `audit_only` | No | diagnostic code와 policy decision만 허용한다. |
| `log_excerpt` | `shareable_benchmark_artifact` | Yes | diagnostic code만 허용한다. |
| `worker_log` | `local_only` | No | 공유 전 명시적 opt-in override가 필요하며 raw UI payload를 포함하면 안 된다. |
| `gui_session_event` | `shareable_benchmark_artifact` | Yes | masked scene label과 stable ID만 허용한다. |
| `benchmark_report` | `shareable_benchmark_artifact` | Yes | masked scene label, stable ID, summary metric만 허용한다. |
| `raw_accessibility_label` | `local_only` | No | local matching 전용이다. override가 있어도 공유할 수 없다. |

## Opt-in override requirement

공유 불가 artifact를 override하려면 다음 항목을 모두 확인해야 한다.

| Requirement | 의미 |
|-------------|------|
| `necessity_justification` | 더 안전한 artifact로 benchmark 결과를 증명할 수 없는 이유. |
| `retention_policy` | artifact를 보관할 기간과 저장 위치. |
| `deletion_plan` | 검토 후 artifact를 삭제하는 방법. |

Broad screenshot, raw candidate label, credential, account setting, payment
flow, security prompt, unrelated desktop content는 기본 artifact에서 제외한다.
Raw label은 policy가 허용한 local matching에서만 사용할 수 있다.

## Benchmark runner gate

- 필수 권한이 거부되었거나 사용할 수 없으면 readiness contract가 이미
  unsupported로 분류한 경우를 제외하고 benchmark failure다.
- 권장 권한이 거부되면 benchmark를 degrade 또는 skip할 수 있지만 full
  parity로 보고하면 안 된다.
- Broad screenshot은 `opt_in_override`가 true이고 모든 override requirement가
  확인된 경우에만 허용한다.
- Shareable artifact는 raw window title, raw element text, typed user text,
  file path, screenshot, credential, security prompt를 포함하면 안 된다.
- Audit record, log, session event, report는 기본적으로 masked scene label,
  diagnostic code, stable ID, geometry summary만 사용한다.
