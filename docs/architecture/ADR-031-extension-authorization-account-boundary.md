[English](./ADR-031-extension-authorization-account-boundary.md) | [한국어](./ADR-031-extension-authorization-account-boundary.ko.md)

# ADR-031: Extension Authorization and Account Boundary

**Status**: Accepted (Amended 2026-07-21 after 3-loop review — see [Amendment 2026-07-21](#amendment-2026-07-21-review-resolutions))
**Date**: 2026-07-19
**Scope**: `maekon-core` Extension authorization models and ports, future account-scoped OAuth/Keychain adapter, runtime capability broker, readiness UI/API
**Related**: ADR-008 (token refresh), ADR-021 (consent), ADR-026 (consent port), ADR-029 (Extension boundary), ADR-030 (work-context convergence)
**Issue**: #8584 (`MK-EXT-01.S03`)

---

## Context

The existing standalone provider path from #5713 and the integration lifecycle
from #8080 provide useful OAuth, OS-Keychain, readiness, and revoke primitives.
They do not define an Extension installation's accounts, capability grants,
provider scopes, cursor ownership, or dynamic revoke semantics.

In particular, the current `OAuthPort` and `OAuthClient` are keyed by
`provider_id`. Their token namespace and refresh lock therefore describe one
managed AI-provider connection, not multiple accounts of the same provider in
multiple Extension installations. Reusing that key as an Extension account
boundary would mix credentials and could apply one account's scope or refresh
result to another account.

OAuth authorization is also not product consent. A provider can grant a broad
scope while Maekon consent, the package manifest, the installation grant, or a
managed policy allows only a narrower operation. A UI that reduces installed,
enabled, authorized, granted, and ready to one `connected` boolean can expose a
sync control before the backend is permitted or able to use it.

This ADR defines a dynamic Extension authorization contract before a provider
connector, scheduler, or storage schema is implemented. It does not generalize
the current managed-AI OAuth port by assertion, implement provider endpoints,
or make any Extension available.

## Decision

### 1. Authorization has an account-scoped canonical identity

The canonical `ExtensionAccountKey` contains:

```text
extension_id || install_id || provider_id || account_id
```

`account_id` is an opaque local account subject derived from an issuer-qualified
provider subject and, when applicable, tenant/workspace identity. It is never a
display name, email address, login hint, access token, or provider URL. If the
provider exposes no stable non-email subject, multi-account support remains
`unavailable(account_identity_unstable)`.

The local value is non-reversible and installation-bound:

```text
account_id = base32(HMAC-SHA256(
  local-account-key,
  "extension-account/v1\0" || issuer || provider_subject || tenant_or_workspace?
))
```

Canonical encoding uses length prefixes and explicit missing-value markers.

An account subject from one issuer, tenant, Extension, or installation is never
equal to the same text from another boundary. Cursor, secret, grant, health,
rate-limit, cache, projection, provenance, refresh lock, and in-flight work keys
all include the complete `ExtensionAccountKey`.

The logical OS-Keychain namespace is:

```text
extension/{extension_id}/{install_id}/{account_id}
```

Every path segment is validated and length-bounded. `account_id` is the opaque
local subject above, so a Keychain inventory cannot become an email or tenant
directory. Provider and tenant identity are bound into the value metadata and
namespace derivation; a namespace lookup must verify both before returning an
opaque credential handle.

### 2. Existing primitives are reused only within their proven boundary

| Existing asset | Disposition | Extension rule |
|---|---|---|
| `ConsentManagerPort` / `effective_permissions` | reuse | live product-consent term; withdrawal and erasure remain authoritative |
| `OAuthPort` / `OAuthClient` | reuse implementation patterns, not its key shape | PKCE, callback cancellation, token classification, redirect-disabled credential POSTs, and refresh coordination inform the new adapter; provider-only APIs do not satisfy multi-account Extension authorization |
| `SecretStore` / `KeychainOps` | reuse adapter | secret values remain OS-Keychain-only; the Extension namespace is account/install scoped and enumeration metadata is privacy-sensitive |
| `InternalRangePolicy` and hardened outbound client | reuse | remote authorization/token/revoke/introspection endpoints require the strict policy in this ADR |
| provider readiness snapshots | reuse vocabulary | Extension readiness is a separate account/install snapshot and may not inherit AI-provider availability |
| ADR-029 lifecycle/grant axes | authoritative | this ADR refines authentication, capability, operation, health, and revoke behavior without collapsing those axes |
| ADR-030 access epoch and envelope lifecycle | authoritative | account authorization creates/ends access epochs; work-context erase and tombstone behavior stays owned by ADR-030 |

Existing OAuth code can continue serving its current AI-provider contract. It
must not be advertised as Extension multi-account support until an additive
account-scoped port and adapter pass this ADR's matrix.

### 3. Effective capability is an exact live intersection

For operation `O`, resource class `R`, and account `A`:

```text
effective(A, O, R) =
  global Maekon consent
  ∩ manifest declaration
  ∩ installation capability grant
  ∩ account OAuth scope
  ∩ managed policy
  ∩ OS and provider readiness
```

Every term is evaluated for the exact Extension, installation, account,
capability, operation, resource class, egress target, and current policy
version. Missing, stale, unknown, expired, partially understood, or broader than
the host can map is denial. Deny always wins.

Managed policy can remove providers, capabilities, resources, egress targets,
or versions. It cannot create product consent, add a manifest capability,
approve an installation grant, add OAuth scope, or turn a user denial into an
allow.

An external write or destructive action also requires its separate per-run or
per-action approval after the intersection succeeds. That approval is
single-purpose, short-lived, account-bound, and cannot be reused as install
approval, OAuth authorization, or a future action grant.

### 4. Approval and authorization facts stay separate

The following are independent records with independent revocation:

| Fact | What it authorizes | What it does not authorize |
|---|---|---|
| install approval | register the verified package for one installation | enablement, account access, content collection, execution |
| enablement | permit the host to consider the installation | OAuth, capability, sync, or action |
| capability grant | one bounded capability/resource/egress set for an installation and optional account | provider scope, product consent, destructive action |
| OAuth authorization | provider-issued scopes for one account | Maekon capture/OCR/full-text consent or package capability |
| per-run/action approval | one exact external action with preview and expiry | future runs, background sync, new account, broader target |

A manifest change that broadens capability, resource, account mode, provider,
runtime, or egress invalidates only the affected grants and returns readiness to
a non-ready state. Silent scope escalation and automatic reauthorization are
forbidden.

### 5. Lifecycle summary is derived from orthogonal state, not a boolean

The authoritative snapshot retains ADR-029's installation, enablement,
authentication, grant, operation, update, availability, and health axes plus
current product consent, scope delta, Keychain readiness, policy version, and
account identity verification. A summary is evaluated for one installation,
account, and required capability set; losing one capability does not revoke an
unrelated account or capability. The required product summary uses these values:

| Summary | Derivation and allowed transition |
|---|---|
| `discovered` | verified descriptor known, not installed |
| `installed` | installed but disabled; no operation allowed |
| `enabled` | enabled but one or more consent/grant/account/auth/readiness terms are incomplete |
| `authorizing` | one explicit account-bound OAuth flow is active |
| `ready` | every required live intersection term allows and no operation is active |
| `syncing` | one authorized account operation is active under a fresh decision |
| `degraded` | a bounded transient provider, rate-limit, network, or backend health failure exists; denied operations stay denied |
| `stale` | token, scope, account, grant, policy, or readiness evidence is too old or changed and must be revalidated |
| `revoked` | product consent, install grant, OAuth authority, account access, or policy authority was explicitly lost |
| `incompatible` | manifest/runtime/API/provider contract cannot be safely understood by this host |

Normal progress is `discovered -> installed -> enabled -> authorizing -> ready
-> syncing -> ready`. A transient failure can move `ready` or `syncing` to
`degraded`; proven recovery re-evaluates the full intersection before returning
to `ready`. Changed or uncertain evidence moves to `stale`, never optimistic
`ready`. Explicit authority loss moves to `revoked` from any active state and
requires a new authorization/grant as applicable. Host incompatibility always
dominates the operational summary.

The summary is for display and filtering. Consumers receive the underlying
axes, denial reasons, freshness, and remediation keys. `connected` may be shown
only as localized copy for `ready` or `syncing`; it is never a stored source of
truth.

### 6. OAuth begins with minimum scope and upgrades explicitly

Each connector declares a versioned capability-to-scope map. A first
authorization requests only the minimum scopes for the capability the user just
selected. Identity scopes must be separately justified; offline access is
requested only when bounded background operation requires refresh.

After callback, the adapter records the provider-returned normalized scope set
and compares it to requested and previously granted sets:

- missing required scope denies only the affected capability and reports
  `scope_partial`;
- an unexpected extra provider scope never expands Maekon capability;
- an omitted scope response is `scope_unverified` unless the provider contract
  supplies an authoritative introspection path;
- a capability needing more scope triggers an explicit incremental-upgrade
  preview showing the delta and purpose; and
- declining an upgrade preserves previously valid narrower capability and does
  not disconnect unrelated accounts.

An incremental flow is bound to the same `ExtensionAccountKey`, install grant,
manifest digest, requested scope delta, redirect, state, PKCE challenge, and a
short expiry. Account mismatch at callback fails the flow and stores no token.

OAuth scopes can never grant screen capture, OCR, full-text retention,
suggestion input, task creation, external write, or another Maekon product
permission unless the other intersection terms independently allow it.

### 7. OAuth endpoints and redirects are pinned and fail closed

Provider authorization metadata is host-curated or verified against a signed
connector descriptor. User-supplied authorization, token, refresh, revoke,
introspection, issuer, or redirect endpoints are not accepted in Phase 1.

The adapter enforces:

1. exact HTTPS scheme, normalized host, permitted port, issuer, and path for
   every remote endpoint;
2. a fixed redirect allowlist containing only the connector's registered
   loopback callback or approved OS application link;
3. loopback binding to `127.0.0.1`/`::1`, an exact random flow path or fixed
   registered path plus one-time state, five-minute maximum lifetime, and no
   wildcard redirect;
4. high-entropy one-time state compared in constant time and bound to account,
   install, manifest, scope request, and redirect;
5. PKCE S256 with a fresh verifier for every flow; no `plain` downgrade;
6. redirect following disabled for token, refresh, revoke, and introspection
   requests; a 3xx is a typed failure and credential bodies are never replayed;
7. strict internal-range rejection for every remote destination, including
   loopback, private, link-local, metadata, CGNAT, NAT64, mapped IPv4, multicast,
   unspecified, and resolved internal addresses;
8. resolve-and-pin or connect-time address verification so DNS rebinding cannot
   replace an approved public address; and
9. bounded response size and timeout for every authorization backend request.

The local callback is the only loopback exception and never accepts provider
response bodies. The browser may open only the validated authorization URL.
Returned links, `Location` values, provider error URLs, and remediation URLs are
never followed by the backend without a new allowlist check.

### 8. Tokens and provider bodies never cross the secret boundary

The OS Keychain is authoritative for access, refresh, and ID tokens, token-set
generation, and any provider credential material. Manifest, config, SQLite,
cursor, projection, cache, log, audit, telemetry, crash report, clipboard,
public DTO, UI state, and error text contain no token, authorization code, PKCE
verifier, state value, client secret, cookie, or raw credential handle.

Non-secret authorization metadata can contain only opaque account ID, provider
ID, normalized granted-scope names or a scope fingerprint, expiry bucket,
credential generation, issuer fingerprint, policy/grant versions, typed health,
and timestamps. Keychain enumeration metadata is owner-only and privacy
sensitive; it contains no values.

Provider response bodies, redirect targets, query strings, headers, and raw
error descriptions are never logged or placed in audit/telemetry. The adapter
reads capped bodies in memory, extracts allowlisted typed OAuth error codes,
then discards the body. Public errors expose a stable reason code, retryability,
bounded status class, and localization/remediation key only.

Current provider code that logs a provider `error_description` is not evidence
of conformance to this Extension contract. The same applies when raw
descriptions are rendered into callback HTML. The future adapter must redact or
replace both paths before use.

### 9. Refresh is account-scoped, rotation-safe, and deterministic

Refresh single-flight keys use the complete `ExtensionAccountKey` plus token-set
generation. Different accounts refresh independently; concurrent refreshes for
one account await or replay the committed result rather than returning a stale
token as proof of readiness.

A successful refresh atomically compare-and-swaps the token generation. If the
provider rotates a refresh token, the new token and access token commit
together. If it omits a refresh token, the current refresh token is retained
only when that provider contract explicitly permits omission. A losing
concurrent response cannot overwrite a newer generation.

| Case | Required result |
|---|---|
| fresh access token and complete scope | use only after a fresh capability decision |
| expiring/expired access token with valid refresh | single-flight refresh before work; no cursor advance until success |
| two refreshes for one account | one network exchange; other callers observe the committed generation or typed failure |
| refreshes for different accounts | isolated locks, tokens, health, rate limits, and results |
| rotated refresh token | atomic generation swap; old token becomes unusable and is erased |
| `invalid_grant`, provider revoke, or authenticated `401` after one bounded retry | cancel account work, mark authorization stale/revoked as the provider contract dictates, erase unusable access material, require explicit reauthorization |
| expired token without refresh token | `stale` + `reauthorization_required`; no silent browser flow |
| partial/missing scope after refresh | deny affected capabilities, keep unrelated narrower ones, require explicit upgrade |
| account switch | cancel/quarantine old account work; change UI selection only; never reuse token, cursor, cache, or decision |

Transient network/5xx/429 failures use bounded exponential backoff and provider
`Retry-After` only after validating and capping it. They produce account/install
health and actionable remediation, not global disconnect. A terminal auth
failure is never retried as a transient error.

### 10. Revoke, policy change, and account loss take effect immediately

The capability broker re-evaluates before every page, retry, projection read,
source-open, suggestion input, and external action. It also publishes a
cancellation epoch so an authority change invalidates already-issued decisions.

On product-consent withdrawal, install-grant revoke, OAuth revoke/401, account
access loss, managed-policy narrowing, or Keychain authority loss:

1. increment the account/install cancellation epoch and reject new work;
2. cancel in-flight acquisition/action at the next cancellation-safe await;
3. atomically refuse cursor advance and projection publication for the affected
   page or action;
4. clear unusable token handles and cached authorization/readiness decisions;
5. apply ADR-030 `access_revoked` and content/tombstone behavior to source data;
6. emit a metadata-only receipt with the authority and affected capability; and
7. expose typed remediation without starting authorization automatically.

An operation that already caused an irreversible provider side effect records
`outcome_unknown` and requires reconciliation; revoke cannot claim rollback.
No later retry uses the old decision or token generation.

### 11. Disconnect and uninstall have explicit cleanup ownership

Account disconnect affects exactly one `ExtensionAccountKey`:

1. disable and cancel that account's work;
2. attempt provider revoke through the pinned endpoint when supported;
3. delete local Keychain credentials even if remote revoke fails;
4. erase the account cursor, retry material, raw payload, projections, and
   caches under ADR-030;
5. mark retained minimized provenance and suppression tombstones with the
   correct lifecycle; and
6. keep the package, installation, unrelated accounts, and user-confirmed
   ADR-028 to-dos.

Remote revoke failure is a visible metadata-only cleanup receipt; the product
does not retain a credential solely to retry revoke. Reconnection creates a new
access epoch and performs explicit authorization.

Uninstall repeats this ordering for every account, then revokes installation
grants and removes package/runtime metadata only after durable cleanup receipts
exist, as required by ADR-029. UI never reports disconnected/uninstalled while
local credential or content cleanup is incomplete.

### 12. Capability broker and audit APIs expose metadata only

`maekon-core` defines an additive object-safe `ExtensionAuthorizationBrokerPort`
with operations to:

- evaluate one account/capability/operation/resource request against fresh
  authority snapshots;
- begin/cancel an explicit authorization or scope-upgrade flow;
- obtain an account authorization/readiness snapshot;
- notify revoke, policy, account, and Keychain changes;
- acquire an opaque, short-lived credential lease for an already-authorized
  network adapter; and
- disconnect an account with a durable cleanup receipt.

The lease is non-serializable, process-local, account/operation/audience-bound,
generation-bound, and cancellable. Public application DTOs never carry a token
or a reusable credential handle.

Minimum audit fields are event ID, timestamp, extension/install/opaque account
refs, bounded action and capability, decision/reason, requested/granted scope
fingerprints, manifest/grant/policy/credential generations, provider ID,
outcome, retryability, and correlation ID. Audit excludes secrets, raw scope
consent pages, provider bodies, URLs/query strings, tenant names, emails, and
remote content.

Rate limits and auth health are independently tracked per install/account and
provider operation. UI remediation can say `reauthorize`, `grant_scope`,
`retry_after`, `check_keychain`, `policy_denied`, or `account_unavailable`
without including provider body text.

### 13. UI readiness cannot lead backend truth

The backend snapshot includes all authoritative axes, effective capability
decisions, missing scope/capability deltas, credential expiry bucket, freshness,
health, rate-limit state, and remediation keys. The frontend derives no broader
state locally.

An OAuth callback alone does not make an account `ready`. Readiness requires
successful state/PKCE validation, token storage, account-subject verification,
scope comparison, current product consent, install grant, managed policy,
Keychain availability, provider readiness, and a fresh broker decision.

Optimistic UI may show `authorizing` or `checking`, never `ready` or `syncing`.
An unknown backend, auth, scope, account, or policy state is unavailable. UI
copy is i18n-driven and displays account-safe labels separately from opaque
identifiers.

## Frozen Invariants

Changing any item requires a new ADR or explicit update:

1. OAuth scope and Maekon product consent are independent and intersect at use
   time.
2. Extension/install/account identity scopes every credential, cursor, grant,
   refresh, cache, health, rate-limit, and operation.
3. Installed, enabled, authorized, granted, ready, and syncing are never one
   authoritative boolean.
4. Managed policy can narrow authority but cannot create or broaden user
   authority.
5. Secret values live only in the OS Keychain and process-local bounded leases.
6. Revoke invalidates new and in-flight work without advancing an affected
   cursor or publishing a new projection.
7. Provider bodies, redirect targets, and raw error descriptions never enter
   log, audit, telemetry, public DTO, or UI state.
8. UI readiness never precedes a fresh backend broker decision.

## Consequences

### Positive

- Multiple accounts and installations cannot share a token, cursor, refresh
  lock, capability decision, or projected context accidentally.
- Product consent remains authoritative even when a provider grants broad
  OAuth scope.
- Revoke and account switching have deterministic cancellation and cleanup.
- UI can explain partial scope and remediation without exposing credentials.

### Negative

- The current provider-only `OAuthPort` cannot be reused unchanged for
  Extensions.
- Account-scoped cancellation epochs, token generations, and atomic cleanup add
  storage and property-test work.
- Providers without stable subject identity, pinned endpoints, bounded error
  responses, or explicit revoke behavior remain unavailable.

### Neutral

- Existing standalone AI-provider OAuth remains in its current ownership
  boundary.
- Provider-specific endpoints, scope maps, and account discovery remain
  follow-up implementations that must conform to this contract.

## Alternatives Considered

**A. Treat OAuth scope as Maekon consent.** Rejected because a provider grant
cannot authorize screen/OCR/full-text collection or package capability.

**B. Reuse `provider_id` as the account key.** Rejected because same-provider
accounts and installations would share credentials, refresh locks, and cursors.

**C. Store one `connected` boolean.** Rejected because it hides install,
enablement, grant, scope, policy, health, and operation failures.

**D. Retry refresh globally and accept the last response.** Rejected because
cross-account blocking and refresh-token rotation races can overwrite valid
credentials.

**E. Follow provider redirects and sanitize afterward.** Rejected because a
307/308 can replay credential form bodies before sanitization and DNS rebinding
can change the destination.

**F. Keep a token after disconnect so remote revoke can retry.** Rejected
because local revoke must complete even when the provider is unavailable.

## Review and Implementation Gates

Before this ADR becomes `Accepted`, reviewers must approve identity derivation,
scope mapping, Keychain namespace, lifecycle precedence, endpoint policy,
cancellation epoch, refresh rotation, cleanup ordering, and metadata-only audit.
Later implementation tests must include:

1. OAuth scope/product-consent intersection and managed-policy deny precedence;
2. same provider with two accounts and two installations;
3. enable vs authorize vs grant vs ready state derivation;
4. duplicate callback, wrong/expired state, wrong account, PKCE downgrade, and
   callback cancellation;
5. redirect 301/302/307/308, disallowed host/path/port, private/metadata IP,
   IPv4-mapped IPv6, and DNS-rebinding attempts;
6. concurrent same-account refresh, different-account refresh, rotated refresh
   token, losing generation, expired access token, and missing refresh token;
7. partial/extra/missing scope and declined incremental upgrade;
8. account switch with active page, consent revoke during a page, OAuth 401,
   policy narrowing, and Keychain loss;
9. disconnect/uninstall with successful and failed remote revoke, cursor and
   projection cleanup, and confirmed to-do retention;
10. provider error-body, redirect, token, code, state, verifier, email, and raw
    ACL absence from logs/audit/telemetry/public DTOs;
11. per-account rate-limit and auth-health isolation; and
12. backend-not-ready/unknown state UI non-regression with en/ko parity.

## Known Follow-ups

1. **#8587 source runtime** — schedule pages only after an Accepted broker
   decision and cancellation-epoch binding.
2. **#8589 encrypted ledger** — persist account-scoped cursor and cleanup
   receipts without credentials.
3. **Provider connector ADRs** — define signed endpoint metadata, minimum scope
   maps, stable subject derivation, revoke semantics, and evidence per provider.
4. **OAuth adapter implementation** — add account-scoped ports without changing
   the current provider-only port's wire contract.

## Amendment 2026-07-21: review resolutions

Moved from `Proposed` to `Accepted` after a 3-loop adversarial review
(security devil's-advocate + rust-core implementability lenses). The SSRF/endpoint
pinning (§7), Keychain secret boundary (§8), refresh-rotation CAS (§9),
deny-wins intersection (§3), and revoke/uninstall ordering (§10/§11) were
confirmed sound and reuse proven code. The resolutions below are contract.

### Blocking resolution

- **§1 — `local-account-key` is a new dedicated Keychain-only installation
  secret.** The HMAC key that derives `account_id` is generated **once per
  installation** as fresh random bytes, stored **only in the OS Keychain**
  (owner-only, via the existing `KeychainRegistry`), and is **never** written to
  SQLite, config, telemetry, or any network request — in particular it is
  independent of and never derived from `device_identity.device_id` (which is a
  plaintext SQLite UUID sent to the server, and would collapse the
  non-reversibility and installation-binding guarantees). Because the key is
  per-installation, two installs of the same extension produce **different**
  `account_id`s for the same provider subject, upholding §1's boundary-inequality
  claim. Loss or rotation of `local-account-key` invalidates every derived
  `account_id`: the affected accounts must re-authenticate (their Keychain
  namespaces/cursors/grants become unreachable and are cleaned up under the §11
  order), and rotation is therefore a deliberate, user-visible re-connect event,
  never silent.

### Important resolutions

- **§12 — `ExtensionAuthorizationBrokerPort` is `#[async_trait]`.** Its
  I/O-bound operations (credential lease over Keychain via `spawn_blocking`,
  network revoke, begin/cancel authorization flow) follow the `OAuthPort`
  async-trait shape, not the deliberately-sync `ConsentManagerPort` shape.
- **§3 — the "global Maekon consent" AND-term needs a dedicated consent field.**
  The existing `ConsentPermissions` enum (screen_capture, ocr_processing, …) has
  no tier that semantically governs "ingest a third-party Extension's
  OAuth-scoped content." A **new `ConsentPermissions` field** (e.g.
  `extension_context_ingest`) is required before any `context_source` connector
  ships; until then that AND-term is treated as **absent (fail-closed)**, never
  silently satisfied by reusing an unrelated tier like `full_text_extraction`.
  Tracked under Known Follow-ups; P02 (#8587) must land it before ingesting
  content.
- **§10 — the cancellation epoch is broker-owned and distinct from ADR-030's
  `access_epoch_id`.** ADR-030's `access_epoch_id` governs content/tombstone
  lifecycle; the broker's account/install cancellation epoch is a separate
  counter owned by this ADR. §2's table wording is scoped to the broker's own
  epoch and does not share ADR-030's counter.
- **§7 item 8 — DNS-rebinding defense may require a custom connector.** Honest
  scoping note: resolve-and-pin plus connect-time address re-verification is not
  fully expressible through the reused `reqwest::Client` builder alone (reqwest
  exposes `dns_resolver` for resolve-then-pin but not the connected socket
  address for a second check without a custom `hyper_util` connector). Satisfying
  Review Gate test #5 may therefore require replacing part of the reused HTTP
  stack, not just configuring it — this is accepted as implementation cost, not a
  contradiction of the "reuse hardened outbound client" reuse claim.
- **§6 — first-connect wrong-account is a UX responsibility, not an authz gap.**
  On a brand-new connect with no existing `ExtensionAccountKey` to mismatch
  against, the callback's returned subject is authoritative and its data stays
  correctly scoped to its own account key; the OAuth account-chooser UX must
  carry the "you picked account B, not A" risk. No boundary is crossed.

### Vocabulary + cross-reference

- The canonical derived-summary vocabulary is §5's
  `discovered|installed|enabled|authorizing|ready|syncing|degraded|stale|revoked|incompatible`.
  #8586's issue-body list (`authenticated`/`synced`) is non-canonical and must
  not become the contract.
- Implementers reuse the already-shipped `task_source_refs.account_subject_ref`
  (ADR-028 / v49) as the opaque account-subject projection rather than inventing
  a third account-identity representation alongside `account_id`.
- New broker-port failure wire codes follow the ADR-019 typed-code process (the
  locked 54-entry snapshot gets an additive update); new ADR-022 id prefixes
  (capability grant, cleanup receipt, audit event) register in
  `id_generation.rs`. Next migration slot is **v50**.

## Related Docs

- `docs/architecture/ADR-008-network-resilience-patterns.md`
- `docs/architecture/ADR-021-config-consent-core-placement.md`
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md`
- `docs/architecture/ADR-029-extension-package-runtime-boundary.md`
- `docs/architecture/ADR-030-work-context-envelope-convergence.md`
- `crates/maekon-core/src/ports/oauth.rs`
- `crates/maekon-network/src/oauth/mod.rs`
- `crates/maekon-network/src/oauth/refresh_coordinator.rs`
- `crates/maekon-network/src/oauth/token_exchange.rs`
- `crates/maekon-storage/src/keychain.rs`
