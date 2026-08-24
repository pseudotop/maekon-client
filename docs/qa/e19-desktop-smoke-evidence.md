# E19 Desktop Smoke Evidence Runbook

This runbook defines the privacy-safe bridge for real desktop E19 smoke runs.
It reuses the E14 GUI benchmark report and permission evidence contracts:

- `automation.gui.benchmark_report.v1`
- `automation.gui.permission_evidence.v1`
- `automation.gui.benchmark_report.v1.release_decision`

No default PR workflow depends on interactive desktop state. E19 desktop smoke
is explicit, workflow_dispatch-only, and release-auditable until a later issue
promotes a subset to blocking CI.

For rc release operability, the release-decision contract also accepts the
explicit `exact_sha_ci_substitute` mode. It requires successful automatic
checks, all four `Release Smoke` build rows, and `Integrity Gates` on one exact
public commit SHA. This mode is not interactive macOS permission evidence: TCC
grant/revoke/suppression/recovery and consent-byte invariance remain
`deferred_unproven`, and the post-publish updater observation remains a separate
gate.

## Decision States

Release managers must record one of these states in the release-decision
manifest:

| State | Meaning |
| --- | --- |
| `optional` | Evidence is useful but not release blocking. |
| `soft_block` | Evidence is incomplete or degraded and needs release-owner review. |
| `hard_block` | Evidence is stale, missing, mismatched, or semantically failing. |
| `blocked_for_privacy` | Evidence cannot be sanitized safely; do not upload raw artifacts. |

## Sanitized Bundle Shape

Shareable artifacts must be produced through:

```bash
python scripts/sanitize_desktop_evidence.py \
  --input <raw-log-or-metadata-file> \
  --artifact-kind log_excerpt \
  --output-dir <sanitized-bundle-dir> \
  --commit-sha <40-char-sha> \
  --release-tag <vX.Y.Z> \
  --artifact-checksum sha256:<64-char-hex> \
  --runner-label <runner-label> \
  --cleanup-status pass
```

The sanitizer writes marker/count JSON only. It never uploads raw screenshots,
raw accessibility trees, raw stdout/stderr, local DBs, full consent records,
raw runtime logs, or provider account data. If a raw-only artifact is passed to
the sanitizer, it writes `blocked-report.json` and sets `blocked_for_privacy`.

Synthetic privacy canaries cover email, phone, credit card, SSN, IBAN, API key,
OAuth token, user path, password text, provider account/org, URL query token,
`1Password`, `Bitwarden`, `Bank`, and `Authenticator`. Sensitive-app cases must
report `sensitive_app_excluded`.

Every shareable artifact records:

- `privacy_status`
- `artifact_kind`
- `retention_days`
- `redaction_status`
- commit SHA
- release tag
- artifact checksum
- runner label
- cleanup status

## CI Bridge

The bridge is intentionally narrow:

- `release-smoke.yml` remains `workflow_dispatch` only.
- `macos-windowserver-gui-smoke.yml` remains `workflow_dispatch` only.
- GitHub permissions stay read-only: `contents: read`.
- Jobs use the `desktop-smoke` environment.
- Desktop smoke jobs must not reference release signing, macOS certificate,
  macOS notarization, update signing, or release App private-key secrets.
- `upload-artifact` paths must point only to sanitized bundles under
  `${{ runner.temp }}` with `retention-days: 7`.
- Actions logs are treated as shareable evidence, so console output must avoid
  raw UI text, account identifiers, provider diagnostics, and local paths.

Promotion criteria for any blocking subset:

1. Runner inventory is stable for the target OS.
2. Sanitized bundle upload succeeds for at least one release rehearsal.
3. The release-decision manifest maps each release-critical claim through
   History-First evidence.
4. Privacy canaries pass.
5. Manual fallback is documented for every platform row that remains optional.

## Runner Matrix

| OS | Runner label | Session requirements | Permissions | Artifact classes | Evidence kinds | Fallback |
| --- | --- | --- | --- | --- | --- | --- |
| Windows | `windows-desktop-smoke` | Interactive Windows session, visible desktop, foreground input allowed | Graphics capture consent, UI Automation access, foreground input control, toast notifications | MSI, zip, debug binary | sanitized log excerpt, GUI session event, benchmark report, release-decision manifest | `manual_required` with interactive desktop automation or human operator evidence |
| macOS | `macos-windowserver` | Self-hosted macOS runner with WindowServer and unlocked user session | Screen Recording, Accessibility, Input Monitoring or Automation, Notifications | DMG, PKG, universal tarball | sanitized log excerpt, GUI session event, benchmark report, release-decision manifest | `manual_required` when no approved WindowServer runner exists |
| Linux | `linux-desktop-smoke` | Desktop portal or compositor session with stable display server | Portal/compositor screen capture, AT-SPI, desktop portal input control, desktop notifications | DEB, tarball | sanitized log excerpt, GUI session event, benchmark report, release-decision manifest | `blocked` or `manual_required`; never false pass |

Missing runner inventory produces `blocked` or `manual_required`, not pass.
Missing desktop permissions are `blocked` when release-critical and
`soft_block` when the row is optional or exploratory.

Selecting the exact-SHA CI substitute changes the release decision scope rather
than rewriting these runtime states: it permits an operability decision while
preserving the missing interactive claims as deferred and unproven.

Do not reopen E14/E17 platform parity work from this matrix unless a new
release-critical gap is found. Platform expansion stays tied to shipped release
artifact classes.

## Tool Choice

Use scriptable WDIO, Playwright, and Cargo tests when they can prove webview,
IPC, contract, or build behavior without real desktop state. Use interactive
desktop automation only for foreground native desktop behavior that requires a
visible app, overlay, OS permission surface, tray/menu interaction, or manual
provider flow.
