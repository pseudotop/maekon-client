use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::logger::AuditLogger;

/// `Arc<RwLock<AuditLogger>>`를 `AuditLogPort`로 래핑하는 어댑터
///
/// ADR-001 §2: 포트 트레잇은 `&self`, 구현체는 interior mutability 사용
pub struct AuditLogAdapter {
    inner: Arc<RwLock<AuditLogger>>,
}

impl AuditLogAdapter {
    pub fn new(logger: Arc<RwLock<AuditLogger>>) -> Self {
        Self { inner: logger }
    }

    /// 내부 `Arc<RwLock<AuditLogger>>`에 대한 참조 (직접 접근이 필요한 레거시 코드용)
    pub fn inner(&self) -> &Arc<RwLock<AuditLogger>> {
        &self.inner
    }
}

#[async_trait::async_trait]
impl maekon_core::ports::audit_log::AuditLogPort for AuditLogAdapter {
    async fn pending_count(&self) -> usize {
        self.inner.read().await.pending_count()
    }

    async fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
        self.inner.read().await.recent_entries(limit)
    }

    async fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry> {
        self.inner.read().await.entries_by_status(status, limit)
    }

    async fn entries_by_action_prefix(&self, prefix: &str, limit: usize) -> Vec<AuditEntry> {
        self.inner
            .read()
            .await
            .entries_by_action_prefix(prefix, limit)
    }

    async fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
        self.inner
            .read()
            .await
            .entries_by_command_id(command_id, limit)
    }

    async fn stats(&self) -> AuditStats {
        self.inner.read().await.stats()
    }

    async fn has_pending_batch(&self) -> bool {
        self.inner.read().await.has_pending_batch()
    }

    async fn log_event(&self, action_type: &str, session_id: &str, details: &str) {
        self.inner
            .write()
            .await
            .log_event(action_type, session_id, details);
    }

    async fn log_start_if(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
    ) {
        self.inner
            .write()
            .await
            .log_start_if(level, command_id, session_id, action_type);
    }

    async fn log_complete_with_time(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        self.inner.write().await.log_complete_with_time(
            level,
            command_id,
            session_id,
            details,
            execution_time_ms,
        );
    }

    async fn drain_batch(&self) -> Vec<AuditEntry> {
        self.inner.write().await.drain_batch()
    }

    async fn drain_all(&self) -> Vec<AuditEntry> {
        self.inner.write().await.drain_all()
    }
}
