# ADR-002 GUI V2 Security Review

Formal security review of the GUI V2 interaction API (propose-highlight-confirm-execute state machine).

## Scope

- **API surface:** local-only HTTP on `localhost:10090` (Axum web server)
- **Transport:** plain HTTP (no TLS) -- acceptable for localhost-only binding
- **State machine:** 7 REST endpoints managing session lifecycle and execution tickets
- **Signing:** HMAC-SHA256 execution tickets with single-use nonces

## Threat Model

| Property | Detail |
|----------|--------|
| **Network exposure** | None -- binds to `127.0.0.1` only |
| **Attacker model** | Malicious local process running as the same OS user, with full knowledge of the API |
| **Trust boundary** | The OS process boundary; any code running as the logged-in user can call the API |
| **Out of scope** | Remote network attacks, kernel-level compromise, physical access |

## Mitigations

| # | Threat | Mitigation | Implementation | Source | Verified |
|---|--------|-----------|----------------|--------|----------|
| 1 | **Session hijack** | Random UUID capability token per session; required in `x-gui-session-token` header for all endpoints except session creation | `new_capability_token()` generates SHA-256 of a UUID v4 | `crates/oneshim-automation/src/gui_interaction/crypto.rs:53-57` | Pending |
| 2 | **Ticket replay** | Single-use nonce per ticket; consumed nonces tracked in `HashSet<String>` per session | `prepare_execution()` checks `used_ticket_nonces` before accepting; inserts nonce on success | `crates/oneshim-automation/src/gui_interaction/service.rs:568-573` | Pending |
| 3 | **Ticket forgery** | HMAC-SHA256 signature over `session_id\|scene_id\|element_id\|action_hash\|focus_hash\|issued_at\|expires_at\|nonce` | `sign_ticket()` computes HMAC; `verify_ticket()` recomputes and compares | `crates/oneshim-automation/src/gui_interaction/crypto.rs:12-37` | Pending |
| 4 | **Focus spoofing** | Atomic re-validation at execute time: re-capture current focus via `FocusProbe`, compare `focus_hash` against ticket binding; retry up to 2 times with 500ms delay | `validate_execution_binding()` called in `prepare_execution()` with retry loop | `crates/oneshim-automation/src/gui_interaction/service.rs:520-561` | Pending |
| 5 | **Stale sessions** | TTL-based cleanup every 30 seconds; default session TTL: 300 seconds | `expire_sessions()` spawned by `ensure_cleanup_task()` using `Arc::downgrade` weak reference | `crates/oneshim-automation/src/gui_interaction/service.rs:69-86, 735-777` | Pending |
| 6 | **HMAC key leak** | Secret sourced from env var only (`ONESHIM_GUI_TICKET_HMAC_SECRET`); never logged; fail-closed on missing | `require_hmac_secret()` returns `Unavailable` (HTTP 503) when env var is absent or empty | `crates/oneshim-automation/src/gui_interaction/service.rs:779-783` | Pending |

## Verification Checklist

- [ ] **M1 - Capability token generation:** Confirm `new_capability_token()` uses `Uuid::new_v4()` + SHA-256, producing a 64-hex-char token (`crypto.rs:53-57`)
- [ ] **M2 - Token validation on every request:** Confirm `assert_capability_token()` is called in `get_session`, `highlight_session`, `confirm_candidate`, `prepare_execution`, `cancel_session`, and `subscribe_session` (`service.rs:701-716`)
- [ ] **M3 - Nonce tracking and rejection:** Confirm `used_ticket_nonces.contains()` check precedes `insert()` in `prepare_execution()` (`service.rs:568-573`)
- [ ] **M4 - HMAC payload completeness:** Confirm `ticket_signature_payload()` includes all binding fields: `session_id`, `scene_id`, `element_id`, `action_hash`, `focus_hash`, `issued_at`, `expires_at`, `nonce` (`crypto.rs:39-51`)
- [ ] **M5 - Signature verification uses constant-time comparison:** Confirm `mac.verify_slice()` is used (HMAC crate provides constant-time verify) (`crypto.rs:35-36`)
- [ ] **M6 - Focus hash includes PID and app name:** Confirm `ExecutionBinding` carries `focus_hash`, `app_name`, and `pid` for `validate_execution_binding()` (`service.rs:520-524`)
- [ ] **M7 - Session cleanup uses weak reference:** Confirm `Arc::downgrade` prevents cleanup task from keeping the service alive after drop (`service.rs:74`)
- [ ] **M8 - HMAC secret never appears in logs:** Grep for `hmac_secret` in `tracing::` macros; confirm no log output contains the value
- [ ] **M9 - Fail-closed on missing secret:** Confirm `require_hmac_secret()` returns `Unavailable` error (not a default/empty key) (`service.rs:779-783`)
- [ ] **M10 - Ticket expiry with grace window:** Confirm `is_expired_past_grace()` adds `TICKET_EXPIRY_GRACE_SECS` (5s) to handle clock skew (`service.rs:501-518`)

## Residual Risks

| # | Risk | Severity | Rationale |
|---|------|----------|-----------|
| R1 | **Same-user local access** | Medium | Any process running as the logged-in OS user can call the API on `localhost:10090`. This is inherent to the local-agent model. Mitigation: capability tokens limit session access to the token holder. |
| R2 | **No TLS on localhost** | Low | Traffic is loopback-only and never crosses a network boundary. TLS on localhost adds certificate management complexity with no practical security benefit. |
| R3 | **Overlay compositor trust** | Low | The highlight overlay relies on the OS window compositor for correct rendering. A compromised compositor could misrepresent element positions. This is outside the application's control. |
| R4 | **Capability token in memory** | Low | The token exists in process memory (HashMap). A local attacker with `ptrace` access could extract it. Mitigated by OS-level process isolation and session TTL expiry. |
| R5 | **Clock skew on ticket expiry** | Low | If the system clock jumps, tickets may expire prematurely or be accepted past their TTL. Mitigated by the 5-second grace window (`TICKET_EXPIRY_GRACE_SECS`). |

## Recommendations

1. **Periodic rotation of HMAC secret:** Consider documenting a rotation procedure for the `ONESHIM_GUI_TICKET_HMAC_SECRET` env var (active sessions will fail after rotation; this is acceptable for local use).
2. **Rate limiting:** Consider adding per-IP or per-session rate limiting on session creation to prevent resource exhaustion from a rogue local process.
3. **Audit trail:** Wire GUI session events into `AuditLogger` to maintain a forensic timeline of all state transitions and denied operations (see Task 4 of the implementation plan).

## Related Documents

- [GUI Interaction Contract](../contracts/gui-interaction-contract.md) -- schema definitions
- [GUI V2 API Examples](../contracts/gui-interaction-v2-examples.md) -- cURL examples
- [GUI Troubleshooting Runbook](../guides/adr-002-gui-troubleshooting-runbook.md) -- operator diagnostics
- [Standalone Integrity Baseline](./standalone-integrity-baseline.md) -- broader security baseline
