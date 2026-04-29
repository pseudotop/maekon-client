[English](./history-hygiene.md) | [한국어](./history-hygiene.ko.md)

# History Hygiene Guide

This guide defines commit history hygiene for release safety and sensitive-context abstraction.

## Merge policy

1. Prefer squash-merge for feature/fix PRs into `main`.
2. Keep merge commit subjects abstract and intent-focused (avoid secrets/config payload details).
3. Preserve detailed execution logs in CI artifacts and runbooks, not commit subjects.

## Sensitive-context rules

Do not include the following in commit subjects:

- raw env assignments (for example `ONESHIM_...=...`, `MACOS_...=...`)
- words that imply direct secret disclosure (`password`, `private key`, `p12`, `token=`)
- provider account identifiers when avoidable

Use abstract terms instead:

- `auth credential handling`
- `notarization profile wiring`
- `remote provider env alignment`

## CI enforcement

- `scripts/verify-commit-message-hygiene.sh` checks new commit subjects for sensitive keywords and raw env assignment patterns.
- `scripts/verify-http-interface-manifest.sh` ensures public HTTP interface history is versioned and synchronized with routes.
- `scripts/verify-http-openapi-sync.sh` ensures generated OpenAPI snapshots remain synchronized with the interface manifest.

## Operational note

History rewriting on an already-published `main` branch is discouraged.
If historical redaction is unavoidable, use a dedicated incident procedure with explicit force-push approval and downstream coordination.
