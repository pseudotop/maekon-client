//! Local persistence ports — defines contracts for storing events, metrics,
//! sessions, suggestions, and activity segments in the local SQLite database.
//! Implemented by `SqliteStorage` in `maekon-storage`.

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::CoreError;
use crate::models::activity::{IdlePeriod, ProcessSnapshot, SessionStats};
use crate::models::ai_summary::AiSummaryArtifact;
use crate::models::event::Event;
use crate::models::suggestion::Suggestion;
use crate::models::system::SystemMetrics;
use crate::models::tiered_memory::SegmentSummary;

use super::batch_sink::{PendingEvent, UploadProvenance};

/// Local event and suggestion persistence.
///
/// # Errors
/// All methods return `CoreError::Storage` (wire: `storage.failed`) on
/// SQLite failures (lock contention, constraint violation, disk I/O).
/// Iter-47: previously documented as `Internal`; the mass conversion of
/// `StorageError::Internal` → `CoreError::Storage` made the earlier doc
/// stale for ~476 call sites.
#[async_trait]
pub trait StorageService: Send + Sync {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError>;

    /// Persist an event and its private upload evidence atomically.
    ///
    /// Return `true` only for an actual insert; duplicate and erasure-skipped
    /// writes return `false`. Evidence-aware adapters reject a proof that is
    /// not bound to this event and its persisted storage id.
    ///
    /// The compatibility default retains local persistence but cannot attest
    /// to a private proof or an actual insert, so callers must not enqueue it.
    async fn save_event_with_upload_provenance(
        &self,
        event: &Event,
        _provenance: Option<&UploadProvenance>,
    ) -> Result<bool, CoreError> {
        self.save_event(event).await?;
        Ok(false)
    }

    async fn get_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Event>, CoreError>;

    async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError>;

    /// Read pending events with their original storage identity and evidence.
    ///
    /// Legacy adapters cannot recover a persisted id or proof from an Event
    /// alone. Preserve that absence instead of manufacturing a new identity
    /// or trusting the current policy as historical evidence.
    async fn get_pending_events_with_upload_provenance(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingEvent>, CoreError> {
        Ok(self
            .get_pending_events(limit)
            .await?
            .into_iter()
            .map(|event| PendingEvent {
                storage_id: None,
                event,
                provenance: None,
            })
            .collect())
    }

    async fn mark_as_sent(&self, event_ids: &[String]) -> Result<(), CoreError>;

    async fn enforce_retention(&self) -> Result<usize, CoreError>;

    /// Persist an LLM/rule-based suggestion to the unified `suggestions` table.
    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError>;

    /// Persist a freshly-closed activity segment to the `activity_segments` table.
    ///
    /// This is the **local INSERT producer** (#5662). On a single standalone
    /// device the table was previously only ever populated by the cross-device
    /// sync merger (sync is off by default), so every segment-derived surface —
    /// Daily/Weekly Digest, Timeline, Dashboard timetable, regime distribution,
    /// Recalibration — rendered empty. Implementations stamp the local HLC +
    /// `origin_device_id` so the row also propagates correctly once sync is on,
    /// and must be idempotent on the `id` primary key (segment close may retry).
    async fn save_activity_segment(&self, summary: &SegmentSummary) -> Result<(), CoreError>;

    /// Update the llm_summary column of an existing activity_segments row.
    async fn update_segment_llm_summary(
        &self,
        segment_id: &str,
        llm_summary: &str,
    ) -> Result<(), CoreError>;

    /// Persist one terminal summary outcome. Implementations that have not yet
    /// adopted provenance storage retain the legacy generated-text behavior;
    /// the SQLite production adapter overrides this to store every outcome.
    async fn update_segment_ai_summary(
        &self,
        segment_id: &str,
        artifact: &AiSummaryArtifact,
    ) -> Result<(), CoreError> {
        match artifact.text.as_deref() {
            Some(text) => self.update_segment_llm_summary(segment_id, text).await,
            None => Ok(()),
        }
    }
}

/// System metrics, process snapshots, idle periods, and session counters.
///
/// # Errors
/// All methods return `CoreError::Storage` (wire: `storage.failed`) on SQLite failures
/// (lock contention, constraint violation, disk I/O).
/// Iter-47: previously documented as `Internal`; aligned with `StorageService` after
/// the mass `StorageError::Internal` → `CoreError::Storage` conversion.
#[async_trait]
pub trait MetricsStorage: Send + Sync {
    async fn save_metrics(&self, metrics: &SystemMetrics) -> Result<(), CoreError>;

