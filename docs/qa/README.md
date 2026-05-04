# QA Result Storage Policy

This directory is the canonical location for manual/reviewed QA execution results.

## What goes where

- QA criteria/checklists (stable definitions):
  - `docs/guides/uiux-qa-sheet.md`
  - `docs/guides/replay-uiux-qa-sheet.md`
  - `docs/qa/ai-integration-readiness-checklist.md`
  - `docs/qa/local-debug-ai-integration-smoke.md`
- Actual QA run outputs (mutable run evidence):
  - `docs/qa/runs/*.md`
- CI machine artifacts (screenshots, Playwright HTML, logs):
  - CI artifact store (GitHub Actions artifacts), linked from run files.
- Project-level mutable quality summary:
  - Latest QA run and workflow pages.

## Public export boundary

The public-minimal export keeps stable QA checklists and smoke procedures, but
excludes mutable run evidence under `docs/qa/runs/`. Those run files can contain
local ports, screenshots, console logs, and CI-only context that are useful for
maintainers but noisy for first-time public users.

## Execution Policy

- Interactive UI/UX QA must be executed with Playwright CLI, not Playwright MCP.
- Preferred interactive commands:
  - `cd crates/oneshim-web/frontend && pnpm qa:pwcli:open`
  - `cd crates/oneshim-web/frontend && pnpm qa:pwcli:snapshot`
  - `cd crates/oneshim-web/frontend && pnpm qa:pwcli:show`
- Attach run evidence (CLI output summary + artifact/report link) to `docs/qa/runs/*.md`.

## Naming convention for run files

Use date-first filenames for chronological sorting:

- `YYYY-MM-DD-uiux-qa-rcN.md`
- `YYYY-MM-DD-replay-qa-rcN.md`

Examples:

- `2026-02-23-uiux-qa-rc1.md`
- `2026-02-23-replay-qa-rc1.md`

`TEMPLATE-uiux-qa-run.md` is only for authoring and must not be referenced as a completed/latest run.

## Minimum fields per QA run

- Build/commit under test
- Scope (pages/features)
- Checklist version used
- P0/P1/P2 findings
- Pass/Partial/Fail table with evidence links
- Decision (go/no-go) and owner
- Follow-up tickets/commits
