[English](./gui-benchmark-harness.md) | [한국어](./gui-benchmark-harness.ko.md)

# GUI Benchmark Harness 계약

이 문서는 Maekon GUI automation execution-plane port와 OS launcher에 사용하는
공통 benchmark harness 계약을 정의한다.

## 계약 버전

- Harness payload: `automation.gui.benchmark_harness.v1`
- Canonical machine-readable catalog:
  `docs/contracts/gui-benchmark-harness.v1.json`

## 도메인 타입

정식 Rust 계약은 다음 타입에 있다.

- `maekon_core::models::gui::GuiBenchmarkHarnessCatalog`
- `maekon_core::models::gui::GuiBenchmarkCase`
- `maekon_core::models::gui::GuiBenchmarkResult`
- `maekon_core::ports::gui_benchmark::GuiBenchmarkAdapter`

`GuiBenchmarkHarnessCatalog::validate_contract_coverage()`는 공유 case catalog를
검증한다. `GuiBenchmarkHarnessCatalog::validate_result()`는 evidence가 비어
있는데 pass로 요약한 결과와 observable state-change 없이 real-input 성공을
주장하는 결과를 거부한다.

## Shared case stage

| Stage | 필수 coverage |
|-------|---------------|
| `launcher_readiness` | OS launcher discovery, process cleanup policy, WebDriver readiness. |
| `focus` | Focus probe와 execution binding validation. |
| `scene_extraction` | masked 또는 cropped evidence만 사용하는 scene extraction. |
| `candidate_extraction` | latency, confidence, failure mode를 포함한 candidate ranking. |
| `overlay_lifecycle` | geometry-only evidence를 사용하는 overlay show/clear. |
| `input_action` | observable state-change verification을 동반한 승인된 input execution. |
| `verification` | raw sensitive UI payload 없이 before/after state proof. |
| `audit` | diagnostic code와 masked scene label을 사용하는 audit/session evidence. |

## Result schema

모든 case는 같은 result field를 사용한다.

| Field | 목적 |
|-------|------|
| `outcome` | `pass`, `fail`, `skip`, `degraded`, `blocked`, 또는 `unsupported`. |
| `latency_ms` | 측정 가능한 case latency. |
| `confidence` | 적용 가능한 adapter confidence. |
| `failure_mode` | non-pass outcome의 typed failure reason. |
| `evidence_path` | 안정적인 artifact reference. 빈 evidence는 pass가 될 수 없다. |
| `privacy_status` | `safe`, `redacted`, `local_only`, 또는 `rejected`. |
| `input_execution_mode` | readiness contract가 결정한 effective mode. |
| `verification_mode` | execution 검증 방식. |
| `launcher_platform` | `macos`, `windows`, `linux`, 또는 `unknown`. |

## Adapter 규칙

OS별 구현은 `GuiBenchmarkAdapter`를 노출하고 공유 `GuiBenchmarkCase` 값을 실행한다.
Harness는 adapter trait와 shared model에만 의존한다. Adapter끼리 서로 호출하거나
import하면 안 된다.

## Runner gate

- Case를 pass로 요약하려면 필수 evidence path와 artifact kind가 있어야 한다.
- `noop`, `dry_run_worker`, `unsupported`, `unknown` execution mode는 observable
  state change가 필요한 case를 pass할 수 없다.
- `input_action` pass result는 `observable_state_change` verification을 사용해야 한다.
- Launcher case는 Windows `maekon.exe` discovery와 WebDriver readiness를 포함해
  `launcher_platform`을 기록해야 한다.
- `pkill`, `SIGTERM`, `SIGKILL`, extensionless binary name 같은 Unix-only process
  가정은 cross-OS evidence로 인정하지 않는다.
- Shareable artifact는 GUI permission and evidence policy를 따라야 한다.
