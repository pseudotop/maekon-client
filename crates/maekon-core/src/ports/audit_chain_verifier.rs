//! Audit-log hash-chain integrity verification port (#7600).
//!
//! Wraps the durable SQLite `audit_log` SHA-256 hash-chain check (see
//! `AuditChainReport`, ADR-072) behind a narrow port so both the desktop IPC
//! command (`verify_audit_log` in `src-tauri`) and the local web dashboard
//! HTTP surface (`GET /audit/verify`) can reach the SAME real verification
//! logic. Before #7600 this compliance capability had a real implementation
//! (`SqliteStorage::verify_audit_chain`) but zero webview callers, no HTTP
//! route, and no audit-page affordance — an advertised capability with no
//! delivery path in the standalone browser surface (`IS_TAURI=false`).
//!
//! Implemented by `SqliteStorage` in `maekon-storage`. Deliberately a
//! standalone one-method port (not folded into the much larger `WebStorage`
//! supertrait) so adding it does not ripple through every existing
//! `WebStorage` manual mock across the workspace.

use async_trait::async_trait;

use crate::models::audit::AuditChainReport;

/// Verifies the tamper-evident SHA-256 hash chain of the durable audit log.
#[async_trait]
pub trait AuditChainVerifierPort: Send + Sync {
    /// Runs the chain verification and returns the integrity report
    /// (ok/first_break/verified_count/legacy_unchained_count/...). Never
    /// errors — a SQL failure is reported as `ok: false` with a
    /// `first_break` reason (see `AuditChainReport`), mirroring the existing
    /// desktop IPC command's infallible contract.
    async fn verify_audit_chain(&self) -> AuditChainReport;
}
