use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// 감사 로그 수준 (automation policy에서 사용)
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditLevel {
    None,
    #[default]
    Basic,
    Detailed,
    Full,
}

/// 감사 항목 상태
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

    #[test]
    fn audit_status_serde_roundtrip() {
        for status in [
            AuditStatus::Started,
            AuditStatus::Completed,
            AuditStatus::Failed,
            AuditStatus::Denied,
            AuditStatus::Timeout,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let deser: AuditStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(deser, status);
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
