# Cross-Device Sync — Conflict Resolution Strategy

## Overview

MAEKON uses an explicit-consent pull-merge-push cycle for cross-device
synchronization. The `SyncTransport` trait (`maekon-core`) defines the
transport boundary, while `ChangeMerger` in `maekon-network` implements
record-level merging.

Sync is disabled by default. A device must have user consent, a configured sync
transport, and the required pairing or authentication material before payloads
leave the local process.

## Conflict Resolution Rules

### 1. Push Conflict (HTTP 409)

When a push is rejected with 409 Conflict:

1. `SyncEngine` re-pulls the latest state from the peer.
2. `ChangeMerger` applies the merge (see rules below).
3. `SyncEngine` retries the push with the merged result.

A maximum of 3 retry attempts is made before surfacing the error to the user.

### 2. Record-Level Merging

- **Last-write-wins**: Records are compared by HLC (Hybrid Logical Clock)
  timestamp.
- The record with the higher HLC value takes precedence.
- HLC ensures causality across devices — a local wall-clock skew cannot
  override a causally later write from another device.
- Equal HLC values are treated as a deterministic tie that must not duplicate or
  resurrect records.

### 3. Row-Level Erasure Convergence

GDPR Article 17 erasure needs two paths:

- A **device-wide DeletionEvent** for peers that are online or still on older
  sync implementations. The event is a fast convergence signal and is bounded
  to the erasure epoch.
- A **row-level tombstone stream** for peers that were offline. Tombstones keep
  only the content-free skeleton needed for convergence: table name, row id,
  origin device, HLC, and deletion time. They do not retain deleted user
  content.

When a peer receives a tombstone, it hard-deletes the matching row and suppresses
incoming rows whose HLC is less than or equal to the tombstone HLC. A later row
with a higher HLC may supersede the tombstone only after the user has explicitly
re-granted the relevant consent and a new write occurs.

This replaces the older "deletion events always win forever" simplification.
Deletion must win against stale or equal-HLC data, but it must not become an
unbounded delete that can erase newly authored post-regrant data.

### 4. Transport And Pairing

- All sync payloads are encrypted with AES-256-GCM.
- Keys are derived from a user-configured passphrase via Argon2id.
- LAN sync uses explicit pairing. Peer identity is pinned during the initial
  handshake and must be re-confirmed if the peer certificate/fingerprint
  changes.
- No plaintext user content leaves the device outside the encrypted sync
  payload.
- External relay paths must pass the same consent, privacy, and audit gates as
  LAN sync before egress.

## Sequence Diagram

```
Device A                          Device B
   |                                  |
   |--- pull (latest state) --------->|
   |<-- state snapshot ---------------|
   |                                  |
   |   [local merge via ChangeMerger] |
   |                                  |
   |--- push (merged result) -------->|
   |                                  |
   |   409 Conflict?                  |
   |   Yes -> re-pull, re-merge,      |
   |          retry push (max 3x)     |
   |                                  |
   |<-- 200 OK -----------------------|
```

## Erasure Sequence

```
Device A                          Device B
   |                                  |
   | revoke consent                   |
   | hard-delete local rows           |
   | retain tombstone skeletons       |
   |                                  |
   |--- DeletionEvent --------------->|  online fast path
   |--- tombstones in sync stream --->|  offline-peer catch-up path
   |                                  |
   |                                  | hard-delete stale rows
   |                                  | reject rows <= tombstone HLC
   |                                  |
   | re-grant + new write             |
   |--- row with higher HLC --------->|  allowed new post-regrant data
```

## Related Files

- `crates/maekon-network/src/sync/` — LAN server and transport
- `crates/maekon-network/src/integration/` — HTTP remote transport
- `crates/maekon-core/src/consent.rs` — GDPR consent and deletion records
- `crates/maekon-storage/src/migration/v38_sync_tombstones.rs` —
  row-level tombstone schema
- `crates/maekon-storage/src/migration/v39_hlc_clock.rs` — durable HLC floor
