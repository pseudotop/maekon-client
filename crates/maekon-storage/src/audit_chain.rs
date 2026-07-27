//! audit_log SHA-256 hash-chain helper (#4834, ADR-072 client mirror).
//!
//! Collects the hash-computation logic that makes `audit_log` rows
//! tamper-**evident**. Chain rule: `entry_hash = SHA256(prev_hash_bytes ||
//! canonical(record))`.
//! - `prev_hash` is the `entry_hash` of the preceding chain row (genesis uses
//!   [`GENESIS_PREV_HASH`]).
//! - `canonical(record)` is a **length-prefixed + fixed-field-order**
//!   serialization of the immutable fields. serde_json is not used because of
//!   its boundary ambiguity (key ordering / escaping).
//!
//! ## canonical serialization format
//! Each field is encoded as `field_index(1 byte) || u32 len(big-endian, UTF-8
//! byte length) || bytes`. To distinguish an `Option`'s None from a length-0
//! sentinel, a separate presence byte (0/1) precedes the length. The field order
//! fixes the immutable field order of AuditEntry:
//!   0 entry_id, 1 timestamp(RFC3339), 2 session_id, 3 command_id,
//!   4 action_type, 5 status(stable string), 6 details(Option), 7 execution_time_ms(Option).
//!
//! ## hash_version seam (out-of-scope)
//! The current algorithm is plain SHA-256 (no signature). The [`HASH_VERSION`]
//! constant is kept so the algorithm can be branched when HMAC/Ed25519 is
//! introduced to defend against a full-rewrite insider threat. This issue
//! (#4834) implements only v1 (SHA-256-only).

use crate::error::StorageError;
use sha2::{Digest, Sha256};

/// `prev_hash` of the genesis row — 64-char zero hex (32 zero bytes).
pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Current hash-algorithm version. v1 = plain SHA-256 (no signature).
/// Used as the seam for branching when HMAC/Ed25519 is introduced later
/// (#4834 out-of-scope).
pub const HASH_VERSION: u8 = 1;

/// `app_meta` key holding the `entry_hash` of the last row removed by the
/// audit-log compliance-window retention prune (#8056 P3).
///
/// `audit_log` is a legally-retained SHA-256 hash chain (ADR-072 client
/// mirror), so it cannot simply drop its oldest rows: the verifier requires the
/// first retained row's `prev_hash` to equal [`GENESIS_PREV_HASH`]. When the
/// retention prune removes the oldest contiguous prefix it records the pruned
/// prefix's final `entry_hash` here — the exact value the new first row's
/// `prev_hash` already carries. The verifier then accepts that anchor as the
/// pruned chain root, so a compliance-window prune keeps the retained chain
/// tamper-evident (each retained row still links to an immutable recorded root)
/// while bounding unbounded growth. `app_meta` is retained across GDPR erasure,
/// so the anchor survives alongside the retained chain.
pub const AUDIT_CHAIN_PRUNED_ROOT_META_KEY: &str = "audit.chain.pruned_root_prev_hash";

/// Immutable view of an audit record used for hash-chain computation.
///
/// Borrows only the immutable fields of
/// [`AuditEntry`](maekon_core::models::audit::AuditEntry) for canonical
/// serialization. It takes references rather than ownership, which guarantees
/// identical input on both the write and verify sides.
pub struct CanonicalRecord<'a> {
    pub entry_id: &'a str,
    pub timestamp_rfc3339: &'a str,
    pub session_id: &'a str,
    pub command_id: &'a str,
    pub action_type: &'a str,
    /// Stable string for the status (e.g. "Completed") — identical to the string
    /// stored in the DB.
    pub status: &'a str,
    pub details: Option<&'a str>,
    pub execution_time_ms: Option<u64>,
}

