[English](./ADR-028-durable-task-lifecycle-boundary.md) | [한국어](./ADR-028-durable-task-lifecycle-boundary.ko.md)

# ADR-028: Durable Task Lifecycle Boundary

**Status**: Accepted (Amended 2026-07-21 after 3-loop review — see [Amendment 2026-07-21](#amendment-2026-07-21-review-resolutions))
**Date**: 2026-07-19
**Scope**: `maekon-core` task models and ports, `maekon-storage` SQLite adapter, `src-tauri` task use cases and IPC, `maekon-web` task views
**Related**: ADR-001 (crate dependency direction), ADR-006 (IPC contract), ADR-022 (prefix+ULID IDs), ADR-026 (async storage and consent ports), ADR-027 (derived suggestion action binding)
**Issue**: #8576 (`MK-CONTEXT-01.T01`)
**Implementation status**: P01 #8577 landed on parent main in `af5826ad96` (SQLite V49, core/store, IPC, standalone panel), and `655d979ca1` mounted the panel at route `/tasks`. Source-only readback; not release, runtime, or customer-effect evidence.

---

## Context

Maekon can generate transient suggestions, including a consent-gated suggestion
derived from the current local scene. A suggestion is not a durable task. The
existing `SuggestionType` is frozen across SQLite, Serde, protobuf, and UI
surfaces, and its feedback lifecycle does not express user-confirmed work,
blockers, restart recovery, or source loss.

Adding task persistence without a separate contract would create three unsafe
ambiguities:

1. generated text could become a durable claim without an explicit human act;
2. retries or a crash during confirmation could create duplicate tasks; and
3. OCR or extension source material could outlive its consent or retention
   boundary through a task record.

This ADR defines the durable boundary before any task implementation starts. It
does not change `SuggestionType`, implement the storage schema, or enable
external task synchronization. A proposed ADR is reviewable but not in force;
issue #8577 must not implement this contract until the ADR is accepted.

## Decision

### 1. A candidate and a to-do are different claims

`TaskCandidate` is a reviewable proposal. `TodoItem` is a user-confirmed record.
Generation may create a candidate, but only an explicit human confirmation may
create a to-do. No confidence threshold, timer, retry, sync message, extension,
or automation policy may substitute for confirmation.

The models live in `maekon-core`. The additive ADR-022 prefixes are:

| Entity | Prefix | Meaning |
|---|---|---|
| `TaskCandidate` | `tcand` | Reviewable, non-authoritative proposal |
| `TodoItem` | `todo` | Human-confirmed durable task |
| transition receipt | `tmut` | Idempotent mutation receipt |

The implementation must use `generate_id` and must not mint IDs in adapters or
the WebView. Acceptance of this ADR registers these prefixes; ADR-022's ID
format and validation rules remain unchanged.

The durable shapes are deliberately small:

| Model | User/data fields beyond identity and state |
|---|---|
| `TaskCandidate` | sanitized title/body, optional proposed due time and opaque owner reference, expiry, `TaskSourceRef`, revision/timestamps |
| `TodoItem` | sanitized title/body, optional due time and opaque owner reference, unique origin candidate, optional `supersedes_todo_id`, revision/timestamps |
| blocker | directed `blocked_todo_id -> blocker_todo_id` edge between existing to-dos |

An owner reference is a local opaque identifier, not an email, display name,
token, or account subject. Proposed due/owner values remain proposals until the
human confirms them. Blockers are added only through an explicit post-confirm
mutation and cannot be inferred from generated prose.

### 2. State machines are explicit and fail closed

Candidate transitions:

```text
proposed ──confirm──> confirmed
    ├──────dismiss──> dismissed
    └──────expiry───> expired
```

`confirmed`, `dismissed`, and `expired` are terminal. A confirmed candidate has
exactly one originating to-do. A dismissed or expired candidate has none.

To-do transitions:

```text
confirmed ──start────> in_progress ──wait────> waiting
    │          │              │          └──resume──> in_progress
    │          │              ├──────────────done───> done
    │          │              └────────────cancel───> cancelled
    │          ├─────────────────────────────done───> done
    │          └──────────────────────────cancel────> cancelled
    ├───────────────────────────────────────done────> done
    ├──────────────────────────────────────wait─────> waiting
    └──────────────────────────────────────cancel───> cancelled
```

The allowed transition matrix is authoritative:

| From | Allowed destination |
|---|---|
| `confirmed` | `in_progress`, `waiting`, `done`, `cancelled` |
| `in_progress` | `waiting`, `done`, `cancelled` |
| `waiting` | `in_progress`, `done`, `cancelled` |
| `done` | none |
| `cancelled` | none |

Self-transitions, reverse transitions, and reopening are forbidden. A future
reopen flow creates a new `TodoItem` with an explicit `supersedes_todo_id`; it
does not mutate terminal history. Unknown state values remain readable for
export but are not mutable until a newer client understands them.

### 3. Confirmation and every transition are transactional and idempotent

Every command carries an opaque `idempotency_key` and `expected_revision`.
Correctness comes from SQLite transactions and compare-and-swap, not from a
process-local single-flight guard.

Confirmation is one transaction:

1. replay a matching receipt if one exists;
2. compare the candidate's state and revision;
3. update `proposed -> confirmed` with a revision increment;
4. insert one `TodoItem` whose `origin_candidate_id` is unique; and
5. insert a receipt containing only request/result metadata.

A crash commits all five effects or none. Replaying the same key and same
request returns the original result. Reusing a key with different request
content returns `idempotency_mismatch`. A different key racing after a winning
transition returns `revision_conflict` or `already_transitioned`; it never
creates a second to-do.

The same rules apply to candidate dismissal/expiry and to-do transitions.
Receipts are unique by `(entity_kind, entity_id, idempotency_key)` and include a
canonical request hash, from/to states, resulting revision, and optional
resulting entity ID. They never contain titles, bodies, OCR, or source subject
identifiers.

### 4. `TaskSourceRef` is immutable provenance, not source content

Every candidate has exactly one source reference created with the candidate.
Its identity fields never change to point at another source. Lifecycle and
outcome fields may advance monotonically.

| Field | Contract |
|---|---|
| `source_kind` | `local_current_scene`, `interruption`, `extension_context`, or an unknown forward-compatible value |
| `extension_id` | Optional stable extension type ID; never an executable path |
| `install_id` | Optional opaque installation ID; never a credential |
| `account_subject_ref` | Optional opaque account subject; never email, display name, token, or secret |
| `upstream_object_id` | Optional source-owned object ID |
| `upstream_revision` / `upstream_etag` | Optional opaque source version; neither grants authority |
| `occurred_at` | When the source event happened, if known |
| `observed_at` | When Maekon observed it |
| `dedupe_namespace` | Stable non-secret namespace scoped to the source subject |
| `content_hash` | `sha256:<hex>` of canonical sanitized candidate content; never raw OCR bytes |
| `lifecycle` | `active`, `deleted`, `access_revoked`, or `retention_expired`; monotonic away from `active` |
| `source_outcome` | Optional `pending`, `resumed`, `abandoned`, or `expired`; valid only for `interruption` |

Unknown `source_kind` and lifecycle values must round-trip through storage and
export. They are fail-closed for acquisition and mutation. A source reference
does not carry ACLs, consent tokens, access tokens, screenshots, accessibility
trees, raw OCR, app/window titles, or extension payloads.

The future `WorkContextEnvelope` owned by #8583 may populate this reference, but
it does not own task state. `TaskSourceRef` is the immutable, minimized
projection from that envelope. Tasks do not depend on #8583 implementation and
do not persist the envelope itself.

### 5. Source retries deduplicate; similar tasks do not auto-merge

Candidate creation computes:

```text
SHA256(
  "task-candidate/v1\0" || dedupe_namespace || source_kind ||
  extension_id? || install_id? || account_subject_ref? ||
  upstream_object_id? || upstream_revision? || content_hash
)
```

Canonical encoding is length-prefixed UTF-8 with an explicit marker for a
missing optional value. The resulting `dedupe_key` is globally unique in the
local store. Re-delivery of the same source revision returns the existing
candidate even if it was dismissed or expired; a retry must not resurrect it.
A new upstream revision or changed sanitized content produces a new key.

The key is an at-least-once ingestion guard, not semantic task matching. Similar
titles, embeddings, or LLM judgments never auto-merge candidates or to-dos.

### 6. Privacy and retention follow the stricter source boundary

Task persistence stores only a sanitized candidate title/body, user-confirmed
to-do fields, minimized source provenance, and transition metadata. Raw OCR,
screenshots, accessibility text, full extension payloads, and secrets are never
written to task tables or audit events.

Retention rules are fixed for v1:

| Data | Retention |
|---|---|
| proposed candidate content | until `expires_at`, which must be no later than 7 days after creation or the source's earlier expiry |
| dismissed/expired candidate content | cleared in the terminal transition transaction; metadata tombstone retained for 30 days |
| confirmed candidate content | copied into the new to-do, then cleared in the confirmation transaction; provenance tombstone retained while the to-do exists |
| active to-do (`confirmed`, `in_progress`, `waiting`) | until the user transitions or explicitly deletes it |
| terminal to-do (`done`, `cancelled`) | 90-day default, user-shortenable; hard maximum 365 days without a new ADR |
| transition receipts | while the entity exists, then at most 30 additional days for retry convergence |

When consent is revoked or source access is lost:

- new acquisition stops before task construction;
- proposed candidates from that consent/source expire with a typed reason and
  their content is cleared in the same transaction;
- a user-confirmed to-do is not silently deleted, but source lifecycle advances
  to `access_revoked` or `retention_expired`;
- install/account/upstream identifiers, revisions, etags, and dedupe namespace
  are cleared; source kind, lifecycle, occurred/observed timestamps, and the
  non-content transition history remain sufficient to explain provenance; and
- explicit full erasure deletes candidates, source refs, to-dos, blockers, and
  receipts. Retained audit/egress records contain category and outcome only, not
  entity IDs, titles, bodies, hashes, or source identifiers.

Personal-data export must include all task tables through the existing export
and masking path. Full erasure must add the task tables to the canonical
`ALL_TABLES` deletion family in child-first order. Neither path may reconstruct
or imply unavailable raw source data.

### 7. Restart reconciliation never invents user intent

Startup opens the store only after migrations and then runs one idempotent task
reconciliation transaction. It uses
`effective_now = max(current_utc, persisted_last_reconciled_at)`, so a wall-clock
rollback cannot un-expire a candidate. The persisted floor advances only after a
successful reconciliation.

| Observed state after restart | Required result |
|---|---|
| proposed candidate with `expires_at <= effective_now` | transition to `expired`, clear content, write deterministic reconciliation receipt |
| proposed candidate still within TTL | no state change |
| confirmed candidate + one originating to-do | no state change; replay receipts remain valid |
| confirmed candidate without a to-do | integrity error; quarantine/read-only failure, never synthesize a to-do |
| more than one to-do for a candidate | schema integrity failure; fail closed |
| receipt committed | replay the stored result |
| receipt absent because transaction did not commit | caller may retry using the original key |
| unknown state or source lifecycle | preserve for export; refuse mutation |

For interruption sources, intent is explicit:

- `pending`: no resume or abandon decision exists and the candidate is still
  proposed;
- `resumed`: the source has a recorded `resumed_at`; an unconfirmed restore
  candidate is dismissed as `source_resumed`;
- `abandoned`: the user explicitly chose not to restore, or explicitly chose a
  different task; elapsed time alone never means abandonment; and
- `expired`: the candidate TTL elapsed without a decision.

A confirmed to-do is unchanged by a later interruption outcome. Reconciliation
may update minimized provenance, but it may not infer confirmation, completion,
or cancellation.

### 8. Hexagonal ports and IPC keep authority inside the application

The pure state transition functions and models belong to `maekon-core`.
`maekon-core` exposes narrow, object-safe async `TaskCommandPort` and
`TaskQueryPort` traits. `maekon-storage` implements them; it does not decide
transitions. `src-tauri` application use cases construct source refs, enforce
live consent, call the ports, and emit events only after commit. `maekon-web`
receives sanitized DTOs and never calls SQLite or supplies source provenance.

The minimum IPC surface is:

- list/get candidates and to-dos;
- confirm a candidate with `candidate_id`, `expected_revision`, and
  `idempotency_key`;
- dismiss a candidate with the same concurrency fields and a bounded reason;
- transition a to-do with `todo_id`, target state, expected revision, and key;
- delete a to-do explicitly; and
- export/erase through the existing privacy commands.

IPC returns typed results such as `confirmed`, `dismissed`, `expired`,
`revision_conflict`, `already_transitioned`, `idempotency_mismatch`,
`consent_required`, and `source_unavailable`. The client cannot provide a
source reference, `origin_candidate_id`, receipt result, or raw context in a
mutation command. Blocker edits follow the same revision/idempotency contract
and reject self-links and cycles.

### 9. SQLite migration is additive and rollback is restore-based

The implementation introduces five tables:

| Table | Required invariant |
|---|---|
| `task_candidates` | state CHECK, revision, expiry, sanitized nullable content, unique dedupe key |
| `task_source_refs` | exactly one row per candidate, forward-compatible source values, no raw payload |
| `todo_items` | state CHECK, revision, `origin_candidate_id UNIQUE NOT NULL` |
| `todo_blockers` | unique directed edge, no self-edge; application rejects cycles |
| `task_transition_receipts` | unique entity/key tuple, canonical request hash, metadata-only result |

Foreign keys cascade child-first, but the explicit erasure list remains
mandatory. Indexes cover candidate state/expiry, to-do state/update time,
source object/revision lookup, and receipt replay. Free-text fields are not
added to FTS or sync tables in v1.

At baseline commit `11050b0`, `CURRENT_VERSION` is 48, so v49 is the expected
slot. The implementation issue must re-resolve the next unallocated version
against current `main`; this ADR does not reserve a stale number. The migration
must be additive, run inside the existing per-version savepoint after the
pre-migration backup, and include fresh-database, v48-upgrade, injected-failure,
and future-schema tests. There is no backfill from suggestions or interruptions.

An older binary encountering the newer schema continues to fail closed. Rollback
means restoring the pre-migration backup or deploying a compatible binary; it
does not decrement `user_version`, drop task tables in place, or reinterpret a
to-do as a suggestion. Command registration may be feature-disabled without
deleting data.

## Frozen Invariants

Changing any item requires a new ADR or an explicit update to this one:

1. no durable `TodoItem` without explicit human confirmation;
2. one candidate creates at most one originating to-do;
3. `SuggestionType` remains the frozen 10-variant contract;
4. raw scene/extension context never enters task persistence or task audit;
5. within the dedupe-tombstone retention window, source retries cannot
   resurrect dismissed or expired candidates (see [Amendment 2026-07-21](#amendment-2026-07-21-review-resolutions) B3/P1 for the window scoping);
6. unknown states and source values fail closed for mutation but remain
   exportable;
7. restart reconciliation never invents confirmation or task completion; and
8. task persistence is local-only until a separate sync ADR is accepted.

## Consequences

### Positive

- Human confirmation is a structural boundary, not a UI convention.
- Transactional receipts make retries and crash recovery deterministic.
- A minimized source reference supports future extensions without retaining raw
  source material.
- Privacy export, revocation, retention, and erasure have testable table-level
  obligations before implementation begins.

### Negative

- Five tables and two narrow ports add more implementation work than extending
  the existing suggestion row.
- Terminal records require scheduled retention reconciliation.
- Confirmed tasks may intentionally outlive source access, so the UI must explain
  minimized provenance rather than promise that the original source is openable.

### Neutral

- Suggestion feedback and task transitions remain separate histories.
- External task synchronization and extension ingestion can reuse the source
  projection but require their own contracts.

## Alternatives Considered

**A. Add task states to `Suggestion`.** Rejected because it changes the frozen
wire/storage enum, merges ephemeral relevance feedback with user-authored work,
and risks network-generated durable claims.

**B. Persist a to-do immediately and ask for confirmation later.** Rejected
because it makes generation equivalent to a durable claim and creates ambiguous
erasure and retry behavior.

**C. Use process-local single-flight without receipts.** Rejected because it
does not survive restart, multi-window calls, or a commit-response crash.

**D. Store the complete OCR/extension envelope for provenance.** Rejected
because provenance does not justify retaining source content beyond its consent
and retention boundary.

**E. Infer interruption abandonment after a timer.** Rejected because elapsed
time is not evidence of user intent; only expiry may be time-derived.

## Review and Implementation Gates

Before changing this ADR to `Accepted`, reviewers must confirm the state
machines, forbidden transitions, retention periods, source-loss behavior, and
rollback contract. Before #8577 can close, tests must cover:

1. every allowed and forbidden transition;
2. duplicate and conflicting idempotency keys;
3. crash before/after commit and restart replay;
4. dedupe across identical delivery and changed source revision;
5. wall-clock rollback and expiry reconciliation;
6. consent revoke, source delete/access loss, export, and full erasure;
7. fresh DB, baseline upgrade, savepoint rollback, and future-schema refusal;
8. unknown forward-compatible source/state values;
9. crate-boundary and IPC authority checks; and
10. sanitizer floor — candidate title/body is sanitized at `PiiFilterLevel::Standard`
    minimum regardless of the live capture filter level, and candidate
    construction fails closed when no sanitizer is available (see
    [Amendment 2026-07-21](#amendment-2026-07-21-review-resolutions) P3).

## Implementation Status and Known Follow-ups

1. **#8577 P01 implemented and mounted** — parent main `af5826ad96` merged the
   core model/port, SQLite V49, store/export/retention, Tauri IPC, frontend
   hook, and a standalone panel; `655d979ca1` then rendered that panel from
   `pages/tasks/TasksPage.tsx` and registered it at `/tasks` in
   `routes/route-tree.ts`. This is a source readback of parent `main`, not
   evidence of a public export, exact-build pass, real usage, or production
   readiness.
2. **#8583 extension envelope** — map a minimized `WorkContextEnvelope`
   projection into `TaskSourceRef`; do not persist the envelope in task tables.
3. **Task sync ADR** — define conflict, tombstone, and cross-device confirmation
   semantics before any task table enters sync descriptors.
4. **Reopen semantics** — if required, specify `supersedes_todo_id` UX and
   history presentation without mutating terminal rows.

## Amendment 2026-07-21: review resolutions

This ADR moved from `Proposed` to `Accepted` after a 3-loop adversarial review
(independent devil's-advocate, rust-core implementability, and privacy/retention
lenses). The review surfaced four blocking clusters and several important gaps;
all are resolved below and these resolutions are contract, superseding any
earlier prose they contradict. The state machines (§2), the confirmation
transaction shape (§3), `effective_now` un-expiry protection (§7), and the
additive-migration test bar (§9) were confirmed sound and are unchanged.

### Blocking resolutions

- **B1 — result-code disambiguation (§3).** A losing compare-and-swap returns
  `already_transitioned` when the entity is already in the requested target
  state (the desired end-state was reached by another key), and
  `revision_conflict` when the current revision differs and the target state has
  **not** been reached. A `confirm` on a candidate already in a terminal state
  other than `confirmed` (i.e. `dismissed`/`expired`) returns
  `already_transitioned` carrying that terminal state, never `revision_conflict`.
  Callers treat `already_transitioned` as an idempotent no-op success and
  `revision_conflict` as refetch-then-retry. Review gate #2 asserts these exact
  codes.
- **B2 — dedupe for anchor-less source kinds (§5).** For source kinds without an
  upstream object/revision anchor (notably `local_current_scene`),
  `dedupe_namespace` MUST incorporate a per-capture occurrence discriminator
  (the capture/frame id or a monotonic capture sequence) so that identical
  sanitized content produced from two distinct captures yields two distinct
  `dedupe_key`s. Content-hash-only dedupe therefore guards at-least-once
  ingestion of a **single** capture occurrence and never suppresses a
  legitimately re-observed scene. Anchored kinds (`interruption`,
  `extension_context`) use their upstream object/revision as the occurrence
  anchor. This preserves §5's stated intent ("an at-least-once ingestion guard,
  not semantic task matching").
- **B3 / P1 — FK cascade, receipt retention, and dedupe-tombstone permanence
  (§6, §9).** ~~This codebase does not enable the SQLite `foreign_keys` PRAGMA
  (verified: no connection sets it in `sqlite/mod.rs`), so the five tables' `ON
  DELETE` declarations are documentation-only and do not cascade at the engine
  level.~~
  **CORRECTED 2026-08-01 (#9735): `foreign_keys` is ON.** The original check
  looked for an explicit `PRAGMA` write and found none — but the value is a
  *compile-time default*: `libsqlite3-sys` builds the bundled amalgamation with
  `-DSQLITE_DEFAULT_FOREIGN_KEYS=1`, and this workspace pins
  `rusqlite = { features = ["bundled-sqlcipher", ...] }`. Measured on a live
  connection: `PRAGMA foreign_keys = 1`, and `pragma_compile_options` contains
  `DEFAULT_FOREIGN_KEYS`. So the `ON DELETE` declarations **are** engine-enforced.
  The application-enforced child-first deletes described below remain correct
  and harmless (they are redundant with the engine, not in conflict with it) —
  but code must not *reason* from the false premise. It already had cost:
  `activity_segments` inserts carrying a not-yet-checkpointed `regime_id` were
  rejected outright by the enforced FK, on the authority of a comment that said
  they could not be. All content-clearing (§3), tombstone expiry (§6), and erasure
  (§6) deletions are application-enforced as explicit child-first `DELETE`
  statements inside the governing transaction, matching the existing
  `ALL_TABLES` pattern in `sqlite/maintenance/retention.rs`. Because deletion is
  application-ordered rather than engine-cascaded, `task_transition_receipts`
  and dedupe tombstones keep **independent** retention lifetimes from their
  logical parent — deleting a parent row does not implicitly delete them, which
  is what makes the receipt "30 additional days" clause implementable. A
  dismissed/expired candidate clears its content in the terminal transaction but
  retains a minimal dedupe tombstone (`dedupe_key`, terminal outcome,
  timestamps — no title/body/source text) for the tombstone window. Within that
  window a retry returns the terminal candidate and does not resurrect it;
  **after** the window elapses and the tombstone is purged, a later redelivery is
  a new proposal. Frozen Invariant #5 is scoped to the tombstone window (30 days
  for v1), not unbounded time.

### Important resolutions

- **P2 — `task_source_refs` deletion timing (§6).** A `task_source_refs` row is
  deleted together with its owning candidate row: at the candidate's tombstone
  expiry for dismissed/expired candidates, and when a confirmed candidate's
  provenance tombstone is released (i.e. when its originating to-do is deleted).
  It is never retained past its owning candidate/to-do.
- **P3 — sanitizer floor (§6, gate #10).** Candidate title/body sanitization uses
  the `PiiSanitizer` port at a fixed minimum floor of `PiiFilterLevel::Standard`,
  independent of the live capture filter level (which the user may set to `Off`),
  mirroring the export path's `EXPORT_SANITIZE_LEVEL`. Candidate construction
  **fails closed** (no candidate is created) if a sanitizer is unavailable.
- **P4 — export masking columns (§6).** The durable content columns (candidate
  and to-do `title` and `body`, plus any dismiss `reason`) are added to the
  export `MASKED_COLUMNS` allowlist as a defense-in-depth second layer over
  write-time sanitization. `body` and `reason` are not yet in that list and #8577
  must add them.
- **P5 / audit sink (§6).** This ADR writes no task `title`/`body`/hash/source
  identifier to `audit_log`, `session_audit_log`, or `egress_ledger`. Task
  lifecycle produces only in-process events emitted after commit (§8). §6's
  "category and outcome only" sentence is a prohibition on task data reaching
  those sinks, not a description of an existing task→audit path; routing task
  events to any audit sink requires a future ADR specifying category/outcome-only
  columns.
- **I1 — manual to-dos are out of scope for v1.** Every v1 to-do originates from a
  candidate with a `TaskSourceRef`. A purely user-authored "Add Task" with no
  source is out of scope; if added later it requires a `manual`/`user_authored`
  `source_kind` with its own dedupe/provenance rules via a follow-up. #8577 does
  not synthesize a fake source for manual entry.
- **I2 — reopen mechanism is deferred, not settled (§2).** §2's sentence about a
  "future reopen flow" is illustrative only. Reopen's origin-candidate linkage
  and `source_kind` (which must not violate the one-candidate→one-to-do invariant
  or `origin_candidate_id UNIQUE NOT NULL`) are unspecified here and owned by
  Known Follow-up #4.
- **I3 — blocker-edge identity (§8, §9).** Blocker edges use
  `entity_kind = todo_blocker` and are keyed for idempotency by the directed
  `(blocked_todo_id, blocker_todo_id)` pair; `expected_revision` is checked and
  bumped against the **blocked** to-do (the edge's owning entity), whose id is the
  receipt `entity_id`. Blocker edges carry no ULID prefix of their own.
- **I4 — re-delivery after confirmation (§5/§6).** A retry that matches a source
  revision whose candidate was already confirmed returns
  `already_transitioned` with the confirmed state, not a content-null "ghost"
  candidate; the caller is told the work already became a to-do.
- **I5 — `todo_blockers` on parent deletion (§6/§9).** Because deletion is
  application-ordered (see B3), deleting a to-do explicitly removes its incident
  blocker edges in the same transaction; a dependent to-do that loses a blocker
  edge this way is surfaced to the UI (no silent unblock without signal).
- **I6 — `consent_required` / `source_unavailable` triggers (§8).**
  `consent_required` is returned when live consent for the source's category is
  absent at mutation time; `source_unavailable` when the source lifecycle is not
  `active` (deleted/access_revoked/retention_expired) and the operation needs
  live source acquisition. Neither applies to operations on already-confirmed
  to-dos, which are source-independent.
- **I7 — same-key races resolve at the single-writer funnel (§3).** The
  idempotency contract relies on the crate's single-writer SQLite funnel
  (`with_conn`/`with_conn_mut`, `parking_lot`) so a same-key race serializes at
  the connection layer. "Not a process-local single-flight guard" means
  correctness does not depend on an in-memory dedup map, not that concurrent
  writers race at the engine.
- **C1 — id-generation prefix registry (impl checklist).** #8577 must register
  `tcand`/`todo`/`tmut` in the `USED_PREFIXES` list in
  `crates/maekon-core/src/id_generation.rs` (ADR-022's public prefix registry is
  already pre-populated for these, effective on this ADR's acceptance).

Minor wording items raised in review (ASCII diagram vs. authoritative matrix,
`lifecycle` lateral moves, `source_outcome` CHECK vs. prose, `upstream_etag`
exclusion rationale, dedupe byte-layout precision, "newer compatible binary"
rollback wording, restore-based-rollback trade-off) are left to the #8577
implementation to encode precisely against the authoritative matrix and CHECK
constraints; none change the contract.

## Related Docs

- `docs/architecture/ADR-022-client-id-generation-ulid.md`
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md`
- `docs/architecture/ADR-027-suggestion-action-binding.md`
- `crates/maekon-storage/src/migration/mod.rs`
- `crates/maekon-storage/src/sqlite/maintenance/retention.rs`
- `crates/maekon-storage/src/sqlite/maintenance/export.rs`
