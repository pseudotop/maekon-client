# Public Contribution Governance

This guide defines the public labels, ownership rules, and branch protection
expectations for the Maekon Client hybrid contribution lane.

Maekon Client is still released from the parent source of truth. Public PRs are
welcome for OSS-safe work, then maintainer-approved changes are validated in the
parent source tree before they are exported back to the public repository.

For the contributor-facing lifecycle summary, see
[`public-contributor-path.md`](./public-contributor-path.md).

## Contribution Lanes

Use exactly one lane label when triaging a public issue or PR.

| Label | Use for | Default handling |
| --- | --- | --- |
| `lane:good-first-dx` | Docs, setup notes, small tests, typo fixes, beginner-friendly maintenance | Public PR is encouraged |
| `lane:local-feature` | Local dashboard, local export, settings, or UX work that does not alter capture, consent, egress, or release semantics | Public PR may be accepted after normal review |
| `lane:provider-adapter` | Public provider metadata/spec updates and adapter compatibility fixes | Review egress and credential handling before import |
| `lane:privacy-docs` | Privacy explanations, consent copy, PII docs, safe screenshots, or disclosure guidance | Privacy owner review is expected |
| `lane:trust-core` | Consent, PII masking, capture, audio, automation policy, sandbox, updater, release signing, and local API security | Requires owner review and private validation |
| `lane:enterprise-contract` | Managed sync, team analytics, SSO/RBAC, admin, compliance, or enterprise API contracts | Route to maintainer discussion before implementation |
| `lane:security-disclosure` | Vulnerabilities or suspected sensitive data exposure | Do not discuss details in public issues; use `SECURITY.md` |

## Risk Labels

Use these labels in addition to the lane when a change may affect a protected
surface.

| Label | Meaning |
| --- | --- |
| `risk:privacy` | The change can alter consent, PII masking, capture, raw evidence, retention, or data minimization behavior |
| `risk:security` | The change can alter sandboxing, local API auth, update integrity, dependency trust, or secret exposure |
| `risk:release` | The change can alter packaging, signing, notarization, installer behavior, update flow, or public export |

## Hold Labels

Hold labels block public merge or parent import until the condition is resolved.

| Label | Remove when |
| --- | --- |
| `do-not-merge/security` | Security owner confirms the public thread is safe and required private handling is complete |
| `do-not-merge/private-test` | Maintainers run the needed private gates and summarize the safe result publicly |
| `do-not-merge/needs-owner` | The relevant CODEOWNER or maintainer approves the current patch |
| `do-not-merge/dco` | The required `Signed-off-by` line or legal attestation is present |

## Flow Labels

| Label | Meaning |
| --- | --- |
| `ok-to-test` | A maintainer has reviewed the PR enough to run maintainer-controlled tests |
| `security-reviewed` | Security/privacy review has cleared the public handling path |
| `imported-to-parent` | The public change has been imported into the parent source tree for release validation |

## CODEOWNERS

`.github/CODEOWNERS` is included in the public export and assigns the current
maintainer owner to the full tree. Dedicated teams can replace `@pseudotop`
later without changing the public contribution model.

Sensitive paths are listed explicitly so branch protection can require
CODEOWNER review for trust-core work:

- `.github/**`, release workflows, release scripts, update code, and
  supply-chain metadata;
- `crates/maekon-automation/**`, `crates/maekon-sandbox-worker/**`,
  `crates/maekon-vision/**`, `crates/maekon-audio/**`, `crates/maekon-network/**`,
  and `crates/maekon-storage/**`;
- `src-tauri/**`, `policy/**`, `api/proto/**`, and `specs/providers/**`.

CODEOWNER review is necessary but not sufficient for trust-core changes. Use
the risk and hold labels above when private validation or security review is
also required.

## Branch Protection

Public `main` should use a branch protection rule or ruleset with these
settings:

1. Require a pull request before merging.
2. Require conversation resolution before merging.
3. Require CODEOWNER review.
4. Dismiss stale approvals or require approval of the latest push.
5. Require the stable public checks that run for fork-safe PRs.
6. Block force pushes and direct deletion of `main`.
7. Keep release, signing, and private validation secrets out of fork PR
   workflows.

Do not make private trust-core gates visible as required public checks if doing
so would expose maintainer-only validation names, raw captures, screenshots,
local paths, or maintainer-only infrastructure details. Public summaries should
describe the risk class and safe outcome, not the private evidence.

See [`public-private-ci-split.md`](./public-private-ci-split.md) for the
fork-safe public matrix, maintainer-only gate triggers, and workflow guardrails.

See [`hybrid-import-workflow.md`](./hybrid-import-workflow.md) for the manual
public PR import recipe, attribution fields, and export handoff comments.

See [`good-first-issues.md`](./good-first-issues.md) for public-safe starter
issue rules and the first batch of copy-ready issue seeds.

See [`public-contributor-path.md`](./public-contributor-path.md) for the
public-safe PR lifecycle, evidence checklist, and maintainer response contract.

## Label Sync

Maintainers can create or repair the public label set with:

```bash
scripts/sync-public-contribution-labels.sh OWNER/REPO
```

Run the same command against the public export repository when that repository
is ready to accept public PRs directly.
