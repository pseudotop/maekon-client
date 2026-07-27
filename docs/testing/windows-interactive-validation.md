# Windows interactive validation topology

Status: executable admission contract; interactive tier execution is not yet
enabled. Tracking: #9144 and #9190.

The source of truth is the parent repository's internal validation-tier
manifest, `windows-validation-tiers.v1.json` (maintained outside this
export). The PR,
nightly, and release-candidate tiers have one owner each for environment,
timeouts, retries, evidence, and result semantics. The validator rejects
configuration that could turn a locked/background session, unverified release
artifact, missing evidence, or manual prompt into an automated pass.

## Tier boundary

| Tier | Primary scope | Environment | Timeout / retry | Evidence |
| --- | --- | --- | --- | --- |
| PR | Rust contracts, renderer CDP, minimal real-Tauri smoke | Ephemeral CI; no native Windows claim | 60 min; no retry | source SHA, structured result, sanitized logs |
| Nightly | Full real-Tauri plus adopted UIA single-instance, shortcut, and native-dialog cases | Restored Windows 11 VM, isolated user/profile, unlocked serialized desktop | 180 min; one infrastructure-only retry | snapshot identity, admission receipt, JSONL, product receipts, UIA evidence |
| Release candidate | Install/update/rollback, DPI/locale/topology, lock/sleep, proxy/offline, protected manual gates | Clean VM reset and the exact signed artifact | 360 min; no retry | source/artifact SHA, Authenticode, lifecycle before/after, manual decisions |

Tray activation and production notification activation remain blocked by #9181
and #9182. They are not silently replaced by coordinate input or debug
simulation.

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
   interactive desktop, and start exactly one runner.
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
similarly named binaries are not substitutes.

Evidence may include bounded app/dialog captures, sanitized logs, structured
state receipts, UIA selectors, hashes, and pass/fail/blocked decisions. It must
exclude tokens, cookies, credentials, raw provider payloads, unrelated desktop
content, broad screenshots, and raw sensitive user input. Nightly retention is
14 days; release-candidate retention is 30 days.

UAC, credential entry, security prompts, and privacy prompts are manual gates.
Automation stops before the protected action. A human records only the
decision/result and non-sensitive postcondition; no credential or protected
prompt content is captured.

## Enablement gate

`.github/workflows/maekon-client-windows-interactive.yml` is dispatch-only and
currently admission-only. It validates the topology, requires explicit
operator confirmation, targets only the five reviewed labels, and then fails
intentionally after writing an admission receipt. Therefore a green workflow
cannot be mistaken for completed native UI or release lifecycle coverage.

Enable actual nightly/release-candidate commands only in a separate reviewed
change after:

1. the isolated snapshot and runner labels exist;
2. the runner marker is configured;
3. a dry-run admission receipt is reviewed;
4. tier commands write the manifest's complete evidence contract; and
5. failure/locked/background drills prove that the workflow remains fail-closed.