/// Encode a `u32` big-endian length prefix WITHOUT a truncating cast.
///
/// The canonical format uses a `u32` length prefix per field. A field longer
/// than `u32::MAX` (≈4 GiB) would silently wrap under `as u32`, producing a
/// shorter declared length while the full bytes are still appended — which
/// collides distinct records and breaks the tamper-evident chain. Since this
/// is a security-critical hash chain we REJECT (return an error) instead of
/// truncating, mirroring the `u32::try_from` precedent in
/// `sync_merger::extract_hlc_counter`.
fn push_len_prefix(buf: &mut Vec<u8>, index: u8, len: usize) -> Result<(), StorageError> {
    let len = u32::try_from(len).map_err(|_| StorageError::Validation {
        field: format!("audit_chain.field[{index}]"),
        message: format!("canonical field exceeds u32 length prefix ({len} bytes)"),
    })?;
    buf.extend_from_slice(&len.to_be_bytes());
    Ok(())
}

/// Pushes a string field onto the buffer as `index || u32 len(BE) || bytes`.
fn push_str_field(buf: &mut Vec<u8>, index: u8, value: &str) -> Result<(), StorageError> {
    buf.push(index);
    let bytes = value.as_bytes();
    push_len_prefix(buf, index, bytes.len())?;
    buf.extend_from_slice(bytes);
    Ok(())
}

/// Pushes an `Option<str>` field as `index || presence(0/1) || (if present)
/// u32 len(BE) || bytes`.
fn push_opt_str_field(
    buf: &mut Vec<u8>,
    index: u8,
    value: Option<&str>,
) -> Result<(), StorageError> {
    buf.push(index);
    match value {
        Some(v) => {
            buf.push(1);
            let bytes = v.as_bytes();
            push_len_prefix(buf, index, bytes.len())?;
            buf.extend_from_slice(bytes);
        }
        None => buf.push(0),
    }
    Ok(())
}

