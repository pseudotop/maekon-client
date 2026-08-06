[English](./ADR-034-http-core-extraction.md) | [한국어](./ADR-034-http-core-extraction.ko.md)

# ADR-034: `maekon-http-core` — a shared outbound-HTTP substrate below the adapters

**Status**: Proposed — 2026-08-05
**Date**: 2026-08-05
**Scope**: new crate `maekon-http-core` (hardened outbound client, retry/backoff, circuit breaker); `maekon-network` (becomes a consumer, keeps its transports); future `maekon-integration`
**Related**: ADR-001 §3 (DI + adapter boundaries), D7 circuit-breaker broadening (2026-04-20), #9855 (Google Calendar OAuth factory), #9639 (integration rollup)
**Issue**: TBD

---

## Context

Third-party integrations live at `crates/maekon-network/src/integration/` — **12,707 lines** measured 2026-08-05. That subtree already contains a complete Google Calendar connector (1,561 lines: HTTP API, mapping, cursor, health, two integration test files) whose only missing piece is a registered OAuth factory (#9855). The product intent is to add **several more connector presets**, deliberately scoped to essential tools rather than a broad catalog.

The architecture rule in `CLAUDE.md` is explicit:

> **Forbidden**: Direct dependency between adapter crates (e.g. monitor → storage). All cross-crate communication must go through `maekon-core` traits.

So "move integrations into their own crate" cannot be done naively: `integration/` rests on primitives that live inside `maekon-network`, and a new adapter crate may not depend on another adapter crate.

### What `integration/` actually borrows

Measured by `grep -rh "^use crate::" crates/maekon-network/src/integration`:

```
crate::error::NetworkError
crate::outbound::{hardened_client_builder, read_text_capped, BodyReadError, TransportPolicy}
crate::provider_error_body::provider_error_body_state
crate::resilience::{jittered_backoff_delay, RetryBackoffGate, RetryBackoffPolicy,
                    extract_retry_after, scale_duration, MAX_RETRY_AFTER_SECS}
```

These are not incidental helpers. `hardened_client_builder` fixes the redirect policy; `read_text_capped` bounds response-body reads (a DoS guard). **Duplicating them into a second crate is the one option that must be rejected outright** — a security primitive with two copies drifts, and only one copy gets the next fix.

### Why the primitives are not integration-specific

| Module | LOC | Crate-internal deps | Consumer modules | of which `integration/` |
|---|---:|---|---:|---:|
| `outbound.rs` | 327 | **none** | 2 | 1 |
| `resilience.rs` | 361 | `crate::error::NetworkError` (1 site) | 14 | 3 |
| `circuit_breaker.rs` | 585 | **none** | 8 | 0 |

`resilience` is a substrate for the *whole* network crate — `auth`, `http_client`, `sse_client`, `batch_uploader`, `grpc`, the four AI clients, `sync`, `context_home`. It is not something integrations own and lend out. That is precisely why it belongs *below* both, not inside either.

## Decision

Introduce **`maekon-http-core`**: a small crate holding outbound-HTTP mechanics with no domain knowledge and no transport opinions.

```
                maekon-core            (domain models + port traits)
                  ↑        ↑
       maekon-http-core    │           (hardened client, retry/backoff, breaker)
          ↑         ↑      │
maekon-network   maekon-integration    (adapters — neither depends on the other)
```

`maekon-http-core` may depend on `maekon-core` (it already reaches `maekon_core::backoff::exponential_delay`). No adapter depends on another adapter, so ADR-001 §3 holds.

### Contents

| Moved | Why |
|---|---|
| `outbound.rs` | zero internal deps; lifts verbatim |
| `resilience.rs` | one internal dep, severable (below) |
| `circuit_breaker.rs` | zero internal deps; **included deliberately** — see below |

`circuit_breaker` has no `integration/` consumer today, so moving it is not strictly required. It is included because leaving it behind recreates the problem this ADR exists to solve: a connector calling a third-party API will want a per-endpoint breaker, and its only options would be to depend on `maekon-network` (forbidden) or to duplicate it (rejected above).

### The one coupling to sever

`RetryBackoffGate::on_failure` takes `&NetworkError` (`resilience.rs:114`) solely to answer one question — *did the server hand us a Retry-After hint?*

```rust
pub fn on_failure(&mut self, now: Instant, error: &NetworkError) -> Duration {
    let delay = match error {
        NetworkError::RateLimited { retry_after_secs } => { /* clamp */ }
        _ => jittered_backoff_delay(...),
    };
```

Replace the error reference with the answer itself:

```rust
/// What a failed attempt tells the backoff gate.
pub enum RetryHint {
    /// The server asked us to wait this many seconds (429 / Retry-After).
    After(u64),
    /// No hint — use exponential backoff.
    None,
}
```

`maekon-network` keeps a one-line `impl From<&NetworkError> for RetryHint`, so its call sites become `gate.on_failure(now, (&err).into())` and the clamp behaviour is unchanged. A future `maekon-integration` supplies its own conversion from its own error type.

This is the **entire** semantic change in the extraction. Everything else is an import-path rewrite.

## Alternatives rejected

**Leave `integration/` in `maekon-network`.** Viable today and cheaper — but the crate's stated job is client↔server transport (HTTP/gRPC/SSE/auth), and third-party connectors are a different concern that will keep growing. This ADR is written *because* multiple connector presets are planned; with one connector the extraction would not pay for itself.

**Promote the primitives into `maekon-core`.** Rejected: `hardened_client_builder` returns a `reqwest::ClientBuilder`. Putting it in `maekon-core` pulls `reqwest` into the domain crate that every other crate depends on, destroying the property that makes `maekon-core` testable without infrastructure (ADR-001 §5).

**Duplicate the primitives into `maekon-integration`.** Rejected, as above: `read_text_capped` is a body-size bound and `hardened_client_builder` fixes redirect policy. Two copies of a security control is strictly worse than one shared copy.

## Migration

Deliberately staged so no phase leaves the tree in a half-moved state:

1. **P1 — create the crate, move the three modules, sever the coupling.** `maekon-network` re-exports the moved names (`pub use maekon_http_core::…`) so no other crate changes yet. Verifiable: workspace builds, existing tests pass unmodified.
2. **P2 — repoint consumers.** Rewrite the ~20 `use crate::{outbound,resilience,circuit_breaker}::…` sites to `maekon_http_core::…` and drop the re-exports. Verifiable: no `pub use maekon_http_core` remains in `maekon-network`.
3. **P3 — extract `maekon-integration`** and move `integration/` into it, depending on `maekon-core` + `maekon-http-core`. Verifiable: `./scripts/check-crate-boundaries.sh` passes with the new crate listed; `maekon-integration` does not appear in `maekon-network`'s dependency tree, nor the reverse.
4. **P4 — connector registry.** An explicit registry naming the supported connectors, with a per-connector feature flag, so "essential tools only" is a compile-time fact rather than a convention.

P1 and P2 stand alone and are worth doing even if P3 never happens: they make the resilience substrate a named, testable unit instead of a set of sibling modules.

## Consequences

**Positive.** The adapter rule holds without duplication. A connector crate can use the same hardened client and breaker as the rest of the client. `maekon-network` shrinks toward its actual job. Per-connector feature flags become expressible.

**Negative.** One more crate to build, and a churn of import paths across ~20 files in P2. A cross-crate change to a resilience primitive now touches two crates instead of one.

**Neutral.** `maekon-http-core` is a natural home for future outbound concerns (proxy policy, per-host rate limits) that today would have no obvious owner.

## Non-goals

- Changing retry, backoff, breaker, or redirect **behaviour**. The extraction is behaviour-preserving; `RetryHint` reproduces the existing `RateLimited` clamp exactly.
- Deciding which connectors ship. That is the P4 registry's content, not this ADR's.
- Moving `NetworkError`. It stays in `maekon-network`; only `resilience`'s single use was severed (P1).
- gRPC/SSE transports. They are client↔server concerns and remain in `maekon-network`.

## Amendments

**P3 executed (2026-08-05).** Two deviations from the plan as drafted, both forced by facts P3 surfaced:

1. **`provider_error_body` moved to `maekon-http-core` after all**, reversing the original non-goal. The non-goal was written for P1's scope; P3 forced a decision, because the module had consumers on *both* sides of the new boundary (five in `maekon-network`, one in `maekon-integration`'s `http_transport`) and neither side may depend on the other. It is 107 lines depending only on `reqwest::StatusCode` — pure outbound mechanics squarely within this crate's charter — so moving it beat both duplication and severing.
2. **The two Google Calendar OAuth literals moved to `maekon_core::ports::oauth`** (`GOOGLE_CALENDAR_PROVIDER_ID`, `GOOGLE_CALENDAR_READONLY_SCOPE`). The connector (in `maekon-integration`) uses them as its SecretStore namespace and requested scope; the OAuth provider registry (in `maekon-network`) builds the matching provider config from the same literals. Adapters may not depend on each other, and a drift between two copies would break token lookup silently, so the one copy lives in core. The connector module re-exports them.

