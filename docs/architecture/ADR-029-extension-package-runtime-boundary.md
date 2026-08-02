[English](./ADR-029-extension-package-runtime-boundary.md) | [한국어](./ADR-029-extension-package-runtime-boundary.ko.md)

# ADR-029: Extension Package and Runtime Boundary

**Status**: Accepted (Amended 2026-07-21 after 3-loop review — see [Amendment 2026-07-21](#amendment-2026-07-21-review-resolutions))
**Date**: 2026-07-19
**Scope**: `maekon-core` extension contracts, `maekon-automation` trust reuse, `src-tauri` composition root, extension-facing UI/API
**Related**: ADR-001 (crate direction), ADR-002 (execution boundary), ADR-019 (typed errors), ADR-026 (consent port), ADR-028 (durable task provenance)
**Issue**: #8585 (`MK-EXT-01.S01`)
**Implementation status**: Registry P01 #8586 landed on parent main in `75d6a1e9af` (SQLite V50, core/store, lifecycle IPC, standalone panel), and Skill Pack activation #8588 landed in `4b80bf4bdf` (SQLite V52, activation IPC). The panel is still unmounted and Skill Pack has no frontend surface; connectors, a marketplace, and third-party runtimes remain unimplemented. Source-only readback; not release, runtime, or customer-effect evidence.

---

## Context

Maekon has several adjacent assets but no single installable Extension contract:

- `AutomationTemplatePackageManifest` defines publisher, signature, install and
  execution approval, evidence, rollback, dry-run, and egress trust for reusable
  automation templates;
- `PermissionProfileV2` defines filesystem, network, Unix-socket, and resource
  limits;
- managed policy clamps user configuration and cannot replace consent;
- `FileSkillLoader` reads local Markdown instructions but is not a registry,
  installer, or stable public API;
- Provider Surface Catalog owns AI-provider readiness, not installable packages;
  and
- the read-only MCP work makes Maekon a context provider to another process,
  which is the opposite direction from loading an Extension.

Generalizing any one of those assets into an Extension runtime would duplicate
or weaken the other trust boundaries. Loading arbitrary native code would also
turn the desktop process and its local data into a third-party execution host
before package trust, revocation, and compatibility are proven.

This ADR defines the product vocabulary, package taxonomy, runtime disposition,
lifecycle truth, and threat boundary. It does not implement a registry,
connector, marketplace, relay, or executable plugin loader. `Proposed` means the
contract is reviewable but not yet in force.

## Decision

### 1. Product terms have one responsibility each

| Term | Responsibility | Not this |
|---|---|---|
| **Extension** | versioned install/update/uninstall unit with a manifest, trust identity, and one or more declared contributions | arbitrary executable or marketplace listing |
| **Context Source Connector** | read-only adapter that acquires external work context and produces the bounded source contract | action executor, Skill Pack, or raw-payload archive |
| **Skill Pack** | verified instruction/data bundle with declared capability dependencies | native code, consent grant, or direct tool authority |
| **Action Adapter** | future adapter that writes to an external system only after a separate action contract and per-action human approval | Phase-1 contribution or implicit side effect of context sync |
| **ONESHIM Relay** | optional future execution location for webhook continuity and organization policy | required Maekon backend, Extension type, or silent fallback |

An Extension is packaging and lifecycle. A contribution is a capability-bearing
thing inside that package. A runtime is the mechanism, if any, that evaluates a
contribution. These concepts are never represented by one overloaded `type` or
`connected` flag.

### 2. The manifest is a distinct, versioned core contract

The public wire identifier is `maekon.extension_manifest.v1`. The model belongs
to `maekon-core`; adapters may validate artifacts but may not invent trust or
capability fields.

The required shape is:

| Group | Required fields |
|---|---|
| identity | `extension_id`, semantic `version`, immutable `publisher_id`, `package_digest` |
| source trust | bounded `source_kind`, signature state/key ID or `app_bundle` trust, signed manifest digest |
| compatibility | `manifest_schema`, inclusive `host_api_min`, exclusive `host_api_max` |
| runtime | `runtime_kind`, `execution_location`, declared entry-point identifier with no filesystem path in public DTOs |
| contributions | stable contribution ID, bounded kind, contribution API version, requested capabilities |
| policy | optional `permission_profile_id`, external-egress declarations, data classification, retention class |
| lifecycle | update channel, rollback window, minimum allowed version, uninstall cleanup declaration |

`extension_id` is reverse-DNS style and stable across versions. An installation
gets a separate local opaque `install_id`. Account identity is never part of the
package identity; it is represented by an opaque account subject under that
installation. Manifests contain no OAuth secret, account token, raw ACL member
list, cursor, payload, user path, or executable command line.

Phase 1 accepts exactly one contribution per Extension, of kind
`context_source`. The reserved taxonomy is:

| Contribution kind | Code/data trust | Phase-1 disposition |
|---|---|---|
| `context_source` | adapter code must be first-party and compiled into the signed app; remote payload is always untrusted data | supported contract, read-only only |
| `skill_pack` | signed/curated data and instructions; never grants a capability and never becomes source content | unavailable pending capability-resolver ADR |
| `action_adapter` | executable external-write boundary | unavailable pending separate action/HITL ADR |
| unknown future value | unknown | preserve for inventory/export; unavailable for enable or operation |

Bundling multiple contributions is deferred. It can couple unrelated grants and
create a confused-deputy path, so it requires a later ADR.

### 3. Phase 1 supports only the built-in read-only runtime

Runtime disposition is normative:

| Runtime kind | Disposition | Reason |
|---|---|---|
| `first_party_builtin` | **supported in Phase 1**, only for read-only `context_source` | code ships inside the app trust/signing boundary; manifest describes it but does not load it |
| `declarative` | unavailable | requires a bounded schema, interpreter, resource limits, and golden compatibility tests |
| `mcp` | unavailable | consuming or installing an MCP server is not the existing read-only provider direction |
| `subprocess` | unavailable | needs a dedicated protocol, binary provenance, OS sandbox, kill/timeout, and audit contract |
| `wasm` | unavailable | needs engine supply-chain review, host-call allowlist, fuel/memory, and deterministic ABI rules |
| arbitrary native binary | rejected | uncontrolled executable supply chain |
| dynamic library (`dylib`/DLL) | rejected | runs inside the trusted process and bypasses process isolation |
| in-process Node or Rust plugin | rejected | same-process code execution is outside the product trust boundary |

Package-source disposition is equally narrow:

| Source kind | Phase-1 disposition |
|---|---|
| `app_bundle` | supported for a first-party built-in whose manifest digest is bound to the signed app release |
| `local_file` | unavailable; a local path is not publisher or signature trust |
| `curated_registry` | unavailable until signed distribution/update and key-rotation contracts exist |
| `public_marketplace` | unavailable; no listing, download, rating, install, or support claim exists |
| unknown | inventory/export only; unavailable |

“Supported runtime” does not mean a provider is shipped or proven. A particular
connector is supportable only after its own authentication, sync, restart,
revoke, delete, and rate-limit evidence passes. Until then UI and API return a
typed unavailable reason.

### 4. Existing E74 assets are reused without changing ownership

| Existing asset | Disposition | Extension rule |
|---|---|---|
| `AutomationTemplatePackageManifest` / #7441 | **reuse semantics; extend via a separate schema** | reuse publisher/source/signature, install-vs-execution approval, evidence, egress, dry-run, and rollback meanings; do not deserialize it as an Extension |
| `PermissionProfileV2` / #7437 | **reuse** | manifest may request a profile/capabilities; the live resolver remains authoritative and deny precedence cannot be weakened |
| managed policy / #7438 and #4832 | **reuse** | it may narrow runtime, source, capability, provider, version, or egress; it never creates user consent or OAuth scope |
| record-to-template / #7442 | **defer** | Extension work does not reimplement authoring or promote observed content into a Skill Pack |
| `FileSkillLoader` | **defer** | internal Markdown discovery is not registry/install trust and no existing file is auto-migrated |
| Provider Surface Catalog | **reject generalization** | remains the owner of AI-provider readiness; Extension Registry is a new bounded catalog |
| read-only MCP provider / #7440 and #7919 | **keep separate** | Maekon serving sanitized context is not an Extension runtime and cannot install MCP servers |
| sandbox worker | **defer reuse** | a future subprocess runtime may reuse OS-isolation principles, but the current worker is not a plugin loader |

`automation.template_package_trust.v1` remains readable and validated exactly as
today. There is no automatic manifest migration, wrapper install, or registry
discovery. If common Rust value objects are extracted later, both wire formats
must remain byte/field compatible and be protected by golden tests. Existing
automation templates never gain Extension capabilities through conversion.

### 5. Lifecycle and readiness are orthogonal state axes

The registry/API/UI reports these axes separately:

| Axis | Minimum values |
|---|---|
| availability | `available`, `unavailable(reason)` |
| installation | `bundled`, `not_installed`, `installing`, `installed`, `install_failed`, `uninstalling`, `uninstalled` |
| enablement | `disabled`, `enabled` |
| account authentication | `not_required`, `unauthenticated`, `authenticating`, `authenticated`, `revoked`, `auth_error` |
| capability grant | `not_requested`, `pending`, `granted`, `denied`, `revoked`, `expired` per account/capability |
| operation | `idle`, `running`, `blocked(reason)`, `failed(reason)` with an operation kind such as `sync` or `query` |
| update | `current`, `update_available`, `staged`, `activating`, `update_failed`, `rollback_available` |
| health | `unknown`, `healthy`, `degraded(reason)`, `unhealthy(reason)` |

Install permission, enablement, provider authentication, product consent,
capability grant, and operation approval are distinct facts. In particular:

- installed is not enabled;
- enabled is not authenticated;
- OAuth scope is not Maekon product consent;
- authentication is not a capability grant;
- a read grant is not an external-write approval; and
- managed allowance is not user approval.

A UI summary such as “connected” may be derived only when every required axis is
healthy and must expand to the axes above. It may never hide an unsupported,
revoked, stale, or partially configured state.

### 6. Effective capability is an intersection evaluated at use time

For account-scoped operation `O`:

```text
effective(O) =
  product consent
  ∩ manifest request
  ∩ installation grant
  ∩ provider/account scope
  ∩ managed policy
  ∩ runtime readiness
  ∩ current source health
```

Every term must allow the exact operation, resource class, account subject, and
egress target. Missing, stale, unknown, or broader-than-understood values deny.
The check runs before each sync page, retry, projection read, and future action;
an install-time check is insufficient.

The package cannot self-grant. Manifest capability additions, account changes,
publisher changes, signature-key changes, runtime changes, or a broadened egress
set invalidate the affected grant and require fresh review. Managed policy may
deny or further narrow the intersection but may not turn a user-denied term into
allow.

### 7. Standalone local operation is the success path

`execution_location` has three reserved values:

| Value | Meaning |
|---|---|
| `local` | operation runs on the user's Maekon device and does not require ONESHIM |
| `relay` | future operation runs through an explicitly configured ONESHIM Relay; unavailable in Phase 1 |
| `either` | future package supports both, but the active location is explicit; no silent local-to-relay failover |

Phase 1 accepts only `local`. A relay outage cannot block a supported standalone
connector. A managed organization may deny a location but cannot silently move
data to relay. Relay authentication, retention, webhook continuity, tenant
isolation, and organization authorization require a separate ADR and evidence.

### 8. Update, rollback, and uninstall preserve trust and erase data

Updates are staged before activation. The registry verifies schema/API
compatibility, package digest, trusted publisher/signature continuity, minimum
allowed version, runtime disposition, and capability diff. Activation swaps the
version atomically. A failure retains the last verified version and reports
`update_failed`; it does not run a partially staged package.

Rollback is explicit and can select only a retained, previously verified
artifact that is not below the managed/security minimum. Rollback does not
restore revoked grants, credentials, provider scope, consent, or erased data.
Any capability difference is re-evaluated at activation time.

Uninstall is ordered and fail closed:

1. disable new operations and cancel/timeout in-flight work;
2. revoke local capability grants;
3. delete account-scoped credentials from the OS Keychain;
4. erase cursors, raw payload cache, searchable projection content, and retry
   material under the connector retention contract;
5. mark source provenance `deleted`, `access_revoked`, or
   `retention_expired` as appropriate; and
6. remove package/runtime metadata after cleanup receipts are durable.

User-confirmed to-dos are not silently deleted. ADR-028 keeps only minimized
source provenance after source loss. Uninstall failure remains visible and
retryable; the product never reports `uninstalled` while credential or content
cleanup is incomplete.

### 9. The threat model is normative and bounded

| Threat | Required mitigation |
|---|---|
| supply-chain substitution | app-bundle trust for built-ins; digest/signature/publisher/source/version verification for future artifacts; staged activation and audit |
| downgrade/rollback attack | managed minimum version, verified retained artifact only, no grant restoration |
| indirect prompt injection | every external payload is untrusted data; never system/developer instruction, Skill Pack, capability declaration, or tool authority |
| credential theft | OS Keychain, account/install-scoped namespace, opaque handles; no secret in manifest/config/log/telemetry/public DTO |
| confused deputy | exact account/resource/operation capability intersection at call time; no package self-grant or cross-account cursor reuse |
| stale permission or revoke race | live checks on every page/retry, cancellation, cursor quarantine, fail-closed projection reads |
| cross-account confusion | identity includes extension/install/account subject; cursor, cache, provenance, and grants are account-scoped |
| untrusted runtime escape | Phase 1 has no loaded runtime; future subprocess/WASM requires a separate sandbox/ABI ADR |
| malicious update broadening | capability/publisher/runtime/egress diff invalidates grants; old version remains until explicit activation |
| relay data expansion | relay unavailable in Phase 1; no silent execution-location fallback |

External content may be summarized or projected only through the later envelope,
ledger, consent, and content-guard contracts. Signing a package does not make
its remote content trusted instructions.

### 10. Compatibility and unavailability fail closed

The host accepts a manifest only when its schema major is understood and
`host_api_min <= current_host_api < host_api_max`. An empty or inverted range,
unknown runtime/contribution/capability, missing security field, invalid digest
or signature, publisher mismatch, or unsupported location returns a typed
unavailable result and never attempts best-effort execution.

Minimum reason codes include:

- `manifest_schema_unsupported`
- `host_api_incompatible`
- `runtime_unsupported`
- `contribution_unsupported`
- `source_unsupported`
- `marketplace_unavailable`
- `publisher_untrusted`
- `signature_unverified`
- `managed_policy_denied`
- `consent_required`
- `authentication_required`
- `capability_grant_required`
- `source_unhealthy`
- `cleanup_incomplete`

Unknown non-security display metadata may round-trip for forward compatibility.
Unknown security, lifecycle, or capability semantics remain inventory/export
data only. UI and API use the same canonical resolver and never label an
unavailable package as installed, connected, or supported.

### 11. Public export contains the contract, not sensitive evidence

This ADR, the future manifest schema, bounded reason codes, lifecycle semantics,
and high-level threat categories are public-exportable. They let contributors
verify claims and fail-closed behavior.

Parent-only material includes internal strategy comparisons, vendor selection
notes, red-team payload corpora, exploit reproduction detail, secret/key
locations, unpublished provider identifiers, and operational incident evidence.
Public docs may state the mitigation and test obligation but do not copy those
artifacts. No public surface may claim marketplace, third-party runtime, relay,
or provider support from approval of this ADR.

## Frozen Invariants

Changing any item requires a new ADR or explicit update:

1. Phase 1 runtime is first-party, built-in, local, and read-only
   `context_source` only.
2. No arbitrary native binary, dynamic library, or in-process Node/Rust plugin.
3. Install, enable, authenticate, grant, operation, update, and uninstall remain
   separate state facts.
4. Effective capability is the live intersection; managed policy never replaces
   user consent.
5. External payload is untrusted data and never becomes Skill Pack instruction.
6. Existing automation template, Provider Surface Catalog, and MCP provider
   contracts keep their current ownership and wire meaning.
7. Standalone local success does not require ONESHIM.
8. Unsupported runtimes and contributions are reported as unavailable, never
   simulated or silently degraded.

## Consequences

### Positive

- The first connector proof has a small, auditable runtime surface.
- Existing trust and permission assets are reused without conflating their wire
  contracts.
- UI/API readiness can be honest about partial setup and unsupported runtimes.
- Uninstall, revoke, and source-loss obligations exist before payload storage is
  implemented.

### Negative

- Phase 1 cannot install third-party connectors or reuse arbitrary community
  plugins.
- Separate lifecycle axes and capability diffing require more registry/UI work
  than a single enabled flag.
- Future declarative, subprocess, WASM, Skill Pack, Action Adapter, and relay
  support each require additional design and evidence.

### Neutral

- Built-in connectors are represented as Extensions for lifecycle honesty even
  though their code is delivered with the app.
- Existing automation templates remain outside the Extension Registry until a
  separate bridge is approved.

## Alternatives Considered

**A. Generalize `AutomationTemplatePackageManifest` in place.** Rejected because
automation execution trust is not connector identity/auth/sync lifecycle, and a
wire change would destabilize an existing contract.

**B. Treat `FileSkillLoader` directories as installed Extensions.** Rejected
because local file discovery has no publisher, signature, compatibility,
permission, update, or uninstall guarantees.

**C. Load native, Node, or Rust plugins in process.** Rejected because a single
extension gains the desktop process's data and authority and bypasses process
isolation.

**D. Use MCP as the universal plugin runtime.** Rejected because the existing MCP
direction is a read-only Maekon provider, while installing/consuming MCP servers
adds a distinct executable and credential boundary.

**E. Require ONESHIM Relay for connectors.** Rejected because it breaks the
standalone product invariant and silently expands egress/availability scope.

**F. Report one `connected` state.** Rejected because it hides whether install,
auth, grant, consent, health, or operation readiness is actually missing.

## Review and Implementation Gates

Before changing this ADR to `Accepted`, reviewers must approve the E74 reuse
table, runtime disposition, effective-capability formula, lifecycle axes,
uninstall order, and public/private evidence split. Later implementation must
test:

1. unknown schema/runtime/contribution/capability fail-closed paths;
2. install vs enable vs auth vs grant state truth;
3. every capability-intersection term and revoke race;
4. manifest compatibility/digest/signature/publisher/version checks;
5. update capability diff, atomic activation, failed update, and rollback floor;
6. multi-account cursor/grant/cache separation;
7. uninstall cleanup success, partial failure, restart, and retry;
8. untrusted-content non-promotion into instructions or capabilities;
9. typed UI/API unavailability parity; and
10. public-export exclusion of parent-only threat evidence.

## Implementation Status and Known Follow-ups

1. **#8583 envelope contract** — define external source identity, revisions,
   tombstones, and minimized task provenance.
2. **#8584 permission/OAuth contract** — define account-scoped credentials,
   dynamic grants, and provider scope reconciliation.
3. **#8586 registry P01 implemented, not mounted** — parent main `75d6a1e9af`
   merged the lifecycle axes, SQLite V50, IPC, a frontend hook, and a standalone
   `ExtensionRegistryPanel` with component tests. That panel appears in no route
   entry and in no non-test source file, so extension lifecycle has no reachable
   UI. `4b80bf4bdf` added Skill Pack catalog/activation and the capability
   resolver (SQLite V52, `activate_skill_pack` and `clear_skill_pack_activation`
   IPC, regression fix `26aa185c82`) with no frontend surface at all. App/route
   composition and actual connector/runtime work remain separate follow-ups.
4. **Subprocess/WASM ADR** — required before any non-built-in runtime is enabled.
5. **Action Adapter ADR** — required before any external write contribution.
6. **Relay ADR** — required before `relay` or `either` becomes available.

## Amendment 2026-07-21: review resolutions

Moved from `Proposed` to `Accepted` after a 3-loop adversarial review
(devil's-advocate + rust-core implementability lenses). The contract's core
(effective-capability intersection §6, standalone-local-success §7, threat table
§9, uninstall ordering §8, frozen invariants) was confirmed sound; the
resolutions below are contract and supersede any earlier prose they refine.

### Blocking resolutions

- **B1 — `bundled` is a provenance sub-state, orthogonal to installation (§5).**
  The `installation` axis and code provenance are **separate facts**, not
  mutually-exclusive values of one enum. A Phase-1 `first_party_builtin`
  connector is always `bundled` (its code ships with the app) AND carries an
  independent installation state: it starts `not_installed` (present but not
  enabled/configured by the user) and transitions `installing -> installed` on
  an explicit user enable, then follows the normal `uninstalling -> uninstalled`
  path on removal. "Bundled" therefore never blocks reaching `installed`; the
  §8 uninstall order and §5 per-account axis apply to a bundled built-in exactly
  as to any other source kind. The registry must model provenance and
  installation state as two columns, never collapse them.
- **B2 — add an 11th review gate: no secret in logs/telemetry/DTOs during normal
  operation.** §9's "credential theft" mitigation ("no secret in
  manifest/config/log/telemetry/public DTO") gains a dedicated test obligation
  beyond the uninstall-time Keychain deletion (gate 7): tests must assert that a
  live install/authorize/sync cycle emits no OAuth token, refresh token,
  client-secret, or Keychain value into `tracing` output, telemetry events, or
  any public/exported DTO. This gate is required before Accepted-in-practice for
  any connector that holds a credential.

### Important resolutions

- **I1 — publisher change is a hard reject, not a soft revoke.** `publisher_id`
  is immutable identity (§2); §10 already fail-closes on `publisher_untrusted`
  at compatibility check. §6's "publisher changes" clause is defense-in-depth
  against a re-attestation/substitution TOCTOU and MUST behave as a hard reject
  (invalidate + block), never a "soft revoke, offer re-review" UX. Signature-key
  rotation (an anticipated event for `curated_registry`) is the only
  re-review-eligible change in that bullet; publisher identity change is a
  substitution signal.
- **I2 — add `execution_location_unsupported` reason code.** The §10 minimum
  reason-code list gains a first-class code for a package whose
  `execution_location` is not `local` (Phase-1 rejects `relay`/`either`); it must
  not be folded into `source_unsupported`/`runtime_unsupported`.
- **I3 — update/uninstall interleaving.** (a) An uninstall requested while the
  `update` axis is `staged`/`activating` aborts the in-flight activation before
  proceeding with the §8 order (no half-activated version survives uninstall).
  (b) A managed policy raising the minimum-allowed version above an already
  `installed` version blocks operation on the next per-operation check via
  `managed_policy_denied` (§6 runs before each sync page/retry); the install is
  not silently force-updated.
- **I4 — `health` axis reason codes.** `degraded(reason)`/`unhealthy(reason)`
  enumerate at minimum `rate_limited`, `upstream_error`, `timeout`,
  `auth_expired`, and a forward-compatible unknown — matching `availability`'s
  taxonomy rigor since both feed the §6 intersection and the §5 UI-honesty rule.
- **I5 — extension reason codes are a distinct namespace, not `CoreError` wire
  codes.** The ~14 registry reason codes are a registry-scoped string namespace;
  they do NOT expand the ADR-019 wire-locked `CoreError` 54-code snapshot. Adding
  one needs no ADR-019 process. (The header `Related` line already
  lists ADR-019; the bottom Related Docs list is aligned to match.)

### Vocabulary reconciliation (P01 / #8586)

P01 (#8586) implements the **8 raw lifecycle axes of §5 only** — it is
self-contained and needs neither ADR-031's OAuth/account broker nor ADR-030's
context envelope (P01 excludes SaaS OAuth/sync and action execution). The
account-authentication axis reports `not_required` for accounts-not-required
built-ins. Any single human-facing readiness label must use ADR-031 §5's
canonical derived-summary vocabulary
(`discovered|installed|enabled|authorizing|ready|syncing|degraded|stale|revoked|incompatible`),
NOT the ad-hoc list in #8586's issue body (`authenticated`/`synced` are not
canonical). The implementation defines a **new `ExtensionManifest` model in
`maekon-core`**, not an extension of the frozen
`automation.template_package_trust.v1` wire format (Alternative A), and registers
its opaque `install_id` prefix in `id_generation.rs`. Next migration slot is
**v50**.

Minor items (package_digest vs signed-manifest-digest relationship, rollback-window
semantics, manifest no-secret enforcement mechanism) are deferred to the first
`curated_registry` source kind, which Phase 1 does not ship.

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `docs/architecture/ADR-002-os-gui-interaction-boundary.md`
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md`
- `docs/architecture/ADR-028-durable-task-lifecycle-boundary.md`
- `crates/maekon-automation/src/template_package.rs`
- `crates/maekon-core/src/config/sections/privacy.rs`
- `crates/maekon-core/src/config/managed.rs`
- `crates/maekon-core/src/ports/mcp_readonly.rs`
