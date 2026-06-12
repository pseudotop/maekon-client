use maekon_core::models::audit::AuditEntry;

/// Callback trait for persisting audit entries to durable storage.
///
/// Implemented by the binary crate to bridge AuditLogger (library) with
/// SQLite (infrastructure), preserving hexagonal architecture boundaries.
pub trait AuditPersistence: Send + Sync {
    fn persist(&self, entry: &AuditEntry);
}

/// Blanket impl: any `Fn(&AuditEntry) + Send + Sync` satisfies `AuditPersistence`.
impl<F: Fn(&AuditEntry) + Send + Sync> AuditPersistence for F {
    fn persist(&self, entry: &AuditEntry) {
        self(entry);
    }
}

/// Query interface for historical audit lookup.
///
/// Implemented by the binary crate to bridge `AuditLogger` (library) with
/// SQLite-backed historical storage, preserving hexagonal architecture
/// boundaries (`maekon-automation` cannot depend on `maekon-storage`
/// directly per ADR-001).
///
/// Used by [`super::logger::AuditLogger::entries_by_command_id`] to fall through from the
/// in-memory `VecDeque` buffer (~1000-row cap) to persistent storage when
/// the buffer doesn't have enough matching entries.
///
/// # Invariant
///
/// Implementations MUST return entries with stable, unique `entry_id`. The
/// dedupe step in [`super::logger::AuditLogger::entries_by_command_id`] keys on `entry_id`
/// to merge buffer + storage results — the same `entry_id` MUST always carry
/// the same logical entry (same timestamp, same details). The production
/// `SqliteAuditQuery` satisfies this via the V25 schema's
/// `UNIQUE(entry_id)` constraint. Custom implementations that violate
/// this invariant may silently drop legitimate entries during dedup.
pub trait AuditQuery: Send + Sync {
    /// Return audit entries whose `command_id` exactly matches.
    /// Ordered by `timestamp DESC`. Empty vec if none match.
    /// Synchronous — implementations doing I/O should use `block_in_place`.
    fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry>;
}
