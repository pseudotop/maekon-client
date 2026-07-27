[English](./ADR-030-work-context-envelope-convergence.md) | [한국어](./ADR-030-work-context-envelope-convergence.ko.md)

# ADR-030: Work Context Envelope and Convergence

**Status**: Accepted (amended 2026-07-21)
**Date**: 2026-07-19
**Scope**: `maekon-core` work-context models and ports, future encrypted ledger adapter, timeline/task/memory evidence projections
**Related**: ADR-022 (prefix+ULID IDs), ADR-023 (memory evidence), ADR-026 (consent port), ADR-028 (durable task provenance), ADR-029 (Extension boundary), ADR-031 (extension authorization/account boundary)
**Issue**: #8583 (`MK-EXT-01.S02`)

---

## Context

The existing `maekon_core::models::event::ContextEvent` is a foreground desktop
app/window observation. It feeds capture decisions and is already serialized and
stored as part of the PC event contract. It cannot represent an external
provider's account identity, remote object version, access state, deletion, or
cursor replay without changing that contract's meaning.

External work systems are also at-least-once sources in practice. A page can be
delivered twice after a crash, updates can arrive out of order, an object can be
deleted before an older update is replayed, and access can disappear without a
content revision. Treating each delivery as a new event would duplicate timeline
items and could resurrect data after revoke or retention expiry.

This ADR defines a separate `WorkContextEnvelope`, acquisition port, minimized
provenance, and deterministic convergence rules before a connector or SQLite
schema is implemented. It does not redefine the durable task lifecycle, build a
connector, generate memory claims, or add a new PC `Event` variant.

## Decision

### 1. External work context is a separate source family

`WorkContextEnvelope` lives in a new `maekon-core` work-context module. Its wire
identifier is `maekon.work_context_envelope.v1`. Local envelope IDs use the
additive ADR-022 prefix `wctx`; the prefix takes effect only when this ADR is
Accepted.

The following remain frozen:

- `ContextEvent` keeps its current app/window/activity fields and PC-capture
  responsibility;
- `Event` gains no `WorkContext` variant;
- existing PC event wire/storage, batch upload, and capture triggers do not
  deserialize or act on an envelope; and
- external and PC context meet only in a query/projection view with explicit
  source-family discrimination.

An envelope records one observed revision of one remote object. It is evidence
of ingestion, not proof that the provider delivered exactly once or that access
still exists.

### 2. The envelope is metadata and provenance, never a raw payload

The required model is:

| Group | Fields and rules |
|---|---|
| local identity | `envelope_id`, `schema_version`, `access_epoch_id` |
| source identity | `extension_id`, `install_id`, opaque `account_subject_ref`, `remote_type`, `remote_id` |
| version | `revision_model`, optional `remote_revision`, optional `etag`, optional normalized source order, `content_hash` |
| classification | bounded `kind`, data classification, retention class |
| time | `occurred_at`, `source_updated_at`, `observed_at`, `ingested_at` |
| relations | optional opaque thread, parent, project, and actor refs |
| authority evidence | minimized access snapshot and consent snapshot; neither is live authority |
| provenance | ingest-run ID, prior accepted envelope ID, source cursor/page digest, projection/raw-blob refs |
| lifecycle | `active`, `deleted`, `access_revoked`, or `retention_expired` |

Source refs are opaque identifiers, not names, emails, URLs with tokens, or raw
ACL entries. A relation ref contains only a bounded ref kind, opaque source ID,
and optional fingerprint. The envelope contains no message body, document text,
meeting notes, attachment, HTML, provider payload, OAuth token, ACL member list,
or search tokens.

`account_subject_ref` is the privacy-minimized representation of the issue's
`account_id` requirement. It is stable only within the declared extension and
installation boundary; display names and email addresses are never account IDs.

The bounded kind taxonomy is:

- `message`
- `meeting`
- `document`
- `issue`
- `decision`
- `task`
- `unknown`

An unknown source kind must round-trip through inventory and export but is not
eligible for searchable projection, suggestion input, task generation, or graph
projection until explicitly mapped.

