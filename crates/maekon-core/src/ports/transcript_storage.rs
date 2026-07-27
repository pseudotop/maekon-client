//! Port for persisting speech-to-text transcripts to local storage (#8059).

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::audio::TranscriptRecord;
use crate::types::TimeWindow;

/// Persist and query voice transcripts.
///
/// Transcripts are a LOCAL-ONLY artifact: they are never added to the
/// cross-device sync surface, so this port has no HLC/tombstone concerns. The
/// `save_transcript` implementation also indexes the (PII-masked) text into the
/// keyword full-text index so a persisted transcript is immediately reachable
/// from search. Deletion is handled by the shared range-delete / full-wipe /
/// age-retention primitives (they clear both the table and its search index
/// rows), so this port intentionally exposes only insert + range query.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for all SQLite operations.
#[async_trait]
pub trait TranscriptStoragePort: Send + Sync {
    /// Persist a transcript and index its text for keyword search. Callers that
    /// treat persistence as best-effort (transcription is the primary function)
    /// should log and continue on `Err`.
    async fn save_transcript(&self, record: &TranscriptRecord) -> Result<(), CoreError>;

    /// List transcripts whose `timestamp` falls inside the closed-closed
    /// window, most recent first.
    async fn query_transcripts_in_range(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<TranscriptRecord>, CoreError>;
}
