# Hybrid Import Workflow

This guide explains how maintainers move an accepted public contribution into
the parent source of truth without weakening attribution, release validation, or
public transparency. It is intentionally manual during Phase 0/1 so the process
can stay auditable while contribution volume is still low.

For the contributor-facing route before import, see
[`public-contributor-path.md`](./public-contributor-path.md).

## Scope

Use this workflow for public PRs that are OSS-safe and ready for maintainer
review:

- docs and developer-experience fixes;
- i18n parity and copy consistency;
- examples with synthetic data only;
- local UI or local export changes that do not alter capture, consent, egress,
  automation policy, sandbox, updater, or release-signing semantics;
- public QA templates that do not reveal private validation details.

Do not use this workflow for security reports or sensitive runtime changes that
need private handling first. Route those changes through the labels and hold
states in [`public-contribution-governance.md`](./public-contribution-governance.md).

## Import Decision

Patch import remains manual for Phase 0/1. Maintainers should not automate patch
import until the hybrid lane has processed at least five low-risk public PRs with
clean attribution, parent validation, and public handoff comments.

The default legal posture for import is DCO. A CLA is not required for ordinary
low-risk public PRs. Route corporate-sponsored, patent-sensitive, or
non-standard IP/licensing contributions through maintainer legal review before
import, as defined in
[`public-contribution-governance.md`](./public-contribution-governance.md).

Manual import is the default because it lets maintainers verify:

- the public patch does not include secrets, private screenshots, raw capture
  content, local absolute paths, or private validation names;
- the contribution lane and risk labels are correct;
- DCO or other legal attestation expectations are satisfied;
- the parent source tree receives the patch with the public PR link and author
  attribution intact;
- release/export validation runs before the public repository is regenerated.

## Manual Recipe

1. Triage the public PR with exactly one lane label and any needed risk or hold
   labels.
2. Confirm the public PR uses synthetic data and contains privacy-safe evidence.
3. Confirm the public PR is ready to import: maintainer review complete, DCO or
   legal posture clear, and no unresolved public review threads.

   When a required DCO or CLA status check is not configured, verify the
   public branch manually before import:

   ```bash
   git log --format=%B <public-base>..<public-pr-head> | grep -Eq '^Signed-off-by: .+ <[^>]+>$'
   ```

   If the command does not find a sign-off, do not clear `do-not-merge/dco`
   unless a maintainer-approved legal attestation link is recorded in the
   public PR or the parent import PR.

4. Create a parent-source branch dedicated to the import.
5. Import the patch from the public PR. Preserve the original commits when they
   are clean and scoped; otherwise squash manually and keep author attribution in
   the parent commit body.
6. Add attribution metadata to the parent commit or parent PR:

   ```text
   Public-PR: https://github.com/<public-owner>/<public-repo>/pull/<number>
   Public-Issue: https://github.com/<public-owner>/<public-repo>/issues/<number>
   Original-Author: <name or handle>
   Co-authored-by: <name> <email>
   Signed-off-by: <name> <email>
   ```

7. Run parent validation appropriate for the lane and risk class.
8. Regenerate the public export from the validated parent source.
9. Update the public PR with a safe handoff comment and apply `imported-to-parent`
   when the public repository is ready for that label.
10. Close or merge the public PR according to the public repository policy.

## Export Handoff

After parent validation passes, the public handoff should include only safe
summary data:

- parent import status;
- public export or release reference;
- public checks that passed;
- whether private validation was required and its safe outcome summary;
- remaining public follow-up work, if any.

Use this style when private validation was involved:

> Imported into the parent source tree and validated for the relevant risk
> class. Maintainer-only validation passed; sensitive evidence is not included in
> this public thread. The validated change will appear in the next public export
> or release reference.

Do not post private logs, screenshots, raw captures, private test names,
maintainer-only infrastructure details, or local absolute paths.

## Automation Trigger

Consider a scripted import helper only after the process has enough real data to
avoid encoding the wrong workflow. The helper should be limited to safe
mechanics:

- fetch the public PR patch;
- verify the public PR URL and author metadata are present;
- create a parent import branch;
- apply the patch without resolving conflicts silently;
- prepare a commit message template with attribution fields;
- run public export guardrails.

The helper must not run private validation, bypass CODEOWNER review, post public
comments automatically, or expose maintainer credentials to fork-controlled
code.

Do not move from manual import to scripted import until all of these are true:

- at least five low-risk public PRs have completed import, parent validation,
  export, and public handoff without attribution corrections;
- the required public check set is stable according to
  [`public-private-ci-split.md`](./public-private-ci-split.md);
- two maintainer dry-runs of the helper reproduce the manual attribution fields
  and stop cleanly on conflicts;
- the helper has a documented rollback path that leaves the parent source tree
  and public PR untouched when a precondition fails.