Data classification is `public`, `internal`, `confidential`, `restricted`, or
`unknown`; `unknown` is enforced as `restricted`. Retention class is a policy
identifier resolved by the local policy engine, not a provider-supplied TTL.

### 3. Time fields have distinct meanings

| Field | Meaning | Ordering authority |
|---|---|---|
| `occurred_at` | when the business event happened; optional/provider-authored | never version authority |
| `source_updated_at` | provider-reported last modification time; optional | only comparable when that connector contract proves monotonic semantics |
| `observed_at` | local time when the connector observed the remote record | diagnostics, not remote conflict resolution |
| `ingested_at` | local time when the envelope transaction committed | local audit/retention start, not remote version authority |

The local ledger may stamp writes with the existing HLC to make local operations
monotonic across clock rollback. That HLC does not pretend to be the remote
revision. Wall-clock timestamps never break an opaque remote-version tie unless
the provider-specific contract explicitly makes them authoritative.

### 4. Identity and revision dedupe are canonical and account-scoped

The source object identity is the canonical length-prefixed encoding of:

```text
extension_id || install_id || account_subject_ref || remote_type || remote_id
```

Its non-reversible local key is:

```text
source_object_key = HMAC-SHA256(local-dedupe-key, "work-context-object/v1\0" || identity)
```

The HMAC prevents a retained key or public export from becoming a dictionary of
provider/account IDs. Different accounts or installations cannot collide even
when a provider reuses the same remote ID.

Each record also gets a revision fingerprint:

```text
SHA256(
  "work-context-revision/v1\0" || revision_model ||
  remote_revision? || etag? || source_updated_at? ||
  content_hash || lifecycle
)
```

Canonical encoding has length prefixes and explicit missing-value markers. The
local uniqueness key is `(source_object_key, access_epoch_id,
revision_fingerprint)`. Replaying it returns the original ingest result and does
not create another envelope or projection.

`content_hash` covers canonical sanitized content, not ciphertext, raw ACL, or
provider JSON bytes. A source cursor is an account/install-scoped checkpoint;
it is never object identity, version authority, or a cross-account dedupe key.

The product promises idempotent local upsert under at-least-once acquisition. It
does not claim exactly-once remote delivery.

### 5. Every connector declares revision quality

`ContextSourceDescriptor` declares one revision model:

| Model | Contract |
|---|---|
| `monotonic` | connector provides a normalized order value only when provider semantics guarantee total order for one object |
| `opaque` | revision/etag supports equality only; changed values are incomparable |
| `content_hash_only` | provider has no trustworthy version token; active updates dedupe by sanitized content hash, and production support requires a proven delete strategy |

Adapters may normalize a provider version but may not claim monotonic ordering
from lexical strings or timestamps without provider evidence. A connector that
cannot deterministically handle delete/access loss reports capability
`unavailable` and cannot be advertised as supported.

### 6. Merge and lifecycle rules are deterministic and fail closed

Lifecycle is monotonic within one `access_epoch_id`:

```text
active ──source delete────> deleted
   ├────access loss───────> access_revoked
   └────retention expiry──> retention_expired
```

Terminal states clear content availability. Re-authentication/re-grant creates a
new access epoch; it never changes the old tombstone back to active.

For one source object and access epoch, merge proceeds in this order:

1. an identical revision fingerprint is `duplicate` and replays the stored
   result;
2. access/consent denial immediately yields `access_revoked`, erases content,
   and dominates content revisions;
3. a higher comparable active revision replaces the prior active projection;
4. a lower comparable revision is `stale` and cannot replace or resurrect;
5. the same comparable revision with a different content hash is
   `revision_conflict`;
6. a delete/retention tombstone suppresses every equal or lower comparable
   active revision;
7. changed opaque revisions are `incomparable`; their metadata is retained under
   a deterministic conflict ID computed from sorted revision fingerprints, but
   no winner is exposed to search, suggestions, tasks, or graph projection; and
8. an active record after `deleted` is accepted only when the provider contract
   explicitly supports undelete and supplies a strictly higher comparable
   revision. Access revoke always requires a new epoch.

Delete-before-update is therefore safe: the tombstone blocks the replayed older
update. If a delete and update are incomparable, visibility follows the safer
tombstone while the object is conflict-quarantined. Delivery order never decides
which content becomes searchable.

