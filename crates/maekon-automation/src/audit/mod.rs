// ADR-013: audit module split (was 1418 lines)
// Responsibilities:
//   traits.rs              — AuditPersistence + AuditQuery port traits
//   logger.rs              — AuditLogger struct, all logging methods, PII sanitization helpers
//   adapter.rs             — AuditLogAdapter (AuditLogPort impl, bridges tokio RwLock to port)
//   channel_persistence.rs — ChannelAuditPersistence (off-reactor blocking-SQLite drain, #6123)
//   tests.rs               — AuditLogger unit tests (buffering, PII redaction, persistence)
//   query_tests.rs         — storage fall-through tests for recent/command-scoped audit queries

mod adapter;
mod channel_persistence;
mod logger;
#[cfg(test)]
mod query_tests;
#[cfg(test)]
mod tests;
mod traits;

// Canonical types from maekon-core — re-exported for backward compat
pub use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};

// Public surface — all callers use `maekon_automation::audit::{...}`
pub use adapter::AuditLogAdapter;
pub use channel_persistence::ChannelAuditPersistence;
pub use logger::{AuditError, AuditLogger};
pub use traits::{AuditPersistError, AuditPersistence, AuditQuery, SessionAuditPersistence};