    async fn get_metrics(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SystemMetrics>, CoreError>;

    async fn aggregate_hourly_metrics(&self, hour: DateTime<Utc>) -> Result<(), CoreError>;

    async fn cleanup_old_metrics(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;

    /// Delete `system_metrics_hourly` rollup rows whose `hour` predates `before`.
    ///
    /// The hourly rollup table has a documented 30-day retention (V3 migration)
    /// but previously had no scheduled cleanup, so it grew unbounded. `before`
    /// is truncated to the start of its hour and compared against the stored
    /// hour-bucket key (`%Y-%m-%dT%H:00:00Z`) so the boundary bucket is treated
    /// consistently with the write-time format. Returns the number of rows
    /// deleted.
    async fn cleanup_old_hourly_metrics(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;

    async fn save_process_snapshot(&self, snapshot: &ProcessSnapshot) -> Result<(), CoreError>;

    async fn get_process_snapshots(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProcessSnapshot>, CoreError>;

    async fn cleanup_old_process_snapshots(
        &self,
        before: DateTime<Utc>,
    ) -> Result<usize, CoreError>;

    async fn start_idle_period(&self, start_time: DateTime<Utc>) -> Result<i64, CoreError>;

    async fn end_idle_period(&self, id: i64, end_time: DateTime<Utc>) -> Result<(), CoreError>;

    async fn get_ongoing_idle_period(&self) -> Result<Option<(i64, IdlePeriod)>, CoreError>;

    async fn get_idle_periods(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<IdlePeriod>, CoreError>;

    async fn cleanup_old_idle_periods(&self, before: DateTime<Utc>) -> Result<usize, CoreError>;

    async fn upsert_session(&self, stats: &SessionStats) -> Result<(), CoreError>;

    async fn get_session(&self, session_id: &str) -> Result<Option<SessionStats>, CoreError>;

    async fn end_session(&self, session_id: &str, ended_at: DateTime<Utc>)
        -> Result<(), CoreError>;

    async fn increment_session_counters(
        &self,
        session_id: &str,
        events: u64,
        frames: u64,
        idle_secs: u64,
    ) -> Result<(), CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_codes::StorageCode;
    use crate::models::event::{SystemEvent, SystemEventType};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct LegacyOnlyStorage {
        save_calls: AtomicUsize,
        saved: Mutex<Vec<Event>>,
        pending: Vec<Event>,
        pending_limits: Mutex<Vec<usize>>,
        fail_save: bool,
        fail_pending: bool,
    }

    #[async_trait]
    impl StorageService for LegacyOnlyStorage {
        async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
            self.save_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_save {
                return Err(CoreError::Storage {
                    code: StorageCode::Failed,
                    message: "legacy save unavailable".to_string(),
                });
            }
            self.saved.lock().unwrap().push(event.clone());
            Ok(())
        }

        async fn get_events(
            &self,
            _from: DateTime<Utc>,
            _to: DateTime<Utc>,
            _limit: usize,
        ) -> Result<Vec<Event>, CoreError> {
            panic!("upload defaults must use the legacy pending operation")
        }

        async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError> {
            self.pending_limits.lock().unwrap().push(limit);
            if self.fail_pending {
                return Err(CoreError::Storage {
                    code: StorageCode::Failed,
                    message: "legacy pending unavailable".to_string(),
                });
            }
            Ok(self.pending.clone())
        }

        async fn mark_as_sent(&self, _event_ids: &[String]) -> Result<(), CoreError> {
            panic!("upload defaults must not acknowledge events")
        }

        async fn enforce_retention(&self) -> Result<usize, CoreError> {
            panic!("upload defaults must not change retention")
        }

        async fn save_suggestion(&self, _suggestion: &Suggestion) -> Result<(), CoreError> {
            panic!("upload defaults must not write suggestions")
        }

        async fn save_activity_segment(&self, _summary: &SegmentSummary) -> Result<(), CoreError> {
            panic!("upload defaults must not write activity segments")
        }

        async fn update_segment_llm_summary(
            &self,
            _segment_id: &str,
            _llm_summary: &str,
        ) -> Result<(), CoreError> {
            panic!("upload defaults must not update activity summaries")
        }
    }

    fn event(sequence: usize) -> Event {
        Event::System(SystemEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: SystemEventType::MetricsUpdate,
            timestamp: Utc::now(),
            data: serde_json::json!({ "sequence": sequence }),
        })
    }

    #[tokio::test]
    async fn legacy_default_save_persists_once_without_attesting_an_insert_or_proof() {
        for with_proof in [false, true] {
            let storage = LegacyOnlyStorage::default();
            let event = event(1);
            let proof =
                UploadProvenance::new("historical-id", &event, [3; 32], true, true).unwrap();

            let inserted = storage
                .save_event_with_upload_provenance(&event, with_proof.then_some(&proof))
                .await
                .unwrap();

            assert!(!inserted, "legacy storage cannot authorize new upload work");
            assert_eq!(storage.save_calls.load(Ordering::SeqCst), 1);
            let saved = storage.saved.lock().unwrap();
            assert_eq!(saved.len(), 1);
            assert_eq!(
                serde_json::to_value(&saved[0]).unwrap(),
                serde_json::to_value(&event).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn legacy_default_pending_preserves_payloads_without_inventing_identity_or_proof() {
        let storage = LegacyOnlyStorage {
            pending: vec![event(1), event(2)],
            ..Default::default()
        };

        let rows = storage
            .get_pending_events_with_upload_provenance(7)
            .await
            .unwrap();

        assert_eq!(*storage.pending_limits.lock().unwrap(), vec![7]);
        assert_eq!(rows.len(), storage.pending.len());
        for (row, original) in rows.iter().zip(&storage.pending) {
            assert_eq!(row.storage_id, None);
            assert_eq!(row.provenance, None);
            assert_eq!(
                serde_json::to_value(&row.event).unwrap(),
                serde_json::to_value(original).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn legacy_default_upload_operations_preserve_typed_storage_errors() {
        let storage = LegacyOnlyStorage {
            fail_save: true,
            fail_pending: true,
            ..Default::default()
        };
        let error = storage
            .save_event_with_upload_provenance(&event(1), None)
            .await
            .unwrap_err();
        match error {
            CoreError::Storage { code, message } => {
                assert_eq!(code, StorageCode::Failed);
                assert_eq!(message, "legacy save unavailable");
            }
            other => panic!("legacy save must preserve its storage error, got {other:?}"),
        }
        assert_eq!(storage.save_calls.load(Ordering::SeqCst), 1);
        assert!(storage.saved.lock().unwrap().is_empty());

        let error = storage
            .get_pending_events_with_upload_provenance(9)
            .await
            .unwrap_err();
        match error {
            CoreError::Storage { code, message } => {
                assert_eq!(code, StorageCode::Failed);
                assert_eq!(message, "legacy pending unavailable");
            }
            other => panic!("legacy pending must preserve its storage error, got {other:?}"),
        }
        assert_eq!(*storage.pending_limits.lock().unwrap(), vec![9]);
    }
}