Local `retention_expired` suppresses the same or older revision. A genuinely
newer revision may be ingested only under current consent/access and a policy
that still permits acquisition.

### 7. Raw, projection, envelope, and tombstone planes are separate

| Plane | Content | Encryption and retention |
|---|---|---|
| raw payload blob | provider response/body needed for bounded parsing or reprocessing | memory-only by default; if explicitly consented, AEAD-encrypted with account/install-scoped key, default TTL 24h, hard maximum 7d |
| searchable projection | sanitized title/body/summary and bounded refs needed for timeline/search | encrypted at rest; default TTL 30d and never beyond user/source retention |
| envelope | minimized identity, version, classification, provenance, hashes, lifecycle | encrypted where identifiers remain; retained with projection or confirmed reference obligations |
| suppression tombstone | HMAC source key, access epoch, version/order fingerprint, lifecycle, deletion time | content-free; retained for `max(provider replay horizon, projection retention, 90d)`, hard maximum 365d |

A connector must declare a bounded replay horizon. If safe suppression requires
more than 365 days or the horizon is unknowable, the connector remains
unsupported until a different convergence design is accepted.

Raw and projection keys, TTLs, export paths, and erasure jobs are independent.
Deleting a raw blob does not delete its minimized envelope. Deleting/revoking a
source immediately removes raw and projection availability; a content-free
tombstone may remain only to prevent replay resurrection.

### 8. Access and consent snapshots are evidence, not authority

`AccessSnapshot` contains access decision (`allowed`, `denied`, `unknown`), a
visibility class, a non-reversible scope fingerprint, evaluation time, and
provider-policy version. It contains no raw ACL member list. If a connector must
temporarily process a raw ACL, it stays in the encrypted raw plane under the raw
TTL and is never serialized in a public DTO.

`ConsentSnapshot` contains the product permission ID/version, decision, and
evaluation time; it contains no consent token or mutable grant object.

Every timeline query, search, suggestion input, projection read, and source-open
request re-evaluates live product consent, account access, source lifecycle, and
retention. A stored snapshot cannot authorize a later read. `unknown` access
denies. Revoke/access loss cancels in-flight page work, prevents cursor advance,
clears content in the same durable operation, and makes cached DTOs unusable.

### 9. Ports separate acquisition from persistence and public delivery

`maekon-core` defines two narrow object-safe async ports:

- `ContextSourcePort` describes source/revision/delete capabilities and pulls a
  bounded `ContextSourcePage` for one opaque account subject and cursor; and
- `WorkContextStorePort` performs idempotent ingest, convergence, projection,
  tombstone, cursor, export, and erasure operations.

A page includes a page/checkpoint digest, records, optional next cursor, and
`has_more`. A record carries minimized source/version/access metadata plus an
in-memory or sealed raw-payload handle. The handle is not a public DTO and the
adapter never writes storage directly.

The application use case performs live consent/access checks and commits the
page atomically:

1. validate source descriptor and account epoch;
2. canonicalize identities and revisions;
3. apply each deterministic merge result;
4. write/replace projection and schedule raw expiry as permitted;
5. write tombstones/conflict receipts;
6. advance the account/install cursor; and
7. emit events only after commit.

If a crash occurs before commit, no cursor advances. If commit succeeds but the
provider response/ack is lost, the page is replayed and local uniqueness returns
the prior results. Process-local single-flight is only an optimization.

### 10. ADR-028 `TaskSourceRef` is a minimized immutable projection

| `TaskSourceRef` field | Envelope mapping |
|---|---|
| `source_kind` | `extension_context` |
| `extension_id` / `install_id` / `account_subject_ref` | same opaque source identity fields |
| `upstream_object_id` | remote object ID while access/retention permits; otherwise cleared per ADR-028 |
| `upstream_revision` / `upstream_etag` | accepted source version while permitted |
| `occurred_at` / `observed_at` | same timestamp meanings |
| `dedupe_namespace` | task-specific namespace derived from the HMAC source-object key, never the raw identity tuple |
| `content_hash` | accepted sanitized projection hash |
| `lifecycle` | mapped `active`, `deleted`, `access_revoked`, or `retention_expired` |
| `source_outcome` | absent; reserved for interruption sources |

