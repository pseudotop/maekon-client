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
- the manifest is older than the configured freshness window

Use:

```bash
python scripts/release_decision_manifest.py validate --manifest <manifest.json>
```
