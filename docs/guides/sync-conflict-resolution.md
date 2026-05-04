# Cross-Device Sync — Conflict Resolution Strategy

## Overview

Maekon uses a pull-merge-push cycle for cross-device synchronization.
The `SyncTransport` trait (`oneshim-core`) defines the transport boundary,
while `ChangeMerger` in `oneshim-network` implements record-level merging.

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

### 3. GDPR Deletion Events

- Deletion events ALWAYS win over non-deletion changes, regardless of
  HLC ordering.
- Article 17 compliance: once deleted, data cannot be restored by sync.
- The `deletion_pushed` flag prevents redundant deletion pushes across
  peers.

### 4. Encrypted Transport

- All sync payloads are encrypted with AES-256-GCM.
- Keys are derived from a user-configured passphrase via Argon2id.
- No plaintext data leaves the device.
- Peer identity is verified via device fingerprint exchange during the
  initial pairing handshake.

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

## Related Files

- `crates/oneshim-network/src/sync/` — LAN server and transport
- `crates/oneshim-network/src/integration/` — HTTP remote transport
- `crates/oneshim-core/src/consent.rs` — GDPR consent and deletion records
