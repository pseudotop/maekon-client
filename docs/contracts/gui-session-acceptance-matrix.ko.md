[English](./gui-session-acceptance-matrix.md) | [한국어](./gui-session-acceptance-matrix.ko.md)

# GUI 세션 Acceptance Matrix

이 문서는 Maekon automation benchmark에 사용하는 cross-OS GUI session
acceptance matrix를 정의한다.

## 계약 버전

- Matrix payload: `automation.gui.acceptance_matrix.v1`
- Canonical machine-readable matrix:
  `docs/contracts/gui-session-acceptance-matrix.v1.json`

## 도메인 타입

정식 Rust 계약은 다음 타입에 있다.

- `maekon_core::models::gui::GuiSessionAcceptanceMatrix`
- `maekon_core::models::gui::GuiSessionAcceptanceCase`

`GuiSessionAcceptanceMatrix::validate_contract_coverage()`는 matrix가 필수
lifecycle, geometry, execution-mode, negative-path case를 포함하는지 검증한다.

## Lifecycle coverage

Matrix는 ADR-002 primary session flow를 포함한다.

| Stage | 필수 coverage |
|-------|---------------|
| `propose` | 현재 scene proposal과 privacy-safe candidate. |
| `highlight` | display scaling, multi-monitor, negative origin, primary-monitor fallback overlay geometry. |
| `confirm` | ticket 발급 전 focus revalidation. |
| `execute` | `noop`, `dry_run_worker`, `sandboxed_real_input`, `direct_real_input`, `unsupported` mode. |
| `verify` | safe before/after state evidence 또는 명시적 degraded/unsupported status. |
| `audit` | session, policy, diagnostic evidence. |
| `timeout` | session expiry와 ticket issuance denial. |
| `cancel` | terminal cancellation과 후속 call rejection. |

## Benchmark 규칙

- Default benchmark run은 low-risk session-based case만 포함한다.
- High-risk direct input case는 operator가 명시적으로 opt in하지 않으면 `excluded`다.
- Legacy direct execution은 primary session success metric과 분리한다.
- Real-input execution case는 `before_after_state` evidence를 포함해야 한다.
- Unsupported/degraded environment는 parity를 가장하지 않고 해당 상태를 기록해야 한다.

## Negative case

Matrix는 stale scene, stale bounding box, coordinate drift, focus-window
mismatch, ticket/capability failure case를 포함한다. 이 case들은 real input
전송 전에 실패해야 한다.

## Privacy 규칙

Evidence는 display-safe여야 한다. 안정적인 case ID, session ID, policy
decision, geometry summary, diagnostic code는 사용할 수 있다. raw window
title, element text, screenshot, file path, user-entered text는 포함하면 안 된다.
