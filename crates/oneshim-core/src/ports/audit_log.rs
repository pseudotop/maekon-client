//! Audit logging port — defines the contract for recording, querying,
//! and batch-flushing automation audit entries.
//! Implemented by `AuditLogger` in `oneshim-automation`.

use async_trait::async_trait;

use crate::models::ai_session::SessionAuditEntry;
use crate::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};

/// 감사 로그 포트 — oneshim-web 핸들러가 사용하는 감사 로그 인터페이스
///
/// 구현체는 내부적으로 interior mutability를 사용하여 `&self`로 mutation을 처리합니다.
/// (ADR-001 §2: 포트 트레잇은 `&self` 사용, 구현체는 `RwLock` 사용)
///
/// # Errors
/// **No fallible methods.** Every method returns `()`, `usize`, `bool`,
/// `Vec<AuditEntry>`, or `AuditStats` — not `Result<_, _>`. This is by
/// design: the audit log is best-effort instrumentation that must never
/// block the automation path. Buffer overflow is silently dropped and
/// surfaced via `stats().dropped_count`; batch upload failures are
/// retained internally and retried by the `AuditLogger` impl.
/// `record_session_event` has a no-op default for adapters that don't
/// support session audit.
#[async_trait]
pub trait AuditLogPort: Send + Sync {
    // ── Query methods ──

    /// 버퍼의 대기 중인 항목 수
    async fn pending_count(&self) -> usize;

    /// 최근 항목 조회 (비파괴적, 최신순)
    async fn recent_entries(&self, limit: usize) -> Vec<AuditEntry>;

    /// 상태 기준 필터 조회
    async fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry>;

    /// action_type 접두사 기준 필터 조회
    async fn entries_by_action_prefix(&self, prefix: &str, limit: usize) -> Vec<AuditEntry>;

    /// 통계 집계
    async fn stats(&self) -> AuditStats;

    /// 배치 전송 가능 여부
    async fn has_pending_batch(&self) -> bool;

    // ── Mutation methods ──

    /// 일반 이벤트 로깅
    async fn log_event(&self, action_type: &str, session_id: &str, details: &str);

    /// 조건부 시작 로깅 (AuditLevel::None이면 스킵)
    async fn log_start_if(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
    );

    /// 실행 시간 포함 완료 로깅 (AuditLevel::None이면 스킵)
    async fn log_complete_with_time(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    );

    // ── Drain methods (batch upload) ──

    /// 배치 크기만큼 드레인
    async fn drain_batch(&self) -> Vec<AuditEntry>;

    /// 전체 드레인
    async fn drain_all(&self) -> Vec<AuditEntry>;

    /// Return audit entries whose `command_id` exactly matches the given value.
    /// Ordered by `timestamp DESC`. Returns empty vec if none match or on
    /// storage error (infallible by contract — error is logged by impl).
    ///
    /// # Errors
    /// Infallible (returns empty vec on storage error).
    async fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry>;

    // ── Session audit (best-effort) ──

    /// AI 대화 세션 감사 이벤트 기록 (best-effort: 실패 시 경고만, 에러 전파 안 함)
    async fn record_session_event(&self, _entry: SessionAuditEntry) {
        // Default no-op — 구현체에서 세션 감사 지원 시 override
    }
}

#[cfg(test)]
mod port_contract_tests {
    use super::*;

    /// Compile-time assertion — validates the trait method signature.
    /// Uses a trait object to avoid the E0401 nested-generic restriction.
    #[allow(dead_code)]
    fn assert_port_has_entries_by_command_id(
        p: &dyn AuditLogPort,
    ) -> impl std::future::Future<Output = Vec<AuditEntry>> + '_ {
        p.entries_by_command_id("cmd", 10)
    }
}
