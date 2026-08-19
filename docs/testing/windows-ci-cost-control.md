# Windows CI cost control

This policy keeps Windows validation useful without turning native CI into an
unbounded private-repository expense. It complements the functional tier
contract in [Windows interactive validation](./windows-interactive-validation.md).

## Runner ownership

| Lane | Trigger | Runner | Evidence owned | Cost boundary |
| --- | --- | --- | --- | --- |
| Parent contract gates | PR and ordinary CI | Existing Linux self-hosted runner | Workflow, catalog, validator, and deterministic contract results | No hosted Windows allocation |
| Parent Windows release lint | Reviewed release-candidate manual dispatch only | One GitHub-hosted `windows-latest` job | Native Windows compile/lint and deterministic Windows contract results | Global single concurrency; 45 runner-minute timeout |
| Parent E19 desktop smoke | Reviewed release smoke manual dispatch only | One GitHub-hosted `windows-latest` job | Exact-SHA `automation.gui.benchmark_report.v1` release evidence | Shared paid concurrency; 90 runner-minute timeout; no automatic trigger |
| Parent Windows audio fixture | One weekly schedule or reviewed manual dispatch | One GitHub-hosted `windows-latest` job | Synthetic audio/WebDriver build and bounded operator handoff artifact | Global single concurrency; 45 runner-minute timeout; upload only on manual dispatch |
| Parent per-OS patch audit | One weekly schedule or reviewed manual dispatch | Serialized `macos-latest` and `windows-latest` jobs | First-party platform-specific lint | Shared paid concurrency; 60 minutes per OS; 120 hosted runner-minutes per run |
| Public export CI | Reviewed export PR/release workflow | Public repository standard hosted Windows | Release-feature PE build, signed VC runtime staging, ZIP/MSI/NSIS closure | Does not consume the private parent workflow's Windows minutes |
| Consumer Windows lifecycle | No private executable lane | None | Explicitly unverified: consumer Windows 11, unlocked-desktop UIA, reboot, uninstall, and data-removal | A future dedicated-runner decision requires a new reviewed issue |

A Linux runner, Wine, cross-compilation, or a GitHub-hosted Windows Server build
agent cannot replace a consumer Windows 11 lifecycle receipt. The release
manifest must record that boundary rather than infer lifecycle evidence from
the hosted compile/lint result. Public repository hosted runner coverage is
unchanged by this private-parent policy.

## Private hosted native-runner cap

`.github/workflows/maekon-client-windows-release-lint.yml` and
`.github/workflows/maekon-client-desktop-smoke.yml` enforce these versioned
release-lane controls:

- no `push` or `pull_request` trigger;
- no automatic schedule;
- exactly one paid `windows-latest` job per workflow;
- one shared repository-wide paid concurrency group and cancellation disabled;
- a 45-minute release-lint timeout and a 90-minute E19 smoke timeout;
- release-lint dispatch is fixed to `release-candidate` and requires an exact
  40-character source SHA plus `confirm_paid_windows_minutes=true`;
- the release-lint Linux admission job validates input before GitHub
  allocates the paid Windows job;
- a rerun is never admitted; an operator must dispatch a new reviewed run.

The E19 lane accepts only a manually dispatched exact 40-character commit SHA,
requires `confirm_paid_windows_minutes=true`, and rejects reruns. It produces
the GUI benchmark receipt required by the release tag gate, but it does not
restore the retired consumer Windows lifecycle lane or claim clean-VM, reboot,
uninstall, or data-removal evidence.

`.github/workflows/maekon-client-patch-audit.yml` applies the same first-attempt,
exact-SHA, explicit-confirmation admission to its macOS/Windows matrix. The two
legs use `max-parallel: 1`, have a 60-minute timeout each, and share the same
repository-wide paid concurrency group as the two Windows workflows.

The release lanes have a zero-minute automatic ceiling. Each explicitly
confirmed lint dispatch can consume at most 45 Windows runner-minutes, and an
E19 release smoke can consume at most 90. This is not the repository-wide
total: the residual consumers below are accounted for separately. Dollar
conversion is intentionally not hard-coded because GitHub pricing and included
quotas can change.

Recent successful daily runs observed before this policy took roughly 15 to 24
minutes for release lint and 21 to 30 minutes for the audio fixture. The
45-minute timeout preserves cold-cache headroom while remaining a real billing
circuit breaker.

## Residual private Windows consumers

An intentionally separate consumer remains visible in the same contract test:

| Workflow | Trigger and maximum | Reason it remains separate |
| --- | --- | --- |
| `console-windows-contracts.yml` | Path-filtered pull requests only; one Windows job capped at 15 minutes | Exercises the console's natural Windows resolver and is outside the Maekon client release lane |

Therefore, scheduled Maekon client Windows work has a static worst-case cap of
105 minutes per week: 45 minutes for the audio fixture plus the 60-minute
Windows patch-audit leg. Including the separately billed macOS leg, the static
hosted-runner ceiling is 165 minutes per scheduled week. Console
usage is event-driven and path-filtered, so it cannot be expressed as a fixed
weekly total.

## Operator checklist

Before a manual paid run:

1. Prefer the public export CI when the required evidence is a release PE or
   package-closure result.
2. Use the parent paid lint or E19 release smoke only for an exact reviewed
   parent source SHA.
3. Never use **Re-run jobs** for a paid workflow. Reruns fail admission; create
   a fresh reviewed dispatch instead.
4. Confirm no equivalent run is active. The global concurrency policy queues
   behind the running paid workflow instead of canceling already billed work.
5. Select the narrow purpose and acknowledge the applicable 45-minute lint,
   90-minute E19 smoke, or 120-minute patch-audit hosted cap.
6. Check the repository or organization Actions budget and usage dashboard.
   Billing budgets and alerts are external GitHub settings and cannot be
   guaranteed by repository YAML.

As of 2026-08-04, the parent account has an account-wide Actions monthly budget
of USD 50 with **Stop usage** enabled, plus included-usage and budget alerts.
This is the final external circuit breaker, not a repository-specific promise:
all repositories under the account contribute to it and the setting must be
checked in GitHub Billing each month.

The retired private interactive workflow must not be recreated as a shortcut.
Any future consumer Windows lifecycle lane needs a new reviewed issue and must
restore the isolation, snapshot, and evidence-boundary requirements archived in
the interactive validation document.

## Review rule

Any change that adds a hosted Windows or macOS trigger, increases a timeout, adds a
matrix entry, weakens exact-SHA admission, or creates a second paid job must
update `scripts/ci-cost-control-contracts.test.mjs` and include a cost delta in
the PR evidence. A release urgency claim is not an exception to this rule.
