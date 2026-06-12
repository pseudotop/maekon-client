//! Domain models for cross-device sync (P3).
//!
//! These are the data structures that flow between ChangeExtractor,
//! SyncTransport, and ChangeMerger. They carry no logic -- pure DTOs.

use serde::{Deserialize, Serialize};

use crate::sync::Hlc;

/// The kind of changeset being transmitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSetKind {
    /// Normal data synchronization.
    #[default]
    Data,
    /// GDPR Article 17 deletion propagation.
    /// All peers receiving this changeset MUST perform local erasure.
    DeletionEvent,
}

/// A batch of changes to sync between devices.
///
/// Each Vec field holds serialized rows for one syncable table.
/// In Phase 3a-1 the row types are opaque `serde_json::Value`
/// placeholders; Phase 3a-2 will replace them with typed row structs
/// when the ChangeExtractor impl is built.
/// A content-free erasure skeleton (GDPR Art.17 cross-device, epic #5174): one
/// deleted synced-table row's id + the erasure HLC. Carries NO user content over the
/// wire (privacy P5). Mirrors a `sync_tombstones` row (V38, see migration). The fields
/// map 1:1 to the table columns; `hlc_wall_ms`/`hlc_counter` match `Hlc` so the S3
/// suppression compare is lexicographically identical to `Hlc`'s `Ord`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tombstone {
    /// One of the 6 synced tables (validated against the allowlist before use in SQL).
    pub table_name: String,
    /// The deleted row's primary key (id / override_id / suggestion_id).
    pub row_id: String,
    /// Device that authored the erased row (origin-scoping; preserved across the wire).
    pub origin_device_id: String,
    /// Erasure HLC wall-clock ms (matches `Hlc::wall_ms`).
    pub hlc_wall_ms: u64,
    /// Erasure HLC counter (matches `Hlc::counter`).
    pub hlc_counter: u32,
    /// ISO-8601 erasure timestamp.
    pub deleted_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangeSet {
    pub kind: ChangeSetKind,
    pub origin_device_id: String,
    pub origin_device_name: String,
    pub watermark: Hlc,
    pub segments: Vec<serde_json::Value>,
    pub regimes: Vec<serde_json::Value>,
    pub overrides: Vec<serde_json::Value>,
    pub embeddings: Vec<serde_json::Value>,
    pub suggestions: Vec<serde_json::Value>,
    pub param_snapshots: Vec<serde_json::Value>,
    pub preferences: Vec<serde_json::Value>,
    /// GDPR Art.17 row-level erasure tombstones (#5174 S2/#5179). Content-free skeletons
    /// that flow through the normal sync stream so offline peers converge. The struct-level
    /// `#[serde(default)]` above keeps pre-upgrade peers decoding this as empty (P4 compat).
    pub tombstones: Vec<Tombstone>,
}

impl ChangeSet {
    /// Returns true when the changeset carries no data rows.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
            && self.regimes.is_empty()
            && self.overrides.is_empty()
            && self.embeddings.is_empty()
            && self.suggestions.is_empty()
            && self.param_snapshots.is_empty()
            && self.preferences.is_empty()
            && self.tombstones.is_empty()
    }

    /// Total number of rows across all tables.
    pub fn row_count(&self) -> usize {
        self.segments.len()
            + self.regimes.len()
            + self.overrides.len()
            + self.embeddings.len()
            + self.suggestions.len()
            + self.param_snapshots.len()
            + self.preferences.len()
            + self.tombstones.len()
    }
}

/// Result of applying a changeset via ChangeMerger.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncResult {
    /// Rows successfully applied (inserted or updated via LWW).
    pub applied: usize,
    /// Rows skipped because local HLC was higher (lost LWW race).
    pub skipped_lww: usize,
    /// Rows skipped because they already exist (duplicate PK).
    pub skipped_dup: usize,
    /// Rows soft-deleted via tombstone propagation.
    pub tombstoned: usize,
    /// The new high-watermark HLC after applying the changeset.
    pub new_watermark: Hlc,
}

/// Information about a known sync peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    /// Unique device identifier (UUID v4).
    pub device_id: String,
    /// Human-readable device name (e.g., "Work MacBook").
    pub device_name: String,
    /// ISO-8601 timestamp of last successful sync.
    pub last_sync_at: String,
    /// Peer's high-watermark HLC at last sync.
    pub watermark: Hlc,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_hlc() -> Hlc {
        Hlc {
            wall_ms: 1710859200000,
            counter: 42,
            device_id: "dev-a".to_string(),
        }
    }

    #[test]
    fn changeset_is_empty_when_default() {
        let cs = ChangeSet::default();
        assert!(cs.is_empty());
        assert_eq!(cs.row_count(), 0);
    }

    #[test]
    fn changeset_row_count_sums_all_tables() {
        let mut cs = ChangeSet::default();
        cs.segments.push(serde_json::json!({"id": "seg-1"}));
        cs.regimes.push(serde_json::json!({"id": "reg-1"}));
        cs.regimes.push(serde_json::json!({"id": "reg-2"}));
        assert_eq!(cs.row_count(), 3);
        assert!(!cs.is_empty());
    }

    #[test]
    fn changeset_serde_roundtrip() {
        let cs = ChangeSet {
            kind: ChangeSetKind::Data,
            origin_device_id: "dev-a".to_string(),
            origin_device_name: "Work Mac".to_string(),
            watermark: sample_hlc(),
            segments: vec![serde_json::json!({"id": "seg-1"})],
            ..Default::default()
        };
        let json = serde_json::to_string(&cs).unwrap();
        let parsed: ChangeSet = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.origin_device_id, "dev-a");
        assert_eq!(parsed.segments.len(), 1);
        assert!(parsed.regimes.is_empty());
    }

    #[test]
    fn changeset_kind_serde_snake_case() {
        let json = serde_json::to_string(&ChangeSetKind::DeletionEvent).unwrap();
        assert_eq!(json, "\"deletion_event\"");
        let parsed: ChangeSetKind = serde_json::from_str("\"data\"").unwrap();
        assert_eq!(parsed, ChangeSetKind::Data);
    }

    #[test]
    fn sync_result_default_zeros() {
        let result = SyncResult::default();
        assert_eq!(result.applied, 0);
        assert_eq!(result.skipped_lww, 0);
        assert_eq!(result.skipped_dup, 0);
        assert_eq!(result.tombstoned, 0);
    }

    #[test]
    fn peer_info_serde_roundtrip() {
        let peer = PeerInfo {
            device_id: "dev-b".to_string(),
            device_name: "Home Desktop".to_string(),
            last_sync_at: "2026-03-19T12:00:00Z".to_string(),
            watermark: sample_hlc(),
        };
        let json = serde_json::to_string(&peer).unwrap();
        let parsed: PeerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.device_id, "dev-b");
        assert_eq!(parsed.device_name, "Home Desktop");
    }
}
