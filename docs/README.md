[English](./README.md) | [한국어](./README.ko.md)

# Documentation Index

This directory is organized by document intent.

## Top-level docs

- [DOCUMENTATION_POLICY.md](./DOCUMENTATION_POLICY.md): documentation conventions and maintenance rules
- [install.md](./install.md): installation guide
- [testing/source-build-prerequisites.md](./testing/source-build-prerequisites.md): fresh-checkout build and Tauri sidecar prerequisites
- [guides/public-contribution-governance.md](./guides/public-contribution-governance.md): public contribution labels, CODEOWNERS, and branch protection expectations
- [guides/public-contributor-path.md](./guides/public-contributor-path.md): public-safe contribution lifecycle, evidence checklist, and maintainer handoff expectations
- [guides/public-private-ci-split.md](./guides/public-private-ci-split.md): fork-safe public CI and maintainer-only validation boundary
- [guides/hybrid-import-workflow.md](./guides/hybrid-import-workflow.md): public PR import, attribution, parent validation, and export handoff workflow
- [guides/good-first-issues.md](./guides/good-first-issues.md): public-safe first contribution guide and starter issue batch
- [guides/qc-upload-spool-recovery.md](./guides/qc-upload-spool-recovery.md): isolated upload interruption, restart re-prime, and exact sent-marker verification
- [guides/product-terminology.md](./guides/product-terminology.md): user-facing terminology and high-risk copy SSOT
- [guides/global-alpha-feedback-operations.md](./guides/global-alpha-feedback-operations.md): privacy-safe invited Alpha feedback, withdrawal, and incident-pause contract
- [contracts/public-surface-source-map.v1.json](./contracts/public-surface-source-map.v1.json): source-to-export/generated consumer inventory and publication boundaries

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
