[English](./standalone-adoption-runbook.md) | [한국어](./standalone-adoption-runbook.ko.md)

# Standalone Adoption Runbook

A practical rollout checklist for operating ONESHIM in standalone-first mode.

## Day 0 (setup)

1. Run `cargo run -p oneshim-app -- --offline`.
2. Open dashboard at `http://localhost:10090`.
3. In Settings:
- keep sandbox enabled (`Standard` or `Strict`),
- set `external_data_policy` to `PiiFilterStandard` or stricter,
- keep `allow_unredacted_external_ocr=false`.

## Day 1-3 (baseline)

1. Enable only essential templates (`daily-priority-sync`, `deep-work-start`).
2. Capture baseline KPI:
- `success_rate`,
- `blocked_rate`,
- `p95_elapsed_ms`,
- `timing_samples`.

## Day 4-7 (controlled expansion)

1. Add one more template (`bug-triage-loop` or `release-readiness`).
2. If blocked actions increase:
- review policy card in Automation,
- use `scene_action_override` only with reason + approver + expiration.

## Weekly review

1. Export metrics/events from Settings.
2. Review Automation audit log for policy-denied patterns.
3. Keep only templates that improve speed without raising blocked-rate trend.

## Exit criteria for wider rollout

- Stable `success_rate` trend over one week.
- No long-running sensitive override without expiry.
- Replay scene overlay and action execution are validated by E2E in CI.
