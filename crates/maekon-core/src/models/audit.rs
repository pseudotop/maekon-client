use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 감사 로그 수준 (automation policy에서 사용)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum AuditLevel {
    None,
    #[default]
    Basic,
    Detailed,
    Full,
}

/// 감사 항목 상태
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

/// 감사 로그 항목
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

/// 감사 로그 해시 체인 무결성 검증 결과 (#4834, ADR-072 client mirror).
///
/// `audit_log` 행의 SHA-256 해시 체인(seq/prev_hash/entry_hash 컬럼, v37)을
/// 검증한 결과를 담는다. 신규 additive 타입이며 [`AuditEntry`]/CSV export
/// wire contract를 건드리지 않는다 — 체인 필드는 DB row 와 이 리포트에만 존재한다.
///
/// SHA-256-only 체인은 tamper-**evident**(우발적/부분적 손상, 단순 행 편집 탐지)
/// 이지 tamper-**proof**가 아니다. 전면 재기록 가능한 내부자 위협 방어는
/// HMAC/Ed25519 서명이 필요하며 본 이슈 범위 밖이다(`hash_version` seam 참조).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditChainReport {
    /// 체인 전체가 무결한지 여부 (break 없음).
    pub ok: bool,
    /// 체인에 편입된 첫 행의 seq (없으면 None).
    pub first_seq: Option<i64>,
    /// 체인에 편입된 마지막 행의 seq (없으면 None).
    pub last_seq: Option<i64>,
    /// 검증을 통과한(체인 편입) 행 수.
    pub verified_count: u64,
    /// v37 이전 작성되어 체인 미편입(NULL chain)인 legacy 행 수.
    pub legacy_unchained_count: u64,
    /// 최초로 발견된 무결성 위반 (없으면 None).
    pub first_break: Option<AuditChainBreak>,
}

/// 해시 체인 무결성 위반 1건 (#4834).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditChainBreak {
    /// 위반이 발생한 행의 seq.
    pub seq: i64,
    /// 위반 사유 (사람이 읽을 수 있는 설명).
    pub reason: String,
}

/// 감사 로그 통계 (이전 튜플 반환값을 구조체로 대체)
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
