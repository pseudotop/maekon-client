[English](./public-contributor-path.md) | [한국어](./public-contributor-path.ko.md)

# Public Contributor Path

This companion guide explains the public Maekon Client contribution path in one
place. It is for contributors who want to understand what is safe to work on,
what evidence to include, and how an accepted public change reaches a release.

Maekon Client is local-first desktop software with privacy-sensitive runtime
surfaces. Public contributions are welcome for OSS-safe work, while sensitive
runtime and release paths receive stronger maintainer gates before release.

## Current Public Contract

Maekon Client currently uses a hybrid contribution model:

1. Contributors open issues or PRs against the public repository.
2. Maintainers triage the work with one contribution lane and any needed risk
   or hold labels.
3. Fork-safe public checks run without secrets or real user data.
4. Accepted public changes are imported into the parent source tree for full
   release validation.
5. The validated source is exported back to the public repository with
   attribution and a safe handoff summary.

This means a public PR can be real product work even when the final release
validation happens in the parent source tree. Maintainers should explain the
route publicly without exposing sensitive validation evidence.

Until broad external PR intake is announced, maintainers publish only the
starter issues and companion documentation that have already been marked
public-safe.

## What To Work On

The safest first contributions are small, reviewable, and useful without
changing privacy-sensitive behavior.

| Lane | Good public work | Start with |
| --- | --- | --- |
| Docs/DX | Setup notes, clearer command output, typo fixes, public guide updates | A scoped issue or a small docs PR |
| i18n parity | Keeping public English and Korean docs aligned | A guide that already has one language complete |
| Synthetic examples | Fake-data examples, sample configs, local-only playbooks | Examples with fake names, fake domains, and fake tokens |
| Public QA templates | Safer reproduction wording, redaction checklists, public evidence notes | A documentation-only PR |
| Privacy documentation | Explaining public privacy behavior without changing masking code | A `lane:privacy-docs` issue |

See [`good-first-issues.md`](./good-first-issues.md) for copy-ready starter
issue seeds and beginner-safe boundaries.

## Ask Before Coding

Ask a maintainer before writing a large patch when the change may touch:

- consent, capture, audio, OCR, input monitoring, or raw evidence handling;
- PII masking behavior or privacy enforcement logic;
- automation policy, sandbox execution, or action confirmation behavior;
- external egress, provider routing, sync, or telemetry;
- installer, updater, signing, notarization, or release automation;
- local API security, dependency trust, or workflow permissions.

Those areas may still accept contributions, but they need the lane and review
route in [`public-contribution-governance.md`](./public-contribution-governance.md)
before implementation begins.

Security vulnerabilities and suspected sensitive data exposure should use the
private reporting path in `SECURITY.md`, not a public issue or PR.

## PR Lifecycle

1. **Pick the lane.** The issue or PR should have exactly one lane label.
2. **Keep the scope narrow.** Smaller PRs are easier to review and import.
3. **Use synthetic data.** Do not include real customer data, private
   screenshots, raw captures, credentials, local absolute paths, or private
   logs.
4. **Run public checks.** Use the checks listed in the issue or the relevant
   public guide.
5. **Describe the evidence.** Include command output, sanitized screenshots, or
   behavior notes that are safe to keep in a public thread.
6. **Respond to public review.** Maintainers may adjust labels, request smaller
   scope, or route sensitive parts to owner review.
7. **Wait for import and export.** When accepted, maintainers import the patch
   for parent validation and publish a safe handoff summary.

The import handoff is documented in
[`hybrid-import-workflow.md`](./hybrid-import-workflow.md). Public and
maintainer-only CI boundaries are documented in
[`public-private-ci-split.md`](./public-private-ci-split.md).

## Evidence Checklist

Include evidence that helps maintainers review the change without making the
public thread unsafe:

- commands run and whether they passed;
- small before/after notes for docs, UI, or behavior changes;
- screenshots only when they are redacted and contain no sensitive content;
- synthetic fixture names, fake domains, and fake tokens;
- links to public issues, discussions, or docs.

Do not include:

- secrets, API keys, tokens, signing materials, or credentials;
- raw screen, audio, input, browser, or OCR captures;
- customer data, personal data, private logs, or real workspace paths;
- maintainer-only evidence, internal infrastructure details, or unpublished
  roadmap drafts;
- vulnerability details that belong in the private security channel.

## Maintainer Responses

Maintainers should keep public comments understandable and safe.

| Response | What it means |
| --- | --- |
| `ok-to-test` | A maintainer has reviewed enough context to run maintainer-controlled checks |
| `security-reviewed` | Security/privacy review cleared the public handling path |
| `do-not-merge/needs-owner` | A responsible owner still needs to review the patch |
| `do-not-merge/private-test` | Maintainer-only validation is needed before release or export |
| `imported-to-parent` | The public patch was imported for full release validation |

When maintainer-only validation is needed, the public thread should receive only
the risk class and safe outcome summary. It should not receive raw logs,
screenshots, capture content, sensitive local paths, or maintainer-only test
details.

## Before Opening A PR

- [ ] The change fits one contribution lane.
- [ ] The PR is small enough to review in public.
- [ ] Examples and tests use synthetic data only.
- [ ] Evidence is safe for a public thread.
- [ ] Sensitive runtime, release, or security work has maintainer guidance.
- [ ] The PR description includes validation commands and AI-assisted
      contribution disclosure when applicable.
