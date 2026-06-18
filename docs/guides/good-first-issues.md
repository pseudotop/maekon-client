# Good First Issues

This guide is the public-safe on-ramp for first-time Maekon Client
contributors. It explains which starter issues are intentionally small, which
areas are off-limits for beginner work, and what evidence maintainers expect in
a first PR.

See also: [CONTRIBUTING.md](../../CONTRIBUTING.md),
[public contributor path](./public-contributor-path.md),
[public contribution governance](./public-contribution-governance.md), and
the [public/private CI split](./public-private-ci-split.md).

## Public-Safe Starter Rule

A `good first issue` must be useful without touching trust-core behavior. It
should be reviewable in public, use synthetic data only, and have a validation
path that a contributor can run without private credentials or private test
artifacts.

Good starter issues usually fit one of these lanes:

| Lane | Good starter examples | Default labels |
| --- | --- | --- |
| Docs/DX | Setup notes, typo fixes, clearer command output explanations | `good first issue`, `lane:good-first-dx` |
| i18n parity | Translate a public guide or sync public wording between English and Korean | `good first issue`, `lane:good-first-dx` |
| Synthetic examples | Add example inputs that use fake names, fake domains, and fake tokens | `good first issue`, `lane:good-first-dx` |
| Public QA templates | Improve redaction checklists, public evidence wording, or reproduction notes | `good first issue`, `lane:good-first-dx` |
| Privacy documentation | Clarify public privacy guidance without changing masking behavior | `good first issue`, `lane:privacy-docs`, `risk:privacy` |

## Not Good-First

Do not attach `good first issue` to these surfaces, even when the change looks
small. They need owner review and may need maintainer-only validation.

| Surface | Why it is not beginner-safe |
| --- | --- |
| Screen, audio, OCR, or input capture behavior | Can affect consent, raw evidence, and privacy promises |
| PII masking implementation or sanitizer regression tests | Can change trust-core privacy behavior |
| Automation policy, sandbox worker, or action execution | Can affect local safety boundaries |
| External egress, AI provider routing, sync, or telemetry | Can expose data outside the local trust boundary |
| Updater, installer, signing, notarization, release workflows | Can affect supply-chain and release integrity |
| Private CI, maintainer-only test catalogs, or maintainer-only evidence | Can leak internal validation details |
| Fork workflow secrets or GitHub Actions permissions | Can expose credentials to untrusted code |

If an issue touches any of those surfaces, use the labels in
[public contribution governance](./public-contribution-governance.md) instead of
marking it as beginner-friendly.

## First PR Workflow

1. Pick one starter issue with exactly one lane label.
2. Read the linked guide or file before editing.
3. Keep the change narrow and public-safe.
4. Use synthetic examples only. Avoid real customer data, private screenshots,
   raw capture text, raw audio/input data, credentials, local absolute paths, and
   private logs.
5. Run the smallest relevant checks listed in the issue.
6. Open a PR with the issue number, commands run, and privacy-safe evidence.

For docs-only changes, useful checks are usually:

```bash
git diff --check
./scripts/check-language.sh i18n
```

For Rust source changes outside trust-core surfaces, maintainers may ask for:

```bash
cargo fmt --check
cargo test -p <crate>
```

Do not run or request maintainer-only private gates from a fork PR. Maintainers
summarize those results publicly when they are needed.

## Starter Issue Batch

Use these copy-ready seeds for the first public-safe issue batch. Publish them
only when the public repository is ready for external PR intake.

### GFI-DOC-01: Clarify Fresh Checkout Setup

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- Improve wording in `docs/testing/source-build-prerequisites.md` or
  `docs/install.md`.
- Explain one confusing setup step with public commands only.
- Keep the change documentation-only.

Validation:

- `git diff --check`
- Manual link check for changed relative links

Out of scope:

- Release signing, installer behavior, updater behavior, private build scripts,
  or local absolute paths.

### GFI-I18N-01: Add a Korean Companion for a Public Guide

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- Add or refresh a Korean companion for a public guide that already has an
  English source.
- Keep headings and link targets aligned with the English document.
- Leave product identifiers, command names, log keys, and file paths in English.

Validation:

- `git diff --check`
- `./scripts/check-language.sh i18n`

Out of scope:

- Translating internal planning, private review, roadmap, or maintainer-only test
  files.

### GFI-EXAMPLE-01: Add a Synthetic Automation Playbook Example

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- Add one small example to `docs/guides/automation-playbook-templates.md`.
- Use fake services, fake domains, and fake user names.
- Describe expected local-only behavior without promising managed cloud sync.

Validation:

- `git diff --check`
- Manual check that the example contains no real credentials or private data

Out of scope:

- Changing automation policy execution, sandbox worker behavior, egress rules, or
  runtime permissions.

### GFI-QA-01: Improve Public QA Evidence Wording

Labels: `good first issue`, `lane:good-first-dx`

Scope:

- Improve public QA wording in `docs/qa/README.md` or a public QA checklist.
- Make redaction and reproduction expectations clearer.
- Keep examples synthetic and safe for public PR comments.

Validation:

- `git diff --check`
- Manual check that no private screenshots, raw captures, private logs, or
  private test names are referenced

Out of scope:

- Adding maintainer-only test names, raw maintainer evidence, or
  maintainer-only artifact paths.

### GFI-PII-DOC-01: Add Synthetic Privacy Documentation Examples

Labels: `good first issue`, `lane:privacy-docs`, `risk:privacy`

Scope:

- Add documentation-only before/after examples to
  `docs/guides/pii-sanitization-contract.md`.
- Use fake values such as `user@example.test`, `sk-test-redacted`, and
  `/Users/example/project`.
- Explain expected markers without changing sanitizer code.

Validation:

- `git diff --check`
- Manual check that examples are synthetic and do not describe private test
  internals

Out of scope:

- Editing `crates/maekon-vision/**`, `src-tauri/**`, sanitizer behavior, or
  sanitizer regression tests. Those are trust-core unless a maintainer
  reclassifies the work.

## Maintainer Triage Checklist

Before publishing a starter issue:

- [ ] The issue has exactly one lane label.
- [ ] It does not touch any trust-core surface listed above.
- [ ] It can be completed with public code, public docs, and synthetic data.
- [ ] It names the smallest useful files to edit.
- [ ] It lists public checks only.
- [ ] It tells contributors not to include secrets, private screenshots, raw
      captures, private logs, customer data, or local absolute paths.
- [ ] It does not require access to non-public plans, maintainer-only test
      catalogs, or internal release evidence.

## Getting Help

Open a discussion or comment on the issue before implementing if the scope looks
broader than the starter description. When in doubt, keep the PR smaller and ask
maintainers whether the issue should move to a different lane.
