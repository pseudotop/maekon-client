[English](./ADR-022-client-id-generation-ulid.md) | [한국어](./ADR-022-client-id-generation-ulid.ko.md)

# ADR-022: Client ID Generation — prefix+ULID Convention

**Status**: Accepted
**Date**: 2026-05-28
**Scope**: `crates/maekon-core/src/id_generation.rs`, all client crates that generate entity identifiers
**Related**: server ADR-055 (prefix+ULID ID generation), ADR-021 (Config and Consent Core Placement)
**Implementation**: `crates/maekon-core/src/id_generation.rs:14`, `crates/maekon-core/src/lib.rs` (re-export)

---

## Context

The Maekon client generates string identifiers at many call sites across `maekon-core`,
`maekon-network`, `maekon-automation`, `maekon-storage`, and `maekon-vision`. Prior to this
ADR, those sites used `Uuid::new_v4().to_string()` or `format!("{prefix}-{}", Uuid::new_v4())`,
producing RFC 4122 UUID v4 strings.

The server adopted `{prefix}_{ULID}` identifiers under server ADR-055. Cross-boundary
consistency matters when:

- Client-generated IDs appear in sync payloads, audit exports, or gRPC request metadata
  where the server or operators must correlate client-originating and server-originating
  records.
- Sortable IDs reduce storage index fragmentation and enable time-ordered inspection
  without an additional `created_at` column (ULID encodes a 48-bit millisecond timestamp).
- Meaningful prefixes make log scanning and debugging faster
  (`req_01ARZ3NDEKTSV4RRFFQ69G5FAV` is immediately identifiable; a raw UUID is not).

The `consent.rs` comment added in F-RC-C32-03 explicitly deferred this decision:
> *"if cross-boundary trace ID consistency becomes a priority, file an ADR for Rust-side
> `generate_id()` utility"* — deferred in `maekon-core/src/consent.rs`.

The `ulid` crate (`version = "1"`) was already a workspace dependency (used by
`maekon-web`). No new crate dependency is required.

## Scope and Exemptions

`generate_id` (prefix+ULID) is for **entity and correlation identifiers only**.

The following categories are **exempt** and must continue to use
`Uuid::new_v4()` or another CSPRNG-backed primitive:

### Exempt category A — Cryptographic nonces, tokens, and secrets

ULID embeds a predictable 48-bit millisecond timestamp in its high bits and has a
smaller random field (80 bits) than UUID v4 (122 bits of randomness). For
security-sensitive values that require full unpredictability, ULID is the wrong
primitive.

Exempt sites (must stay `Uuid::new_v4()`):

| Site | Role |
|---|---|
| `maekon-automation/src/gui_interaction/crypto.rs` — `new_capability_token()` entropy | SHA-256 is applied to this value to produce a capability token; the input must be CSPRNG-grade |
| `maekon-automation/src/policy/token.rs` — `issue_policy_nonce()` | Signing nonce for HMAC-SHA256 policy tokens; must be unpredictable |
| `maekon-automation/src/controller/mod.rs` — command-confirmation `nonce` | Anti-tamper nonce for pending-confirmation flow; must be unpredictable |
| `maekon-automation/src/gui_interaction/service_execution.rs` — capability-ticket `nonce` | Anti-replay nonce for HMAC-signed execution ticket; must be unpredictable |

### Exempt category B — Server-wire IDs with unverified format contract

Where a client-generated ID is sent to the server and the server-side format validation
is not fully audited, the safest choice is to keep UUID v4 until the server contract
is confirmed to accept ULID format.

Exempt sites:

| Site | Role |
|---|---|
| `maekon-storage/src/sqlite/device_identity.rs` — `device_id` | Sent to the server in `IntegrationBootstrapRequest`; server format contract unverified |

### Exempt category C — RFC-mandated and externally validated IDs

| Site | Reason |
|---|---|
| `maekon-network/src/integration/auth/proof_factory.rs` — `jti` JWT claim | RFC 7519 §4.1.7 requires UUID; server-side JWT validation rejects non-UUID `jti` |
| `maekon-network/src/integration/inbox_coordinator.rs` — `IntegrationEnvelope.nonce` | Server-wire anti-replay field; format validated server-side |
| `maekon-network/src/integration/http_transport/connect.rs` — `IntegrationBootstrapRequest.nonce` | Server-wire protocol field; server may validate format |

### Exempt category D — Non-string Uuid type fields

| Site | Reason |
|---|---|
| `maekon-storage/src/sqlite/test_utils.rs` — `event_id: Uuid` | Field type is `uuid::Uuid`, not `String` |

## Decision

### 1. `generate_id(prefix: &str) -> String` in maekon-core

Client-generated **entity and correlation** string identifiers adopt `{prefix}_{ULID}` shape,
implemented by `generate_id` in `maekon-core::id_generation`:

```rust
// crates/maekon-core/src/id_generation.rs
pub fn generate_id(prefix: &str) -> String {
    validate_prefix(prefix);
    format!("{prefix}_{}", ulid::Ulid::new())
}
```

The function:
- validates the prefix (lowercase ASCII letters/digits/`_`, starts with a letter, max 63 bytes)
- panics on invalid prefix (programmer error, caught at development time)
- is re-exported as `maekon_core::generate_id`
- reuses the existing `ulid = { workspace = true }` dependency

Cryptographic nonces, tokens, server-wire IDs with unverified contracts, RFC-mandated IDs,
and non-string type fields remain on `Uuid::new_v4()` per the exemptions above.

### 2. Prefix registry (this ADR is authoritative)

