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

Until a required DCO or CLA status check is configured, `do-not-merge/dco` is
the enforcement point for public contribution provenance. If no required DCO or
CLA status check is configured, keep `do-not-merge/dco` in place until the
verification command output or approved attestation link is recorded. Record the
safe evidence in the public PR when possible; otherwise record it in the parent
import PR before clearing the hold.

## DCO and CLA Decision

DCO `Signed-off-by` is the default legal attestation for ordinary public
contributions. Do not require both DCO and a CLA for the default docs, DX, i18n,
synthetic-example, public-QA, or low-risk local-feature path.

Use `do-not-merge/dco` as the provenance hold until either the public commits
contain a valid sign-off or a maintainer-approved attestation link is recorded.
If the contribution is corporate-sponsored, patent-sensitive, or carries a
non-standard IP/licensing assertion, add `do-not-merge/needs-owner` and route it
to maintainer legal review before import. A CLA may be requested only for that
elevated route; it is not the default entry requirement.

Treat these as CLA/legal-review triggers:

- a contributor states the work is owned by an employer, sponsor, or client and
  cannot rely on the ordinary DCO certification alone;
- the patch introduces or materially changes patented, patent-pending, or
  proprietary algorithms, protocols, provider integrations, or license grants;
- the change touches trust-core or enterprise-contract behavior and includes a
  substantial new implementation rather than public documentation or synthetic
  fixtures.

## Flow Labels

| Label | Meaning |
| --- | --- |
| `ok-to-test` | A maintainer has reviewed the PR enough to run maintainer-controlled tests |
| `security-reviewed` | Security/privacy review has cleared the public handling path |
| `imported-to-parent` | The public change has been imported into the parent source tree for release validation |

## Phase 2 Public-Canonical Readiness

Phase 0/1 remain hybrid: public PRs are reviewed publicly, then accepted patches
are imported into the parent source of truth before release/export. Phase 2 must
start with a partial public-canonical surface, not a repository-wide switch.

The first candidate surfaces are:

1. public docs and contributor guides, including `README*`, `CONTRIBUTING.md`,
   `docs/README*`, and public `docs/guides/*` files;
2. public i18n companion documentation for those guides;
3. synthetic examples and public QA templates that use fake data only.

Do not promote trust-core code, release/signing automation, updater behavior,
provider egress code, sandbox/automation policy, `src-tauri/**`, `policy/**`,
or supply-chain enforcement files to public-canonical status without a separate
owner decision. Those surfaces remain parent-gated even if public PRs are
accepted for them.

Before promoting any surface, maintainers need all of the following:

- at least five low-risk public PRs processed end-to-end with attribution,
  parent import, parent validation, and public handoff comments;
- the required public check set in `public-private-ci-split.md` is stable;
- public export guardrails pass without private-plan, private-test, local-path,
  or maintainer-only evidence leaks;
- a documented rollback path back to parent-only import for that surface.

## CODEOWNERS

`.github/CODEOWNERS` is included in the public export and assigns the current
maintainer owner to the full tree. Dedicated teams can replace `@pseudotop`
later without changing the public contribution model.

Sensitive paths are listed explicitly so branch protection can require
CODEOWNER review for trust-core work:

- `.github/**`, release workflows, release scripts, update code, and
  supply-chain metadata;
- public export/import boundary files: `scripts/export-public-repo.sh`,
  `scripts/update-public-repo-clone.sh`, `scripts/public-repo-include.txt`,
  `scripts/public-repo-exclude.txt`, `scripts/public-export-provenance.py`,
  `scripts/scan-public-export-secrets.py`, and
  `scripts/sync-public-contribution-labels.sh`;
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
