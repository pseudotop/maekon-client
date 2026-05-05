# ADR-002 GUI V2 Smoke Test Run

**Date**: YYYY-MM-DD
**Tester**: [name]
**Version**: [git hash]
**OS**: [macOS version / Windows version / Linux distro+compositor]

## Environment

| Item | Value |
|------|-------|
| Display | [resolution, DPI scale] |
| Accessibility permission | [granted? Y/N] |
| HMAC Secret configured | [Y/N] |
| AT-SPI daemon (Linux) | [running? Y/N / N/A] |
| Tauri version | 2.x |

## Smoke Scenarios

| # | Scenario | Steps | Expected | Status | Duration | Notes |
|---|----------|-------|----------|--------|----------|-------|
| 1 | Happy path | POST create → POST highlight → POST confirm → POST execute | 200, state=Executed | PASS/FAIL | ms | |
| 2 | Permission denied | Revoke accessibility permission → POST create | 403 Forbidden | PASS/FAIL | ms | |
| 3 | Focus drift | POST create → switch window → POST confirm | 409 Conflict | PASS/FAIL | ms | |
| 4 | Expired ticket | POST confirm → wait 35s → POST execute | 422 Unprocessable | PASS/FAIL | ms | |
| 5 | Overlay failure | Kill overlay window → POST highlight | 503 or graceful fallback | PASS/FAIL | ms | |
| 6 | Session timeout | POST create (TTL=5s) → wait 10s → GET session | 404 Not Found | PASS/FAIL | ms | |
| 7 | Nonce replay | POST execute (success) → POST execute (same ticket) | 422 nonce replay | PASS/FAIL | ms | |
| 8 | Headless/no display | Unset DISPLAY (Linux) or run without GUI | 503 or graceful degrade | PASS/FAIL | ms | |

## Performance Timing

| Operation | P50 | P95 | P99 |
|-----------|-----|-----|-----|
| create_session | ms | ms | ms |
| highlight_session | ms | ms | ms |
| confirm_candidate | ms | ms | ms |
| execute | ms | ms | ms |
| build_candidates (200 elements) | ms | ms | ms |

## Artifacts

- [ ] Screenshot of overlay highlight rendering
- [ ] Log excerpt for each failure scenario
- [ ] tracing span output for performance timing

## Issues Found

| # | Severity | Description | Workaround |
|---|----------|-------------|------------|
| | | | |