| Prefix | Context |
|---|---|
| `req` | gRPC / HTTP request correlation ID (`x-request-id`) |
| `ses` | AI session, GUI interaction session |
| `flow` | OAuth / OIDC device authorization flow |
| `sug` | AI suggestion |
| `ann` | Annotation |
| `pomo` | Pomodoro session |
| `ovr` | Recalibration override |
| `aud` | Audit log entry |
| `evt` | Audit event (log_event path) |
| `consent` | Consent record |
| `tkt` | GUI execution ticket (entity ID, not the security nonce inside it) |
| `hl` | Overlay highlight handle |
| `env` | Integration envelope (local identifier) |
| `rcpt` | Integration prompt receipt |
| `q` | Integration state store queue entry |
| `scene` | UI scene |
| `rect` | UI scene rectangle element |
| `ptr` | Pointer action trace |
| `ctx` / `input` / `proc` / `win` / `clip` / `fa` / `tl` | Timeline/event assembler context types |
| `cch` | Coaching engine message |
| `msg` | Generic message |
| `clm` | Memory claim node (ADR-023 local memory graph) |
| `edg` | Memory graph edge (ADR-023 local memory graph) |
| `tcand` | Durable task candidate (ADR-028; effective when ADR-028 is Accepted) |
| `todo` | Human-confirmed durable to-do (ADR-028; effective when ADR-028 is Accepted) |
| `tmut` | Durable task transition receipt (ADR-028; effective when ADR-028 is Accepted) |
| `wctx` | External work-context envelope (ADR-030; effective when ADR-030 is Accepted) |

### 3. Conversion scope

All production `Uuid::new_v4().to_string()` sites that produce **entity or correlation**
string identifiers are converted to `maekon_core::generate_id("<prefix>")`. Sites in the
exempt categories above remain unchanged.

### 4. Test assertion updates

Integration test assertions that checked UUID v4 wire format (36 chars, 4 hyphens,
`uuid::Uuid::parse_str`) for converted entity IDs are updated to check `req_` prefix +
26-char ULID format. Assertions for exempt sites (e.g. `device_id` UUID format) are
retained unchanged.

## Consequences

### Positive

- Cross-boundary entity and correlation IDs (e.g. `x-request-id`, audit `command_id`,
  suggestion IDs, consent records) share the same shape as server ADR-055 IDs — operators
  need one pattern for log searches.
- Sortable IDs enable time-ordered inspection without extra timestamp columns.
- Prefixed IDs are self-describing in logs and debug output.
- No new crate dependency (`ulid` was already a workspace dependency).

### Negative

- Existing persisted entity IDs (e.g. `consent_id` in JSON) generated by prior releases
  will have UUID v4 format. New IDs will have ULID format. The `String` field type
  accommodates both; no migration is required and no schema validation enforces UUID format
  for these fields.
- `generate_id` panics on invalid prefix — this is intentional (programmer error) and will
  be caught in development. All added prefixes are validated at call time.

### Neutral

- `ulid::Ulid::new()` is monotonically increasing within the same millisecond on a single
  thread. Monotonicity is best-effort across threads; uniqueness is guaranteed.
- This ADR does not affect the server registry. Client prefix names may coincide with
  server prefixes by coincidence; they are independent namespaces.
- Security-sensitive sites retain `Uuid::new_v4()` for full 122-bit CSPRNG randomness;
  the exemption rule is permanent, not provisional.

## Alternatives Considered

**A. Keep bare UUID v4 everywhere.**
Rejected. Cross-boundary entity IDs in logs and exports are indistinguishable by origin,
and sortability requires an extra timestamp column. The server's prior commitment to
server ADR-055 also makes divergence confusing for operators.

**B. Use a different ULID crate or roll a custom generator.**
Rejected. The `ulid` crate was already a workspace dependency used by `maekon-web`.
Adding a second ID crate would increase binary size and maintenance surface.

**C. Adopt UUID v7 (time-ordered UUID) for all sites including nonces.**
Rejected. UUID v7 would still embed a predictable timestamp in the high bits, making it
unsuitable for cryptographic nonces. Additionally, the `uuid` crate `v7` feature may pull
additional CSPRNG infrastructure. ULID is lighter and already present in the workspace.

**D. Use `generate_id` for all sites, including cryptographic nonces.**
Rejected after review. ULID's 80-bit random field and predictable timestamp component are
insufficient for security contexts that require full unpredictability. UUID v4 provides
122 bits of CSPRNG randomness and no timestamp leak.

## Update 2026-07-19 — Durable task identifiers

ADR-028 proposes the additive `tcand`, `todo`, and `tmut` prefixes registered in
Decision §2. They take effect only when ADR-028 changes from `Proposed` to
`Accepted`; until then no implementation may mint them. This update does not
change the prefix syntax, generator, validation, or exemption rules in this ADR.

## Update 2026-07-19 — Work-context envelope identifiers

ADR-030 proposes the additive `wctx` prefix registered in Decision §2 for
external work-context envelopes. It takes effect only when ADR-030 changes from
`Proposed` to `Accepted`; until then no implementation may mint it. This update
does not change the prefix syntax, generator, validation, or exemption rules in
this ADR.

## Known Follow-ups

1. **Prefix governance** — New entity identifiers must register their prefix in the table
   above (Decision §2) via a PR amending this ADR. Unregistered prefixes will be flagged
   in code review.
2. **Lint rule** — A future `maekon-lint` rule
   may prohibit bare `Uuid::new_v4().to_string()` calls for entity-ID production code,
   with explicit exemptions for the categories in the Scope section above.
3. **device_id server contract audit** — If a future audit confirms the server accepts any
   opaque string for `device_id`, migrate to `generate_id("dev")` under a new PR.

## Related Docs

- `crates/maekon-core/src/id_generation.rs` — implementation
- `docs/architecture/ADR-021-config-consent-core-placement.md` — consent.rs context