Confirmation copies this minimized ref; it never stores an envelope, raw blob,
ACL snapshot, or projection as task provenance. Later source loss does not erase
a user-confirmed to-do, but source-open fails closed and UI shows the lifecycle
reason. The task contract remains owned by ADR-028.

### 11. Timeline and memory use evidence refs, not copied source content

The query layer may merge PC events and external projections into a
`TimelineEvidenceItem` view ordered by explicit display-time policy. Every item
retains `source_family = pc_event | work_context`; no serialization conversion or
inheritance occurs. External envelopes never trigger screen capture.

Memory graph and suggestion pipelines receive a `WorkContextEvidenceRef`
containing envelope ID, accepted revision fingerprint, classification,
lifecycle, and content hash. They may dereference a live sanitized projection
through the consent/access gate. They do not copy raw provider content into
claims, nodes, prompts, or audit. Memory graph generation policy remains owned
by #8087.

### 12. Export, revoke, erasure, and source-open are explicit

Personal-data export includes envelopes, projections, conflicts, cursors, and
tombstone explanations through the existing masking path. A still-retained raw
blob is exported only through a reauthenticated, explicitly selected encrypted
attachment path; it never appears in a normal public JSON DTO. Credentials and
raw ACL member lists are never exported.

Consent revoke or account/source access loss:

- stops acquisition and cursor advance;
- clears raw/projection content and account/remote identifiers not needed for
  bounded suppression;
- retains only the HMAC-keyed content-free tombstone for its replay horizon; and
- invalidates timeline/search/suggestion/task-source-open caches.

Full local erasure first disables the connector and clears credentials/cursors,
then deletes every envelope plane including suppression tombstones because no
active acquisition path remains to resurrect them. Future relay or cross-device
propagation requires a separate erasure-convergence ADR.

A confirmed to-do can remain after source erasure under ADR-028, but “open
source” returns `source_unavailable`; it never follows a stale URL, refreshes a
credential, or reconstructs content from a hash.

### 13. Forward compatibility is readable and mutation-safe

Unknown kind/classification/lifecycle/revision values round-trip for inventory
and export. They are not eligible for projection, suggestion, task creation, or
source-open. Missing security/source identity fields fail validation.

Minimum typed results include:

- `ingested`
- `duplicate`
- `stale_revision`
- `revision_conflict`
- `revision_incomparable`
- `source_deleted`
- `access_revoked`
- `consent_required`
- `retention_expired`
- `projection_unavailable`
- `source_unavailable`
- `cursor_replayed`

Public DTOs expose opaque account/source refs and bounded access/lifecycle
states. They never expose account secrets, raw ACL members, raw payload handles,
provider tokens, or unmasked remote URLs.

## Frozen Invariants

Changing any item requires a new ADR or explicit update:

1. PC `Event`/`ContextEvent` wire, storage, and capture semantics remain separate.
2. Local ingestion is idempotent under at-least-once delivery; no exactly-once
   remote claim.
3. Account/install identity participates in every object, cursor, grant, and
   dedupe boundary.
4. Access revoke dominates content revisions and requires a new access epoch.
5. Incomparable/conflicting revisions never feed projection, suggestions, tasks,
   or memory graph.
6. Raw payload, projection, envelope, and tombstone have separate encryption,
   TTL, export, and erasure behavior.
7. Stored access/consent snapshots never authorize a live read.
8. Tasks and memory retain minimized evidence refs, not copied raw source data.

## Consequences

### Positive

- Cursor replay and out-of-order delivery are deterministic before connector
  implementation.
- Existing PC event/capture contracts stay stable.
- Revoke/delete/retention cannot be undone by a stale page replay.
- Task, timeline, and memory consumers share provenance without copying source
  payloads.

### Negative

- Opaque provider revisions can quarantine content instead of choosing a likely
  winner.
- Four storage planes and account-scoped cursors require more lifecycle jobs and
  property tests.
- Providers without a bounded replay/delete model cannot be advertised as
  supported.

### Neutral

- HLC is reused for local monotonic audit/order but does not replace provider
  revision semantics.
