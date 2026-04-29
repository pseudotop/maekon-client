[English](./DOCUMENTATION_POLICY.md) | [한국어](./DOCUMENTATION_POLICY.ko.md)

# Documentation Policy

## Language Policy

- Public-facing documentation in this repository is **English-primary**.
- Multilingual companion docs are maintained for key public guides.
- **Supported languages**: English (primary), 한국어 (ko), 日本語 (ja), 简体中文 (zh-CN), Español (es)
- This includes `README.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, and docs entry pages.
- Companion docs should be kept semantically aligned with English primary docs.
- Naming convention: `{filename}.{lang-code}.md` (e.g., `README.ja.md`, `README.zh-CN.md`)

## Metrics Consistency Policy

- Mutable quality metrics must not be duplicated across documents.
- Live workflow state should be referenced from GitHub Actions run pages instead of being treated as a doc-only source of truth.

## Directory Structure Policy

- `docs/architecture/` is ADR-only.
  - File naming: `ADR-XXX-*.md` and `ADR-XXX-*.ko.md`
  - Non-ADR notes (research, playbooks, runbooks) must not be stored in this directory.
- `docs/guides/` stores operator/developer playbooks and how-to/runbook style documents.
- `docs/contracts/` stores versioned payload/API contracts and generated OpenAPI snapshots.
- `docs/crates/` stores per-crate implementation references.
- `docs/security/` stores security baseline and integrity operations docs.
- `docs/qa/` stores QA templates and run artifacts metadata.
- `docs/testing/` stores testing strategy docs.
- Internal planning, research, review, roadmap, migration, and session workflow artifacts stay out of the public-minimal export. Durable public decisions should graduate to `docs/architecture/`, `docs/guides/`, `docs/contracts/`, or `docs/security/`.

See [docs/README.md](./README.md) for the current document map.

## Why

- Prevent contradictory numbers across docs.
- Reduce maintenance overhead.
- Keep onboarding and release communication consistent.

## Companion Docs

- Korean companion: [DOCUMENTATION_POLICY.ko.md](./DOCUMENTATION_POLICY.ko.md)
