[English](./README.md) | [한국어](./README.ko.md)

# Documentation Index

This directory is organized by document intent.

## Top-level docs

- [DOCUMENTATION_POLICY.md](./DOCUMENTATION_POLICY.md): documentation conventions and maintenance rules
- [install.md](./install.md): installation guide

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

## Naming and placement quick rules

1. Use `ADR-XXX-*` naming only under `docs/architecture/`.
2. Put procedural playbooks/runbooks under `docs/guides/` unless they are security-specific (`docs/security/`).
3. Put API and payload contracts under `docs/contracts/`.
4. Keep English-primary docs and maintain Korean companion docs for key public docs.
