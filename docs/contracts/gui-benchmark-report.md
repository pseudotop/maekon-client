[English](./gui-benchmark-report.md) | [한국어](./gui-benchmark-report.ko.md)

# GUI Benchmark Report Contract

This document defines the stable report format and regression threshold policy
for Maekon native GUI automation benchmarks.

## Contract version

- Report payload: `automation.gui.benchmark_report.v1`
- Canonical machine-readable report:
  `docs/contracts/gui-benchmark-report.v1.json`

## Domain types

The canonical Rust contract lives in:

- `maekon_core::models::gui::GuiBenchmarkReport`
- `maekon_core::models::gui::GuiBenchmarkReportedResult`
- `maekon_core::models::gui::GuiBenchmarkPlatformSummary`
- `maekon_core::models::gui::GuiBenchmarkThresholdRule`

`GuiBenchmarkReport::validate_report()` verifies report shape against the
shared benchmark harness catalog and rejects empty, stale, or dispatch-only pass
summaries.

## Report locations

| Location | Purpose |
|----------|---------|
| `local_json` | Local machine-readable artifact for manual benchmark runs. |
| `ci_artifact` | CI-retained artifact when an environment can run the benchmark. |
| `project_issue_summary` | Compact Project issue summary with outcome, caveats, and links. |
| `manual_review_bundle` | Privacy-reviewed package for operator inspection. |
| `criterion_summary` | Non-interactive Criterion output linked to the same metric names. |

Criterion microbenchmarks remain usable without live OS permissions. They share
metric names and threshold policy, but they do not claim OS-interactive parity.

## Required result fields

Reports preserve the harness result fields and add:

| Field | Purpose |
|-------|---------|
| `evidence_fresh` | Whether referenced evidence is current for this run. |
| `sidecar_present` | Whether the GUI sidecar/worker was present. |
| `hmac_secret_present` | Whether ticket/auth material required for execution was present. |

Platform summaries include capability snapshots, execution mode, verification
mode, launcher platform, privacy statuses, and caveats so Windows, macOS, and
Linux can be compared without hiding platform-specific gaps.

## Threshold policy

| Severity | Meaning |
|----------|---------|
| `advisory` | Performance regression or quality warning; useful for review but not a semantic blocker. |
| `blocking` | Semantic gate failure such as missing real input, missing safe evidence, or insufficient pass rate. |

Threshold metrics use stable integer units such as milliseconds or basis points.
This keeps report diffs deterministic and avoids locale-dependent formatting.

## Pass gates

- A report must not mark an empty result stream as pass.
- Pass results must include evidence paths and artifact kinds.
- Stale evidence cannot pass.
- `noop`, `dry_run_worker`, and dispatch-only `command_accepted` evidence cannot
  prove execution pass.
- Shareable artifacts must follow the GUI permission and evidence policy.
- Non-pass outcomes must keep typed failure modes and platform caveats.
