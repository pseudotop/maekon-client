use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Audit log level (used by the automation policy).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum AuditLevel {
    None,
    #[default]
    Basic,
    Detailed,
    Full,
}

/// Audit entry status.
// F-RC-C37-02: explicit wire contract — PascalCase matches AuditLevel and storage keys
// used in audit_bridge.rs ("external_grpc_completed" etc. are internal labels, not wire).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum AuditStatus {
    Started,
    Completed,
    Failed,
    Denied,
    Timeout,
}

/// Audit log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub entry_id: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub command_id: String,
    pub action_type: String,
    pub status: AuditStatus,
    pub details: Option<String>,
    pub execution_time_ms: Option<u64>,
}

/// Audit-log hash-chain integrity verification result (#4834, ADR-072 client mirror).
///
/// Holds the result of verifying the SHA-256 hash chain of `audit_log` rows
/// (the seq/prev_hash/entry_hash columns, v37). This is a new additive type and
/// does not touch the [`AuditEntry`]/CSV export wire contract — the chain fields
/// exist only on the DB row and in this report.
///
/// A SHA-256-only chain is tamper-**evident** (it detects accidental/partial
/// corruption and simple row edits) but not tamper-**proof**. Defending against
/// an insider threat that can fully rewrite the chain requires HMAC/Ed25519
/// signatures and is out of scope for this issue (see the `hash_version` seam).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditChainReport {
    /// Whether the entire chain is intact (no breaks).
    pub ok: bool,
    /// seq of the first row included in the chain (None if there is none).
    pub first_seq: Option<i64>,
    /// seq of the last row included in the chain (None if there is none).
    pub last_seq: Option<i64>,
    /// Number of rows that passed verification (i.e. are part of the chain).
    pub verified_count: u64,
    /// Number of legacy rows written before v37 that are not part of the chain (NULL chain).
    pub legacy_unchained_count: u64,
    /// First integrity break found (None if there is none).
    pub first_break: Option<AuditChainBreak>,
}

/// A single hash-chain integrity break (#4834).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditChainBreak {
    /// seq of the row where the break occurred.
    pub seq: i64,
    /// Reason for the break (human-readable description).
    pub reason: String,
}

/// Audit log statistics (replaces the previous tuple return value with a struct).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditStats {
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub denied: usize,
    pub timeout: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_level_default_is_basic() {
        assert_eq!(AuditLevel::default(), AuditLevel::Basic);
    }

    /// F-RC-C37-02: pin exact wire strings so any accidental rename_all change
    /// (or removal) is caught immediately.
    #[test]
    fn audit_status_wire_strings_pinned() {
        let cases = [
            (AuditStatus::Started, "\"Started\""),
            (AuditStatus::Completed, "\"Completed\""),
            (AuditStatus::Failed, "\"Failed\""),
            (AuditStatus::Denied, "\"Denied\""),
            (AuditStatus::Timeout, "\"Timeout\""),
        ];
        for (status, expected_json) in &cases {
            let json = serde_json::to_string(status).expect("serialize");
            assert_eq!(
                json, *expected_json,
                "AuditStatus wire string mismatch for {:?}",
                status
            );
            let deser: AuditStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(
                deser, *status,
                "AuditStatus round-trip failed for {:?}",
                status
            );
        }
    }

    #[test]
    fn audit_entry_serde_roundtrip() {
        let entry = AuditEntry {
            entry_id: "e-001".to_string(),
            timestamp: Utc::now(),
            session_id: "sess-001".to_string(),
            command_id: "cmd-001".to_string(),
            action_type: "MouseClick".to_string(),
            status: AuditStatus::Completed,
            details: Some("Success".to_string()),
            execution_time_ms: Some(150),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deser: AuditEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.entry_id, "e-001");
        assert_eq!(deser.status, AuditStatus::Completed);
    }

    #[test]
    fn audit_stats_default() {
        let stats = AuditStats::default();
        assert_eq!(stats.total, 0);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.denied, 0);
        assert_eq!(stats.timeout, 0);
    }
}