/// Pushes an `Option<u64>` field as `index || presence(0/1) || (if present)
/// u64 value(BE)`.
fn push_opt_u64_field(buf: &mut Vec<u8>, index: u8, value: Option<u64>) {
    buf.push(index);
    match value {
        Some(v) => {
            buf.push(1);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        None => buf.push(0),
    }
}

/// Builds the deterministic canonical byte string for a record.
///
/// Identical input always yields identical output (no key-ordering / escaping
/// ambiguity). The field order follows the 0..=7 indices in the module doc.
///
/// Returns an error (never truncates) when any single field exceeds the `u32`
/// length prefix — see [`push_len_prefix`].
pub fn canonical_bytes(record: &CanonicalRecord<'_>) -> Result<Vec<u8>, StorageError> {
    let mut buf = Vec::with_capacity(256);
    // Prefix the algorithm version to bind any future format branching to the integrity.
    buf.push(HASH_VERSION);
    push_str_field(&mut buf, 0, record.entry_id)?;
    push_str_field(&mut buf, 1, record.timestamp_rfc3339)?;
    push_str_field(&mut buf, 2, record.session_id)?;
    push_str_field(&mut buf, 3, record.command_id)?;
    push_str_field(&mut buf, 4, record.action_type)?;
    push_str_field(&mut buf, 5, record.status)?;
    push_opt_str_field(&mut buf, 6, record.details)?;
    push_opt_u64_field(&mut buf, 7, record.execution_time_ms);
    Ok(buf)
}

/// Computes `entry_hash = hex(SHA256(prev_hash_bytes || canonical(record)))`.
///
/// `prev_hash_hex` is the `entry_hash` of the preceding chain row (genesis uses
/// [`GENESIS_PREV_HASH`]). The hex string is absorbed into the hash as-is, byte
/// for byte (it uses the string bytes rather than a hex→raw decode, so the
/// result is deterministic even if `prev_hash` has a malformed format).
///
/// Returns an error (never truncates) when a record field exceeds the `u32`
/// length prefix — see [`canonical_bytes`].
pub fn compute_entry_hash(
    prev_hash_hex: &str,
    record: &CanonicalRecord<'_>,
) -> Result<String, StorageError> {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash_hex.as_bytes());
    hasher.update(canonical_bytes(record)?);
    let digest = hasher.finalize();
    Ok(hex::encode(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> CanonicalRecord<'static> {
        CanonicalRecord {
            entry_id: "e-1",
            timestamp_rfc3339: "2026-06-03T00:00:00+00:00",
            session_id: "sess-1",
            command_id: "cmd-1",
            action_type: "MouseClick",
            status: "Completed",
            details: Some("ok"),
            execution_time_ms: Some(42),
        }
    }

    #[test]
    fn genesis_prev_hash_is_64_zero_hex() {
        assert_eq!(GENESIS_PREV_HASH.len(), 64);
        assert!(GENESIS_PREV_HASH.chars().all(|c| c == '0'));
    }

    #[test]
    fn canonical_is_deterministic() {
        let r = sample();
        assert_eq!(
            canonical_bytes(&r).unwrap(),
            canonical_bytes(&sample()).unwrap()
        );
    }

    #[test]
    fn entry_hash_is_deterministic_and_hex64() {
        let r = sample();
        let h1 = compute_entry_hash(GENESIS_PREV_HASH, &r).unwrap();
        let h2 = compute_entry_hash(GENESIS_PREV_HASH, &sample()).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn changing_any_field_changes_hash() {
        let base = compute_entry_hash(GENESIS_PREV_HASH, &sample()).unwrap();

        let mut r = sample();
        r.details = Some("tampered");
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r).unwrap(), base);

        let mut r = sample();
        r.timestamp_rfc3339 = "2026-06-03T00:00:01+00:00";
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r).unwrap(), base);

        let mut r = sample();
        r.status = "Failed";
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r).unwrap(), base);

        let mut r = sample();
        r.execution_time_ms = None;
        assert_ne!(compute_entry_hash(GENESIS_PREV_HASH, &r).unwrap(), base);
    }

    #[test]
    fn changing_prev_hash_changes_hash() {
        let r = sample();
        let g = compute_entry_hash(GENESIS_PREV_HASH, &r).unwrap();
        let other = compute_entry_hash(&"a".repeat(64), &r).unwrap();
        assert_ne!(g, other);
    }

    #[test]
    fn none_details_differs_from_empty_details() {
        // None and Some("") must be distinguished by the presence byte.
        let mut none = sample();
        none.details = None;
        let mut empty = sample();
        empty.details = Some("");
        assert_ne!(
            compute_entry_hash(GENESIS_PREV_HASH, &none).unwrap(),
            compute_entry_hash(GENESIS_PREV_HASH, &empty).unwrap()
        );
    }

    #[test]
    fn length_prefix_prevents_field_boundary_ambiguity() {
        // ("ab","c") and ("a","bc") must not produce the same hash.
        let mut r1 = sample();
        r1.entry_id = "ab";
        r1.session_id = "c";
        let mut r2 = sample();
        r2.entry_id = "a";
        r2.session_id = "bc";
        assert_ne!(
            compute_entry_hash(GENESIS_PREV_HASH, &r1).unwrap(),
            compute_entry_hash(GENESIS_PREV_HASH, &r2).unwrap()
        );
    }

    /// Drive the `u32` length-prefix overflow guard directly (a real >4 GiB
    /// field is infeasible to allocate in a unit test). `push_len_prefix`
    /// MUST reject `u32::MAX + 1` rather than silently truncate to `0`, which
    /// would otherwise corrupt the tamper-evident chain.
    #[test]
    fn length_prefix_rejects_overflow_instead_of_truncating() {
        let mut buf = Vec::new();
        // u32::MAX exactly is the largest accepted length: it must be accepted AND
        // emit the full 4-byte big-endian prefix (0xFFFF_FFFF), not silently drop it.
        push_len_prefix(&mut buf, 0, u32::MAX as usize)
            .expect("u32::MAX is the largest accepted length and must not be rejected");
        assert_eq!(
            buf,
            u32::MAX.to_be_bytes(),
            "the boundary length must be written as its 4-byte big-endian prefix"
        );

        // One byte past u32::MAX must error (only reachable on 64-bit usize).
        if let Some(overflow) = (u32::MAX as usize).checked_add(1) {
            let err = push_len_prefix(&mut buf, 3, overflow)
                .expect_err("length past u32::MAX must be rejected, not truncated");
            assert!(matches!(err, StorageError::Validation { .. }));
        }
    }
}