The `From<&NetworkError> for RetryHint` impl sketched in the Decision section was **not** implemented: the gate's only consumer holds `CoreError`, so the impl would have been dead code on arrival. `runtime_loop` converts via a local 7-line `retry_hint()` instead.

**P4 executed (2026-08-06)**, deliberately narrower than "registry + everything it enables":

- `maekon-integration/src/connectors.rs` is the compile-time registry: one `BuiltinConnector` row per essential tool, each behind its own Cargo feature (`connector-google-calendar`, default-on). Tests pin the set to exactly the essential list and enforce the MK-EXT read-only-scope invariant mechanically.
- The composition root (`oauth_provider_registry.rs`) reads the registry and appends an `OAuthProviderConfig` per provisioned entry — the #9855 registration, executed as decision A (calendar is essential). Absent credential (`MAEKON_GOOGLE_CALENDAR_OAUTH_CLIENT_ID`) means an inert connector, not an error.
- **Explicitly out of P4**: reviving the MK-EXT extension IPC/UI surface. #9639 retired it because nothing called `register_package` and the IPC advertised a feature that could never work; its revival order (annotations → IPC lines → a real `register_package` call site) is documented in `src-tauri/src/lib.rs` and guarded by `tests/ipc_command_contract.rs`. Wiring the OAuth provider is invisible plumbing and safe; reviving half the surface would recreate the dead-advertising defect. The user-reachable vertical (connect UI → sync scheduling → timeline) is a separate slice on top of this ADR's layering.
