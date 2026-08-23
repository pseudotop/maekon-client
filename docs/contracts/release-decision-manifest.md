[English](./release-decision-manifest.md)

# Release Decision Manifest Contract

This contract defines the E19 desktop smoke release-decision manifest. It is a
derived record over the E14 GUI benchmark report contract, not a new evidence
family.

## Contract version

- Manifest payload: `automation.gui.benchmark_report.v1.release_decision`
- Source benchmark report: `automation.gui.benchmark_report.v1`
- Evidence policy: `automation.gui.permission_evidence.v1`
- Canonical machine-readable example:
  `docs/contracts/release-decision-manifest.v1.json`
- Checklist disposition registry:
  `docs/contracts/release-checklist-dispositions.v2.json`

## Required release metadata

Every manifest records:

- `commit_sha`
- `release_tag`
- `artifact_checksum`
- `generated_at` as UTC `Z`
- `workflow_run_url` or `manual_evidence_id`
- runner OS and labels
- evidence artifact ids and hashes
- top-level `privacy_status` and `redaction_status`
- cleanup result
- covered issue numbers
- the complete canonical checklist result set, bound to the checklist and
  disposition-registry SHA-256 digests
- `release_decision.state`

Supported decision states are `pass`, `optional`, `soft_block`, `hard_block`,
and `blocked_for_privacy`.

## History-First mapping

Every release-critical claim must map to evidence with `sha`, `date`, `path`,
and `summary`. When historical drift is relevant, the claim must include all
three stages:

- `initial`
- `pivot`
- `current`

If history is not relevant, the claim must include `current` evidence and a
`history_not_relevant_reason`.

Unsupported or missing history cannot silently pass. The validator fails closed
and reports the missing stage.

## Complete checklist results

Every top-level checkbox in `docs/release-checklist.md` has a stable
`release-check-id` comment. The disposition registry must contain the same IDs
exactly once and in canonical order. Each entry is classified as:

- `machine`: a command or canonical lane decides the item
- `evidence`: a collector produces a receipt for human reading and sign-off
- `human`: non-mechanical judgment, with the reason recorded in the registry

Manifest build requires `--checklist-results`. The results payload uses
`maekon.release_checklist_results.v2`, contains every stable ID in canonical
order, and records `state` plus a non-empty `receipt`. A passing `human` result
also records the reviewer.

The validator compares the embedded source and registry hashes to the checked
out canonical files. Missing, duplicate, unknown, or reordered IDs fail closed.
An unavailable subject is recorded as such in the registry and blocks release;
it cannot be converted into a human checkbox or a passing result. Registry v2
also separates `pre_publish` from `post_publish`; its explicit `default_phase`
applies to entries without an override. Only a `post_publish` item may
be `pending` in an accepted pre-tag manifest, and it remains visible in the
embedded checklist summary. `RC-MANUAL-006` is closed later by the independent
[`post-publish-updater-receipt`](./post-publish-updater-receipt.md) contract; a
mock updater test does not replace that observation.

## Fail-closed evidence gates

The validator rejects release acceptance when any of these are true:

- benchmark results are empty
- the source benchmark report timestamp is stale
- a pass result has stale evidence
- a pass result has no evidence path or artifact kind
- dispatch-only evidence is treated as execution proof
- evidence artifact ids or hashes are missing
- console or artifact evidence is not sanitized
- privacy/redaction status is not shareable
- cleanup did not pass
- checklist coverage is incomplete or ambiguous
- a pre-publish checklist result is not passing
- a checklist result is blocked or its registered subject is unavailable
- a pending checklist result is not registered as `post_publish`
- the manifest is older than the configured freshness window

Use:

```bash
python scripts/release_decision_manifest.py validate --manifest <manifest.json>
```

Build with checklist results:

```bash
python scripts/release_decision_manifest.py build \
  --checklist-results <checklist-results.json> \
  <other-required-release-arguments>
```
