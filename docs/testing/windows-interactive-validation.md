# Windows interactive validation topology

Status: admission and versioned tier executor are wired; execution remains
fail-closed when the reviewed runner, app artifact, VM snapshot identity, or
manual receipts are absent. Tracking: #9144, #9190, and #9837.

The source of truth is the parent repository's internal validation-tier
manifest, `windows-validation-tiers.v1.json` (maintained outside this
export). The PR,
nightly, and release-candidate tiers have one owner each for environment,
timeouts, retries, evidence, and result semantics. The validator rejects
configuration that could turn a locked/background session, unverified release
artifact, missing evidence, or manual prompt into an automated pass.

The runner-routing and private-repository billing boundary is documented in
[Windows CI cost control](./windows-ci-cost-control.md). Hosted Windows lint,
public artifact closure, and clean-VM interaction are deliberately separate
lanes; none may claim evidence owned by another lane.

## Tier boundary

| Tier | Primary scope | Environment | Timeout / retry | Evidence |
| --- | --- | --- | --- | --- |
| PR | Rust contracts, renderer CDP, minimal real-Tauri smoke | Ephemeral CI; no native Windows claim | 60 min; no retry | source SHA, structured result, sanitized logs |
| Nightly | Full real-Tauri plus adopted UIA single-instance, shortcut, native-dialog, tray, and notification-activation cases | Restored Windows 11 VM, isolated user/profile, unlocked serialized desktop | 180 min; one infrastructure-only retry | snapshot identity, admission receipt, JSONL, product receipts, UIA evidence |
| Release candidate | Install/update/rollback, Run-key reboot persistence, UAC persistence, DPI/locale/topology, lock/sleep, proxy/offline, protected manual gates | Clean VM reset and the exact signed artifact | 360 min; no retry | source/artifact SHA, artifact URI, Authenticode, lifecycle before/after, manual decisions |

Production Windows notification activation uses the WinRT `Activated` callback,
an explicit internal-route allowlist, and a durable sanitized audit receipt.
Tray identity uses a unique accessible Name/AutomationId/ControlType selector. The tested
Windows 11 XAML provider does not dispatch coordinate-free UIA activation to
the application, so hide/restore stays blocked and is never replaced by
coordinate input or debug simulation.

## VM baseline and reset

1. Create a Windows 11 VM dedicated to validation. Do not reuse a developer or
   personal VM.
2. Install Windows updates, WebView2, MSVC/Rust, Node/pnpm, .NET 8, Git Bash,
   and the GitHub runner. Register it only with
   `self-hosted`, `windows`, `x64`, `maekon-interactive`, and
   `unlocked-desktop`.
3. Create a local non-administrator test account with no personal Microsoft,
   provider, browser, or credential-store enrollment. Set
   `MAEKON_WINDOWS_INTERACTIVE_RUNNER=1` only on this reviewed runner.
4. Shut down cleanly and capture a named baseline snapshot. Record the VM
   image/version and snapshot identifier in run evidence.
5. Before every nightly or release-candidate run, stop the runner service,
   revert to the baseline, boot, sign in to the isolated account, verify the
   interactive desktop, confirm `confirm_clean_snapshot_restored`, and start
   exactly one runner.
6. After evidence upload, stop the runner, discard the run profile, and revert
   to the baseline. Never promote a mutated run state into the next baseline.

The admission probe rejects session zero, background execution, a missing
foreground desktop, lock surfaces, missing runner markers, and insufficient
disk. Workflow concurrency uses the global
`maekon-windows-interactive-desktop` group with cancellation disabled, so a new
dispatch cannot overlap or terminate an active desktop run.

## Capacity and cleanup

- Nightly admission requires at least 40 GiB free; release-candidate admission
  requires at least 80 GiB.
- At or below 5 GiB free, stop before building and perform one cleanup pass,
  matching the host policy requested for this project.
- Cleanup is limited to runner-temp Maekon artifacts, disposable isolated
  profiles, and completed unreferenced build targets.
- Never delete a user profile, credential store, source worktree, or failure
  evidence that has not been uploaded.
- Re-run admission after cleanup. Insufficient space remains `blocked`, never
  `pass` or `skip`.

## Artifact and evidence policy

Release-candidate execution requires the exact artifact URI, source SHA,
SHA-256, and a valid Authenticode signature before install. Rebuilt or
similarly named binaries are not substitutes. Admission records all four
identity fields, and execution rejects an artifact whose admitted source SHA
does not match the checked-out executor SHA.

Evidence may include bounded app/dialog captures, sanitized logs, structured
state receipts, UIA selectors, hashes, and pass/fail/blocked decisions. It must
exclude tokens, cookies, credentials, raw provider payloads, unrelated desktop
content, broad screenshots, and raw sensitive user input. Nightly retention is
14 days; release-candidate retention is 30 days.

UAC, credential entry, security prompts, and privacy prompts are manual gates.
Automation stops before the protected action. A human records only the
decision/result and non-sensitive postcondition; no credential or protected
prompt content is captured.

## Executor boundary

`.github/workflows/maekon-client-windows-interactive.yml` remains
dispatch-only. After admission, `Invoke-WindowsInteractiveTier.ps1` resolves
the versioned manifest plan and executes entries serially. Nightly owns full
WDIO plus the single-instance, global-shortcut, native-dialog, tray, and
notification-activation UIA
scenarios. Every entry writes a per-TC JSONL row, bounded driver receipts, the
exact source SHA, sanitized relative evidence references, executable
name/SHA-256/size identity when supplied, admission and snapshot identity, and
a target-scoped cleanup receipt.

TRAY-007 is executable and records `blocked` if the Shell does not expose
exactly one named Maekon notification-area button. NOTIF-007 is executable and
requires the native UIA tree, semantic input, stable PID/HWND, routed product
receipt, and activation audit receipt together. Because `fail` outranks
`blocked`, and `blocked` outranks `pass`, the tier cannot report pass while a
declared product blocker remains.
Release-candidate lifecycle and protected surfaces use reviewed manual receipt
keys; missing or invalid receipts are blocked or failed, never inferred as
pass. A passing receipt is bound to its TC, receipt key, exact source and
artifact SHA, VM snapshot identity, reviewer, timestamp, and existing relative
evidence files. Protected receipts must enumerate every declared surface; an
aggregate `pass` cannot silently omit UAC, credential, security, or privacy
coverage. The executor never accepts those prompts.

The reviewed receipt is a versioned JSON document. Evidence paths are relative
to the receipt directory and must resolve to existing files beneath it:

```json
{
  "schema_version": "maekon.windows-manual-decisions.v1",
  "decisions": [
    {
      "tc_id": "CRT-PRV-PERM-006",
      "receipt_key": "windows-uac-persistence",
      "result": "pass",
      "reviewed_by": "operator-id",
      "recorded_at_utc": "2026-08-03T00:00:00Z",
      "source_sha": "40-character-git-sha",
      "artifact_sha256": "64-character-artifact-sha256",
      "vm_snapshot_identity": "clean-windows-11-baseline-id",
      "surfaces": ["uac"],
      "evidence_paths": ["receipts/PERM-006-postconditions.json"]
    }
  ]
}
```

An operator must still provide all external prerequisites:

1. the isolated snapshot and exact reviewed runner labels;
2. the runner marker and non-sensitive snapshot identity;
3. a passing admission receipt;
4. an exact nightly application path or a signed release-candidate artifact
   with immutable URI, source SHA, and artifact SHA-256;
5. reviewed manual receipts for lifecycle or protected actions.
