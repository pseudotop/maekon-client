[English](./README.md) | [한국어](./README.ko.md)

# Documentation Index

This directory is organized by document intent.

## Top-level docs

- [DOCUMENTATION_POLICY.md](./DOCUMENTATION_POLICY.md): documentation conventions and maintenance rules
- [install.md](./install.md): installation guide
- [testing/source-build-prerequisites.md](./testing/source-build-prerequisites.md): fresh-checkout build and Tauri sidecar prerequisites

## Directories

- `architecture/`: ADR-only architectural decisions
- `guides/`: operator/developer playbooks, runbooks, and how-to guides
- `contracts/`: versioned API/payload contracts and generated OpenAPI snapshots
- `crates/`: crate-level implementation references
- `security/`: security baseline and integrity operations docs
- `qa/`: QA templates, execution run logs, and artifacts metadata
- `testing/`: testing strategy docs

Internal planning, research, review, roadmap, and migration archives are kept
out of the public-minimal export. Durable decisions that matter to public
contributors should be promoted into ADRs, guides, contracts, or security docs.

## Architecture and index policy

- Public readers should start with `docs/architecture/README.md` for ADRs, then
  use `docs/guides/`, `docs/contracts/`, `docs/security/`, and crate docs for
  shipped behavior.
- Internal maintainers should use the plan index in the parent SSOT before TC
  catalog backfill or architecture promotion. That index can point to private
  planning, research, and TC records that are intentionally absent from the
  public export.
- Public docs and exported source comments must remain self-contained. If a
  source comment needs a durable explanation, first promote the behavior into a
  public ADR, guide, contract, security doc, or crate doc.

## Naming and placement quick rules

1. Use `ADR-XXX-*` naming only under `docs/architecture/`.
2. Put procedural playbooks/runbooks under `docs/guides/` unless they are security-specific (`docs/security/`).
3. Put API and payload contracts under `docs/contracts/`.
4. Keep English-primary docs and maintain Korean companion docs for key public docs.
