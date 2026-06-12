[English](./gui-benchmark-report.md) | [한국어](./gui-benchmark-report.ko.md)

# GUI Benchmark Report 계약

이 문서는 Maekon native GUI automation benchmark에 사용하는 안정적인 report
format과 regression threshold policy를 정의한다.

## 계약 버전

- Report payload: `automation.gui.benchmark_report.v1`
- Canonical machine-readable report:
  `docs/contracts/gui-benchmark-report.v1.json`

## 도메인 타입

정식 Rust 계약은 다음 타입에 있다.

- `maekon_core::models::gui::GuiBenchmarkReport`
- `maekon_core::models::gui::GuiBenchmarkReportedResult`
- `maekon_core::models::gui::GuiBenchmarkPlatformSummary`
- `maekon_core::models::gui::GuiBenchmarkThresholdRule`

`GuiBenchmarkReport::validate_report()`는 shared benchmark harness catalog 기준으로
report shape를 검증하고 empty, stale, dispatch-only pass summary를 거부한다.

## Report location

| Location | 목적 |
|----------|------|
| `local_json` | 수동 benchmark run을 위한 local machine-readable artifact. |
| `ci_artifact` | benchmark 실행이 가능한 환경에서 보관하는 CI artifact. |
| `project_issue_summary` | outcome, caveat, link를 포함한 compact Project issue summary. |
| `manual_review_bundle` | operator 검토를 위한 privacy-reviewed package. |
| `criterion_summary` | 같은 metric name에 연결되는 non-interactive Criterion output. |

Criterion microbenchmark는 live OS permission 없이도 계속 사용할 수 있다. Metric
name과 threshold policy는 공유하지만 OS-interactive parity를 주장하지 않는다.

## 필수 result field

Report는 harness result field를 보존하고 다음 항목을 추가한다.

| Field | 목적 |
|-------|------|
| `evidence_fresh` | 참조된 evidence가 이번 run에 대해 최신인지 여부. |
| `sidecar_present` | GUI sidecar/worker 존재 여부. |
| `hmac_secret_present` | execution에 필요한 ticket/auth material 존재 여부. |

Platform summary는 capability snapshot, execution mode, verification mode,
launcher platform, privacy status, caveat를 포함해 Windows, macOS, Linux를
platform-specific gap을 숨기지 않고 비교할 수 있게 한다.

## Threshold policy

| Severity | 의미 |
|----------|------|
| `advisory` | review에는 유용하지만 semantic blocker는 아닌 performance regression 또는 quality warning. |
| `blocking` | missing real input, missing safe evidence, insufficient pass rate 같은 semantic gate failure. |

Threshold metric은 millisecond 또는 basis point 같은 안정적인 정수 단위를 사용한다.
이 방식은 report diff를 deterministic하게 유지하고 locale-dependent formatting을 피한다.

## Pass gate

- Empty result stream을 pass로 표시하면 안 된다.
- Pass result는 evidence path와 artifact kind를 포함해야 한다.
- Stale evidence는 pass할 수 없다.
- `noop`, `dry_run_worker`, dispatch-only `command_accepted` evidence는 execution
  pass를 증명할 수 없다.
- Shareable artifact는 GUI permission and evidence policy를 따라야 한다.
- Non-pass outcome은 typed failure mode와 platform caveat를 유지해야 한다.