- SQLite schema and connector-specific version normalization remain follow-up
  implementation decisions within this contract.

## Alternatives Considered

**A. Extend `ContextEvent`.** Rejected because external objects are not desktop
foreground observations and would change event wire, storage, and capture
triggers.

**B. Use cursor or delivery time as object identity/version.** Rejected because
cursors replay and delivery order varies after restart.

**C. Use last-write-arrival for opaque conflicts.** Rejected because different
delivery orders expose different content and can resurrect deleted data.

**D. Keep complete provider JSON and ACL for provenance.** Rejected because
provenance does not justify long-lived source content or access lists.

**E. Copy external text into timeline/tasks/memory.** Rejected because copied
content escapes source access, consent, and retention revocation.

**F. Delete all tombstones immediately on revoke.** Rejected because an
at-least-once page replay can recreate the erased projection before the cursor
or connector is fully disabled.

## Amendment 2026-07-21 (Acceptance Review)

A three-loop adversarial review (privacy lens + devil's-advocate lens) raised
three blocking defects, six important gaps, and four minor clarifications
against the `Proposed` text. Each is resolved below. These resolutions bind
implementers as firmly as the original Decision sections. Status moves to
`Accepted`.

### B1 — The envelope plane gets a hard retention bound (§7)

§7 gave every other plane a numeric ceiling (raw 24h default / 7d hard maximum,
projection 30d, tombstone 90d floor / 365d hard maximum) but described the
envelope plane only as "retained with projection or confirmed reference
obligations". An envelope with neither a live projection nor a confirmed ADR-028
reference — an object listed once, never confirmed, whose projection has since
expired — therefore had no expiry at all, defeating the retention argument the
four-plane split exists to make.

Resolution: an envelope MUST NOT outlive

    max(projection retention for that object, confirmed ADR-028 reference lifetime)

and when neither obligation exists it inherits the projection default (30d) as
its ceiling, with a hard maximum of 365d matching the tombstone plane. Expiring
an envelope is not a remote deletion: the object transitions to
`retention_expired` and leaves only the content-free suppression tombstone,
which keeps its own independent bound. Retention-job property tests MUST include
an envelope with no projection and no confirmed reference.

### B2 — `local-dedupe-key` custody is specified, not assumed (§4)

§4 relies on `source_object_key = HMAC-SHA256(local-dedupe-key, ...)` being
non-reversible so that a retained key or public export cannot become a dictionary
of provider/account IDs — but never said where that key lives or how it is
generated. ADR-031 had to resolve the identical question for its
`local-account-key`, and both keys currently appear only in ADR prose: neither
`crates/maekon-storage/src/keychain.rs` nor `device_identity.rs` contains a
precedent an implementer could copy.

Resolution, mirroring ADR-031 §1:

- `local-dedupe-key` is generated once per installation from a CSPRNG, stored
  Keychain-only through `KeychainRegistry`, and never written to SQLite, config
  files, logs, telemetry, or any export.
- It MUST NOT be derived from `device_identity.device_id` or any other value that
  is already plaintext-persisted or transmitted to the server. Doing so collapses
  the non-reversibility guarantee that is the entire purpose of the HMAC.
- It is a distinct secret from ADR-031's `local-account-key`. Where a single
  per-installation root secret is preferred, the two MAY be domain-separated
  subkeys — `HKDF-SHA256(root, info = "maekon.dedupe-key.v1")` and
  `HKDF-SHA256(root, info = "maekon.account-key.v1")` — but the same key material
  MUST NOT be used under two names.
- Losing the key is recoverable, not fatal: dedupe degrades to "everything looks
  new" and the ledger re-converges by revision. Key loss is never a reason to
  fall back to an unkeyed hash.

### B3 — "Full local erasure" and ADR-031 uninstall are different actions (§12)

§12 said full local erasure "first disables the connector ... then deletes every
envelope plane including suppression tombstones", while ADR-031 §11 (Accepted)
specifies that uninstall repeats the account-disconnect ordering, whose step 5
*retains* minimized provenance and suppression tombstones. Read literally, the two
documents give opposite instructions for removing an extension.

Resolution: these are distinct user actions, and ADR-031 §11 governs uninstall.

| Action | Suppression tombstones | Rationale |
|---|---|---|
| account disconnect / extension uninstall (ADR-031 §11) | **retained** for their replay horizon | the account may be reconnected, and a replayed stale page must still be suppressed |
| full local erasure (this ADR §12) | **deleted** | a separate, explicitly user-initiated "delete all local data" path that also destroys credentials, cursors, and the connector, after which no acquisition path can resurrect anything |

§12's "disables the connector" describes the erasure path's own internal ordering,
not uninstall. Full local erasure MUST NOT be reachable as a side effect of
uninstalling an extension or disconnecting an account.

### I1 — Plane keys are HKDF subkeys of the existing root key (§7)

§7 required raw-plane blobs to be AEAD-encrypted with an "account/install-scoped
key" without grounding that key in anything. The only key infrastructure that
exists today is the single whole-database `EncryptionKey([u8; 32])` in
`crates/maekon-storage/src/encryption/mod.rs` (AES-256-GCM); the passphrase-based
`derive_key` in `file_transport.rs` is an export-file mechanism, not a plane key.

Resolution: plane keys are HKDF-SHA256 subkeys of the existing `EncryptionKey`.
No new key-management system is introduced.

    raw-plane key = HKDF-SHA256(ikm  = EncryptionKey,
                                salt = install_id,
                                info = "maekon.raw-plane.v1" || account_subject_ref)

Destroying the stored salt/context record for one account crypto-shreds that
account's raw plane without rewriting the database, and this is the sanctioned
mechanism for satisfying §12's "clears raw content" on revoke. Implementation
belongs to #8589.

### I2 — The store mints `access_epoch_id`; it is not the broker's counter (§6, §9)

§6 says a re-grant "creates a new access epoch", but no listed `WorkContextStorePort`
operation mints one, and ADR-031's amendment disclaims sharing its counter from its
own side only — a reader of ADR-030 alone could not tell that two counters exist.

Resolution: `access_epoch_id` is owned and minted by the work-context store, not by
the capability broker and not by a connector. `WorkContextStorePort` gains an
explicit `begin_access_epoch(account)` operation, called on first authorization and
on every re-grant; the broker only signals revoke/re-grant, it never supplies an
epoch value. Every record in one ingested page MUST carry the same epoch, and a page
whose epoch no longer matches the account's current epoch is discarded without
advancing the cursor. ADR-031's broker cancellation epoch is a separate counter with
no ordering relationship to this one.

### I3 — `account_subject_ref` is ADR-031's `account_id` (§2)

§2 described `account_subject_ref` only as "the privacy-minimized representation of
the issue's account_id requirement", with no formula, and ADR-030's Related Docs
omitted ADR-031 entirely — even though ADR-031 explicitly warns against "inventing a
third account-identity representation".

Resolution: `account_subject_ref` **is** ADR-031's `account_id` — the same string,
carried through, never independently re-derived and never a second hashing of it.
ADR-031 is added to Related Docs below.

### I4 — Cursor advance is compare-and-swap (§9)

§9's seven-step page commit is correct for a single invocation but did not prevent
two overlapping ingestions for one account (a restart racing an in-flight fetch, or a
scheduler re-firing) from each reading cursor C0 and each writing its own successor,
with the later write clobbering or regressing the earlier one. Content-level dedupe
prevents data loss but not wasted refetch or two loops resetting each other.

Resolution, mirroring ADR-028's `expected_revision`: the cursor write is a
compare-and-swap against the cursor value read at the start of that page. A mismatch
abandons the page — nothing is committed and the cursor is not advanced — and the
next scheduled run re-reads the current cursor. Implementations MAY additionally
serialize ingestion per account, but the CAS is mandatory regardless.

### I5 — Clock rollback cannot extend a plane TTL (§3, §7)

§3 mentioned HLC stamping as an optional local nicety, but the merge algorithm never
referenced it and none of the four plane TTLs had anti-rollback protection. Since
those TTLs are the entire privacy argument for the four-plane design, a rolled-back
system clock could keep a raw blob "not yet expired" indefinitely.

Resolution, mirroring ADR-028 §7: all four planes evaluate expiry against

    effective_now = max(current_utc, persisted_last_ingested_at)

so a backward clock step can only expire data earlier, never later. `ingested_at`
remains the local retention anchor (§3); remote timestamps never participate in TTL
evaluation.

### I6 — Absence from a listing is not deletion evidence (§5)

For a `content_hash_only` connector the naive delete strategy is "the object stopped
appearing in the listing", which is unsound under at-least-once paginated delivery: a
dropped page, a reorder, or a transient provider error makes a live object look
absent.

Resolution: listing absence MUST NOT be treated as deletion evidence, for any
revision-quality class. A `deleted` lifecycle transition requires an explicit
provider delete signal (event, webhook, audit entry, or an object-scoped fetch
returning a definitive not-found). Absent such a signal the object stays `active`
until local retention expires it as `retention_expired`, which is the honest
statement of what the client actually knows.

### Minor clarifications

- **M1 (§6 rule 7)** — conflict-quarantine metadata is one record per
  `(source_object_key, access_epoch_id)`, not one per delivery, and is bounded by
  the envelope plane ceiling fixed in B1.
- **M2 (§4, ADR-022)** — the `wctx` prefix is **not** currently present in
  `USED_PREFIXES` in `crates/maekon-core/src/id_generation.rs` (verified 2026-07-21;
  ADR-028's `tcand`/`tmut`/`todo` are). Registering it is part of #8587, not a
  pre-existing fact.
- **M3 (§4)** — two installed extensions, or two accounts, may surface the same
  real-world remote object as separate work-context records. This is intentional and
  follows ADR-028 §5's no-auto-merge principle: the client never asserts that two
  provider objects are the same thing.
- **M4 (§4, §12)** — `content_hash` remains unsalted, inherited from ADR-028. This
  is acceptable inside the local scope, but a `content_hash` MUST NOT be exported or
  transmitted in any form that would let a third party correlate installations. When
  #8589 lands the four planes, `PERSONAL_DATA_EXPORT_TABLES` and `MASKED_COLUMNS`
  must be extended in the same PR — a new table that is absent from both silently
  breaks GDPR export and masking.

## Review and Implementation Gates

Before this ADR becomes `Accepted`, reviewers must approve identity encoding,
revision models, conflict quarantine, lifecycle dominance, retention bounds,
TaskSourceRef mapping, and full-erasure behavior. Later implementation property
tests must include:

1. duplicate page and duplicate item;
2. stale and higher comparable revisions;
3. equal revision with different content;
4. changed opaque revision/incomparable conflict;
5. delete-before-update and update-before-delete in both delivery orders;
6. access revoke during a page and re-grant with a new epoch;
7. multi-account same remote ID and cursor separation;
8. crash before commit, crash after commit/before ack, and restart replay;
9. raw/projection TTL independence and clock rollback;
10. source delete, retention expiry, export, revoke, and full erasure;
11. unknown forward-compatible values and public DTO secret/ACL exclusion; and
12. PC Event/capture wire non-regression.

## Known Follow-ups

1. **#8584 permission/OAuth contract** — define account credential and dynamic
   grant reconciliation used by the access epoch.
2. **#8587 source runtime** — implement page scheduling only after Wave-0
   contracts are accepted.
3. **#8589 encrypted ledger** — implement the four planes, query projection, and
   retention jobs without adding task lifecycle or memory generation policy.
4. **#8087 memory graph policy** — decide if and how a live evidence ref may
   become a memory claim.
5. **Relay/cross-device ADR** — required before envelopes or their erasure
   tombstones leave the local device.

## Related Docs

- `docs/architecture/ADR-022-client-id-generation-ulid.md`
- `docs/architecture/ADR-023-local-symbolic-memory-graph.md`
- `docs/architecture/ADR-028-durable-task-lifecycle-boundary.md`
- `docs/architecture/ADR-029-extension-package-runtime-boundary.md`
- `docs/architecture/ADR-031-extension-authorization-account-boundary.md`
- `crates/maekon-core/src/models/event.rs`
- `crates/maekon-core/src/models/sync.rs`
- `crates/maekon-storage/src/sync_retention_tombstone.rs`
