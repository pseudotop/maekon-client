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
- an `operability_assurance` record that chooses either isolated interactive
  runtime proof or the exact-SHA CI substitute described below
- the complete canonical checklist result set, bound to the checklist and
  disposition-registry SHA-256 digests
- `release_decision.state`

Supported decision states are `pass`, `optional`, `soft_block`, `hard_block`,
and `blocked_for_privacy`.

## Operability assurance modes

`operability_assurance.mode` is required and is deliberately narrower than a
general evidence override:

- `interactive_runtime` records a passing isolated-runtime receipt.
- `exact_sha_ci_substitute` authorizes release-operability decisions from CI
  when the automatic checks, `Release Smoke`, and `Integrity Gates` receipts
  all report `success` for the manifest `commit_sha`. `Release Smoke` must
  contain the unique `linux`, `macos-arm64`, `macos-x64`, and `windows` rows.

The CI substitute has the fixed scope `release_operability_only`. It does not
prove macOS TCC grant/revoke behavior or consent-record byte invariance. Those
two claims must remain present as `deferred_unproven`; a release-critical claim
with either reserved claim id cannot be marked `pass` in this mode. The
post-publish updater receipt is also independent and remains governed by
`RC-MANUAL-006`.

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

For a release decision, do not hand-assemble that payload. Use the canonical
collector:

```bash
python scripts/release_decision_manifest.py collect-checklist-results \
  --receipt-index <machine-and-evidence-receipts.json> \
  --human-results <actual-human-decisions.json> \
  --commit-sha <40-character-public-sha> \
  --release-tag <vX.Y.Z> \
  --output <checklist-results.json>
```

The receipt index uses `maekon.release_checklist_receipt_index.v1` and contains
exactly the 65 `machine` and `evidence` entries. Human decisions use the
separate `maekon.release_checklist_human_results.v1` payload and contain exactly
the four `human` entries. Both payloads bind the same public commit and release
tag. Each entry repeats its registered disposition and subject reference and
provides receipt metadata with `uri`, `sha256`, `observed_at`, `commit_sha`, and
`release_tag`; human entries also name the actual reviewer.

The split is a trust boundary. CI can gather machine and evidence receipts, but
it cannot synthesize, default, or reuse the four maintainer decisions. The
collector rejects missing or duplicate IDs, placeholder URIs or reviewers,
subject/disposition drift, future timestamps, and receipt commit/tag mismatch.
It emits all 69 results once in canonical order under
`maekon.release_checklist_collector.v1`. A pre-publish item must pass; only the
registered post-publish item may remain pending.

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

### Freshness clocks

The one-hour (`3600` second) acceptance window is an authorization lifetime, not
the runtime of the desktop smoke. `generated_at` is stamped when the manifest is
built, after the report, commit, artifact checksum, checklist results, and
privacy evidence have been bound into one decision. The full hour is therefore
available for validation, signing, and tag publication instead of being consumed
by the approximately 36-minute hosted smoke that precedes the decision.

The window remains finite because an otherwise replayable decision can decay
when a runner image changes, a dependency is yanked, or a new security advisory
arrives. Commit and artifact identity prevent evidence from being applied to a
different build; the one-hour decision lifetime prevents an accepted manifest
from becoming an indefinite authorization for that build. The embedded benchmark
timestamp and per-result `evidence_fresh` flag remain independently fail-closed,
so stamping a new manifest cannot turn stale source evidence into fresh evidence.

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
- operability assurance is missing, references a different commit, or contains
  a failed/missing automatic-check, four-OS Release Smoke, or Integrity Gates
  receipt
- the CI substitute omits either deferred macOS TCC claim or attempts to mark
  one as passed
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
  --operability-assurance <operability-assurance.json> \
  --checklist-results <checklist-results.json> \
  <other-required-release-arguments>
```
