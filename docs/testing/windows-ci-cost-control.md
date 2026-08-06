# Windows CI cost control

This policy keeps Windows validation useful without turning native CI into an
unbounded private-repository expense. It complements the functional tier
contract in [Windows interactive validation](./windows-interactive-validation.md).

## Runner ownership

| Lane | Trigger | Runner | Evidence owned | Cost boundary |
| --- | --- | --- | --- | --- |
| Parent contract gates | PR and ordinary CI | Existing Linux self-hosted runner | Workflow, catalog, validator, and deterministic contract results | No hosted Windows allocation |
| Parent Windows release lint | One weekly schedule or reviewed manual dispatch | One GitHub-hosted `windows-latest` job | Native Windows compile/lint and deterministic Windows contract results | Global single concurrency; 45 runner-minute timeout |
| Parent Windows audio fixture | One weekly schedule or reviewed manual dispatch | One GitHub-hosted `windows-latest` job | Synthetic audio/WebDriver build and bounded operator handoff artifact | Global single concurrency; 45 runner-minute timeout; upload only on manual dispatch |
| Parent per-OS patch audit | One weekly schedule or reviewed manual dispatch | Serialized `macos-latest` and `windows-latest` jobs | First-party platform-specific lint | Shared paid concurrency; 60 minutes per OS; 120 hosted runner-minutes per run |
| Public export CI | Reviewed export PR/release workflow | Public repository standard hosted Windows | Release-feature PE build, signed VC runtime staging, ZIP/MSI/NSIS closure | Does not consume the private parent workflow's Windows minutes |
| Clean-host lifecycle | Dispatch only | Isolated self-hosted Windows 11 VM with unlocked desktop | Install, first launch, sidecar, UIA/toast, reboot, uninstall, and data-removal receipts | No GitHub-hosted runner-minute charge; infrastructure is operator-owned |

A Linux runner, Wine, cross-compilation, or a Windows Server build agent cannot
replace the clean Windows 11 lifecycle receipt. Conversely, the clean VM must
not rebuild the candidate and substitute a locally produced artifact for the
reviewed signed artifact.

## Private hosted native-runner cap

`.github/workflows/maekon-client-windows-release-lint.yml` and
`.github/workflows/maekon-client-windows-audio-fixture.yml` enforce these
versioned controls:

- no `push` or `pull_request` trigger;
- one weekly automatic run, not a daily run;
- exactly one paid `windows-latest` job;
- one shared global concurrency group across both workflows and every ref;
- a 45-minute paid-job timeout;
- manual dispatch requires an exact 40-character source SHA, a declared
  purpose, and `confirm_paid_windows_minutes=true`;
- the existing Linux self-hosted runner validates admission before GitHub
  allocates the paid Windows job;
- a rerun is never admitted; an operator must dispatch a new reviewed run.

`.github/workflows/maekon-client-patch-audit.yml` applies the same first-attempt,
exact-SHA, explicit-confirmation admission to its macOS/Windows matrix. The two
legs use `max-parallel: 1`, have a 60-minute timeout each, and share the same
repository-wide paid concurrency group as the two Windows workflows.

The two workflows governed by this section have a combined automatic worst-case
ceiling of 90 Windows runner-minutes per week, or 450 minutes in a
five-occurrence month. Each explicitly confirmed manual dispatch can add at
most 45 minutes. This is not the repository-wide total: the residual consumers
below are accounted for separately. Dollar conversion is intentionally not
hard-coded because GitHub pricing and included quotas can change.

Recent successful daily runs observed before this policy took roughly 15 to 24
minutes for release lint and 21 to 30 minutes for the audio fixture. The
45-minute timeout preserves cold-cache headroom while remaining a real billing
circuit breaker.

## Residual private Windows consumers

Two intentionally separate consumers remain visible in the same contract test:

| Workflow | Trigger and maximum | Reason it remains separate |
| --- | --- | --- |
| `console-windows-contracts.yml` | Path-filtered pull requests only; one Windows job capped at 15 minutes | Exercises the console's natural Windows resolver and is outside the Maekon client release lane |

Therefore, scheduled Maekon client Windows work has a static worst-case cap of
150 minutes per week: 90 minutes for the two Windows workflows plus the
60-minute Windows patch-audit leg. Including the separately billed macOS leg,
the static hosted-runner ceiling is 210 minutes per scheduled week. Console
usage is event-driven and path-filtered, so it cannot be expressed as a fixed
weekly total.

## Operator checklist

Before a manual paid run:

1. Prefer the public export CI when the required evidence is a release PE or
   package-closure result.
2. Use the parent paid lint only for an exact reviewed parent source SHA.
3. Never use **Re-run jobs** for a paid workflow. Reruns fail admission; create
   a fresh reviewed dispatch instead.
4. Confirm no equivalent run is active. The global concurrency policy queues
   behind the running paid workflow instead of canceling already billed work.
5. Select the narrow purpose and explicitly acknowledge the 45-minute Windows
   cap or the patch audit's 120 hosted runner-minute cap.
6. Check the repository or organization Actions budget and usage dashboard.
   Billing budgets and alerts are external GitHub settings and cannot be
   guaranteed by repository YAML.

As of 2026-08-04, the parent account has an account-wide Actions monthly budget
of USD 50 with **Stop usage** enabled, plus included-usage and budget alerts.
This is the final external circuit breaker, not a repository-specific promise:
all repositories under the account contribute to it and the setting must be
checked in GitHub Billing each month.

Before a clean-host run:

1. Restore the reviewed baseline before starting the runner.
2. Use only the isolated local test account and unlocked interactive session.
3. Supply the exact artifact URI, artifact SHA-256, source SHA, and snapshot
   identity required by the release-candidate tier.
4. Confirm both runner readiness and clean snapshot restoration in the manual
   dispatch.
5. Stop the runner and revert the VM again after evidence upload.

## Review rule

Any change that adds a hosted Windows or macOS trigger, increases a timeout, adds a
matrix entry, weakens exact-SHA admission, or creates a second paid job must
update `scripts/ci-cost-control-contracts.test.mjs` and include a cost delta in
the PR evidence. A release urgency claim is not an exception to this rule.
