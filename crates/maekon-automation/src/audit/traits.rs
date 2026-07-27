use maekon_core::models::ai_session::SessionAuditEntry;
use maekon_core::models::audit::{AuditEntry, AuditStats};

/// #8045 C2: durability-checked persist failure surfaced by
/// [`AuditPersistence::persist_checked`]. A sink with a bounded queue returns
/// this so an `AuditLevel::Full` record can fail closed instead of dropping the
/// entry silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditPersistError {
    /// The persistence channel is full (sustained-burst back-pressure).
    ChannelFull,
    /// The persistence drain task has exited; entries are no longer persisted.
    ChannelClosed,
}

impl std::fmt::Display for AuditPersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChannelFull => write!(f, "audit persistence channel full"),
            Self::ChannelClosed => write!(f, "audit persistence channel closed"),
        }
    }
}

impl std::error::Error for AuditPersistError {}

/// Callback trait for persisting audit entries to durable storage.
///
/// Implemented by the binary crate to bridge AuditLogger (library) with
/// SQLite (infrastructure), preserving hexagonal architecture boundaries.
pub trait AuditPersistence: Send + Sync {
    fn persist(&self, entry: &AuditEntry);

    /// #8045 C2: durability-checked persist. The default delegates to [`persist`]
    /// and reports success — sinks that cannot detect back-pressure (synchronous
    /// SQLite closures) are best-effort by nature and never fail here. Sinks with
    /// a bounded queue (e.g. `ChannelAuditPersistence`) override this to surface a
    /// full/closed channel so a strictest-level (`AuditLevel::Full`) record can
    /// fail closed rather than drop silently.
    fn persist_checked(&self, entry: &AuditEntry) -> Result<(), AuditPersistError> {
        self.persist(entry);
        Ok(())
    }
}

/// Blanket impl: any `Fn(&AuditEntry) + Send + Sync` satisfies `AuditPersistence`.
impl<F: Fn(&AuditEntry) + Send + Sync> AuditPersistence for F {
    fn persist(&self, entry: &AuditEntry) {
        self(entry);
    }
}

/// Callback trait for persisting AI conversation **session** audit entries to
/// durable storage (#6168).
///
/// The session audit trail (`AuditLogPort::record_session_event`) is distinct
/// from the command audit trail (`AuditPersistence`): it carries
/// [`SessionAuditEntry`] rows (provider/category/event_type/payload) destined
/// for the `session_audit_log` table rather than `AuditEntry` rows for
/// `audit_log`. The binary crate (`src-tauri`) implements this to bridge the
/// `AuditLogAdapter` (library) to `SqliteStorage` (infrastructure), preserving
/// the hexagonal boundary (`maekon-automation` MUST NOT depend on
/// `maekon-storage`).
///
/// Best-effort by contract: implementations MUST NOT panic or block the
/// reactor. The `AuditLogPort::record_session_event` method is infallible, so
/// failures are logged and dropped at the adapter, never propagated.
pub trait SessionAuditPersistence: Send + Sync {
    fn persist(&self, entry: &SessionAuditEntry);
}

/// Blanket impl: any `Fn(&SessionAuditEntry) + Send + Sync` satisfies
/// [`SessionAuditPersistence`].
impl<F: Fn(&SessionAuditEntry) + Send + Sync> SessionAuditPersistence for F {
    fn persist(&self, entry: &SessionAuditEntry) {
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
/// Used by [`super::logger::AuditLogger`] to fall through from the in-memory
/// `VecDeque` buffer (~1000-row cap) to persistent storage when the buffer
/// doesn't have enough recent or command-scoped entries.
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
    /// Return aggregate statistics from the durable audit source.
    fn stats(&self) -> AuditStats;

    /// Return the most recent audit entries across all command ids.
    /// Ordered by `timestamp DESC`. Empty vec if none exist.
    fn recent_entries(&self, limit: usize) -> Vec<AuditEntry>;

    /// Return audit entries whose `command_id` exactly matches.
    /// Ordered by `timestamp DESC`. Empty vec if none match.
    /// Synchronous — implementations doing I/O should use `block_in_place`.
    fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry>;
}
