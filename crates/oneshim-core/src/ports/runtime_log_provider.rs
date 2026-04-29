//! Runtime log snapshot provider port for bug reports.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::bug_report::RuntimeLogSnapshot;

/// Provides runtime log snapshots for bug reports.
///
/// # Errors
/// - `CoreError::Internal` (wire: `internal.generic`) — ALL file I/O
///   failures from the reference `TauriRuntimeLogProvider` adapter
///   (log directory read, log file open, log line read). The adapter
///   uses a String-wrapping helper (`log_helpers::newest_log_file` /
///   `tail_log_file`) that pre-formats `std::io::Error` into
///   context-enriched strings, so the `#[from]` path to `CoreError::Io`
///   isn't reached. Spec §4.6 forbids storing `InternalCode::Io` in
///   `Internal { code }` — the current Internal.Generic + prefixed
///   message is the canonical form for file-I/O errors that need
///   context preservation. Future adapters MAY opt for `CoreError::Io`
///   if they don't need context prefixes.
/// - Absence of any log file on first launch is
///   `Ok(RuntimeLogSnapshot { log_file: None, line_count: 0, recent_text: "" })`,
///   not Err.
#[async_trait]
pub trait RuntimeLogProvider: Send + Sync {
    /// Read the tail of the most recent log file.
    async fn snapshot(&self, line_limit: usize) -> Result<RuntimeLogSnapshot, CoreError>;
}
