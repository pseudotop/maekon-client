//! Durable, metadata-only audit adapter for advisory candidate decisions.

use super::SqliteStorage;
use async_trait::async_trait;
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::error_codes::StorageCode;
use maekon_core::models::audit::{AuditEntry, AuditStatus};
use maekon_core::models::candidate_decision::{DecisionAuditPhase, DecisionAuditRecord};
use maekon_core::ports::candidate_decision::CandidateDecisionAuditPort;
use std::sync::Arc;

pub struct SqliteCandidateDecisionAudit {
    storage: Arc<SqliteStorage>,
}

impl SqliteCandidateDecisionAudit {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

fn unavailable() -> CoreError {
    CoreError::Storage {
        code: StorageCode::Failed,
        message: "Candidate decision audit commit unavailable".into(),
    }
}

#[async_trait]
impl CandidateDecisionAuditPort for SqliteCandidateDecisionAudit {
    async fn record(&self, record: DecisionAuditRecord) -> Result<(), CoreError> {
        let details = serde_json::to_string(&record).map_err(|_| unavailable())?;
        let entry = AuditEntry {
            entry_id: record.id.to_string(),
            timestamp: Utc::now(),
            session_id: record.decision_id.to_string(),
            command_id: "candidate-decision".into(),
            action_type: "candidate_decision.advisory".into(),
            status: match record.phase {
                DecisionAuditPhase::BeforeSend => AuditStatus::Started,
                DecisionAuditPhase::AfterSend
                    if record.attempt.as_ref().is_some_and(|a| a.failure.is_some()) =>
                {
                    AuditStatus::Failed
                }
                DecisionAuditPhase::AfterSend | DecisionAuditPhase::CacheHit => {
                    AuditStatus::Completed
                }
            },
            details: Some(details),
            execution_time_ms: record.attempt.as_ref().map(|a| a.elapsed_ms),
        };
        let storage = self.storage.clone();
        let inserted =
            tokio::task::spawn_blocking(move || storage.try_save_durable_audit_entry(&entry))
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
        if !inserted {
            return Err(unavailable());
        }
        Ok(())
    }
}

#[async_trait]
impl maekon_core::ports::candidate_assessment::CandidateAssessmentAuditPort
    for SqliteCandidateDecisionAudit
{
    async fn record_assessment(
        &self,
        record: maekon_core::models::candidate_assessment::AssessmentAuditRecord,
    ) -> Result<(), CoreError> {
        use maekon_core::models::candidate_assessment::AssessmentAuditPhase;
        let details = serde_json::to_string(&record).map_err(|_| unavailable())?;
        let entry = AuditEntry {
            entry_id: record.id.to_string(),
            timestamp: Utc::now(),
            session_id: record.decision_id.to_string(),
            command_id: "candidate-assessment".into(),
            action_type: "candidate_assessment.advisory".into(),
            status: match record.phase {
                AssessmentAuditPhase::BeforeAttempt => AuditStatus::Started,
                AssessmentAuditPhase::AfterAttempt
                    if record
                        .attempt
                        .as_ref()
                        .is_some_and(|attempt| attempt.failure.is_some()) =>
                {
                    AuditStatus::Failed
                }
                AssessmentAuditPhase::AfterAttempt | AssessmentAuditPhase::CacheHit => {
                    AuditStatus::Completed
                }
            },
            details: Some(details),
            execution_time_ms: record.attempt.as_ref().map(|attempt| attempt.elapsed_ms),
        };
        let storage = self.storage.clone();
        let inserted =
            tokio::task::spawn_blocking(move || storage.try_save_durable_audit_entry(&entry))
                .await
                .map_err(|_| unavailable())?
                .map_err(|_| unavailable())?;
        if !inserted {
            return Err(unavailable());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::candidate_decision::{
        DecisionAuditContext, DecisionStage, DECISION_RUBRIC, DECISION_SCHEMA, JEV_MODEL,
    };
    use uuid::Uuid;

    fn disk() -> (tempfile::TempDir, Arc<SqliteStorage>) {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap());
        (dir, storage)
    }

    fn assert_commit_unavailable(error: CoreError) {
        let CoreError::Storage { code, message } = error else {
            panic!("Expected the closed storage failure contract, got {error:?}");
        };
        assert_eq!(code, StorageCode::Failed);
        assert_eq!(message, "Candidate decision audit commit unavailable");
    }

    fn record() -> DecisionAuditRecord {
        DecisionAuditRecord {
            id: Uuid::new_v4(),
            decision_id: Uuid::new_v4(),
            attempt_id: None,
            phase: DecisionAuditPhase::BeforeSend,
            stage: DecisionStage::Choice,
            attempt: None,
            context: DecisionAuditContext {
                namespace_hash: "a".repeat(64),
                provider: "typesafe",
                requested_model: JEV_MODEL,
                schema_revision: DECISION_SCHEMA,
                rubric_revision: DECISION_RUBRIC,
            },
            cache_source: None,
        }
    }

    #[tokio::test]
    async fn candidate_audit_ports_reject_volatile_storage_before_acknowledgment() {
        use maekon_core::models::candidate_assessment::{
            AssessmentAuditPhase, AssessmentAuditRecord, AssessmentTransport, ASSESSMENT_SCHEMA,
        };
        use maekon_core::models::candidate_decision_policy::CandidateProvider;
        use maekon_core::ports::candidate_assessment::CandidateAssessmentAuditPort;
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        assert_commit_unavailable(audit.record(record()).await.unwrap_err());
        let assessment = AssessmentAuditRecord {
            id: Uuid::new_v4(),
            decision_id: Uuid::new_v4(),
            attempt_id: None,
            phase: AssessmentAuditPhase::BeforeAttempt,
            namespace_hash: "a".repeat(64),
            provider: CandidateProvider::LocalRules,
            transport: AssessmentTransport::InProcess,
            schema_revision: ASSESSMENT_SCHEMA,
            attempt: None,
            cache_source: None,
        };
        assert_commit_unavailable(audit.record_assessment(assessment).await.unwrap_err());
        assert_eq!(storage.verify_audit_chain().verified_count, 0);
    }

    #[tokio::test]
    async fn candidate_audit_requires_sqlite_commit_and_preserves_gap_free_chain() {
        let (_dir, storage) = disk();
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        let first = record();
        audit.record(first.clone()).await.unwrap();
        assert_commit_unavailable(audit.record(first).await.unwrap_err());
        audit.record(record()).await.unwrap();
        let chain = storage.verify_audit_chain();
        assert!(chain.ok);
        assert_eq!(chain.verified_count, 2);
        assert_eq!(chain.last_seq, Some(1));
        storage.conn.retained_write_lock().run(|conn| conn.execute_batch("CREATE TRIGGER deny_candidate_audit BEFORE INSERT ON audit_log BEGIN SELECT RAISE(FAIL, 'test failure'); END;")).unwrap();
        assert_commit_unavailable(audit.record(record()).await.unwrap_err());
        storage
            .conn
            .retained_write_lock()
            .run(|conn| conn.execute_batch("DROP TRIGGER deny_candidate_audit;"))
            .unwrap();
        audit.record(record()).await.unwrap();
        assert!(storage.verify_audit_chain().ok);
        assert_eq!(storage.verify_audit_chain().last_seq, Some(2));
    }

    #[tokio::test]
    async fn candidate_audit_tip_read_failure_is_not_treated_as_genesis() {
        let (_dir, storage) = disk();
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        // A deliberately malformed tip makes query_row conversion fail; the old
        // `.ok()` path would silently append a new genesis link.
        storage.conn.retained_write_lock().run(|conn| conn.execute_batch("INSERT INTO audit_log (entry_id,timestamp,session_id,command_id,action_type,status,seq,prev_hash,entry_hash) VALUES ('broken','2026-01-01','test','test','test','Started',0,'previous',NULL);")).unwrap();
        assert_commit_unavailable(audit.record(record()).await.unwrap_err());
    }

    #[tokio::test]
    async fn candidate_audit_never_acknowledges_an_uncommitted_or_truncated_record() {
        use maekon_core::models::candidate_decision::{DecisionAttempt, DecisionUnavailable};
        let (_dir, storage) = disk();
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        storage
            .conn
            .retained_write_lock()
            .run(|conn| conn.execute_batch("BEGIN;"))
            .unwrap();
        assert_commit_unavailable(audit.record(record()).await.unwrap_err());
        storage
            .conn
            .retained_write_lock()
            .run(|conn| conn.execute_batch("ROLLBACK;"))
            .unwrap();
        let mut invalid = record();
        invalid.phase = DecisionAuditPhase::AfterSend;
        invalid.attempt = Some(DecisionAttempt {
            id: Uuid::new_v4(),
            request_hash: None,
            stage: DecisionStage::Choice,
            attempted: true,
            elapsed_ms: u64::MAX,
            usage: None,
            estimated_cost_microusd: None,
            failure: Some(DecisionUnavailable::Timeout),
        });
        assert_commit_unavailable(audit.record(invalid).await.unwrap_err());
        assert_eq!(storage.verify_audit_chain().verified_count, 0);
        audit.record(record()).await.unwrap();
        assert!(storage.verify_audit_chain().ok);
        assert_eq!(storage.verify_audit_chain().verified_count, 1);
    }

    #[tokio::test]
    async fn candidate_audit_exports_phase_outcomes_and_original_metadata() {
        use maekon_core::models::candidate_decision::{DecisionAttempt, DecisionUnavailable};
        let (_dir, storage) = disk();
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        for (phase, failure, expected) in [
            (DecisionAuditPhase::BeforeSend, None, AuditStatus::Started),
            (DecisionAuditPhase::AfterSend, None, AuditStatus::Completed),
            (
                DecisionAuditPhase::AfterSend,
                Some(DecisionUnavailable::Timeout),
                AuditStatus::Failed,
            ),
            (DecisionAuditPhase::CacheHit, None, AuditStatus::Completed),
        ] {
            let mut entry = record();
            entry.phase = phase;
            if phase == DecisionAuditPhase::AfterSend {
                let attempt_id = Uuid::new_v4();
                entry.attempt_id = Some(attempt_id);
                entry.attempt = Some(DecisionAttempt {
                    request_hash: Some("b".repeat(64)),
                    id: attempt_id,
                    stage: DecisionStage::Choice,
                    attempted: true,
                    elapsed_ms: 12,
                    usage: None,
                    estimated_cost_microusd: None,
                    failure,
                });
            }
            if phase == DecisionAuditPhase::CacheHit {
                entry.cache_source = Some(Uuid::new_v4());
            }
            audit.record(entry.clone()).await.unwrap();
            let stored = storage
                .entries_by_command_id("candidate-decision", 10)
                .into_iter()
                .find(|row| row.entry_id == entry.id.to_string())
                .unwrap();
            assert_eq!(stored.status, expected);
            assert_eq!(stored.session_id, entry.decision_id.to_string());
            assert_eq!(stored.action_type, "candidate_decision.advisory");
            let details: serde_json::Value =
                serde_json::from_str(stored.details.as_deref().unwrap()).unwrap();
            assert_eq!(details["context"]["namespace_hash"], "a".repeat(64));
            assert_eq!(details["context"]["provider"], "typesafe");
            assert_eq!(details["context"]["schema_revision"], DECISION_SCHEMA);
            assert_eq!(
                details["cache_source"],
                serde_json::to_value(entry.cache_source).unwrap()
            );
            if phase == DecisionAuditPhase::AfterSend {
                assert_eq!(stored.execution_time_ms, Some(12));
                assert_eq!(
                    details["attempt_id"],
                    serde_json::to_value(entry.attempt_id).unwrap()
                );
                assert_eq!(
                    details["attempt"]["estimated_cost_microusd"],
                    serde_json::Value::Null
                );
                assert_eq!(
                    details["attempt"]["failure"],
                    serde_json::to_value(failure).unwrap()
                );
            }
        }
        assert!(storage.verify_audit_chain().ok);
        assert_eq!(storage.verify_audit_chain().verified_count, 4);
    }

    #[tokio::test]
    async fn candidate_assessment_audit_commits_native_metadata_without_jev_cost_or_payload() {
        use maekon_core::models::candidate_assessment::{
            AssessmentAuditPhase, AssessmentAuditRecord, AssessmentTransport,
            LocalAssessmentAttempt, ASSESSMENT_SCHEMA,
        };
        use maekon_core::models::candidate_decision_policy::CandidateProvider;
        use maekon_core::ports::candidate_assessment::CandidateAssessmentAuditPort;
        let (_dir, storage) = disk();
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        let attempt = LocalAssessmentAttempt {
            id: Uuid::new_v4(),
            request_hash: Some("d".repeat(64)),
            attempted: true,
            elapsed_ms: 11,
            usage: None,
            failure: None,
        };
        let entry = AssessmentAuditRecord {
            id: Uuid::new_v4(),
            decision_id: Uuid::new_v4(),
            attempt_id: Some(attempt.id),
            phase: AssessmentAuditPhase::AfterAttempt,
            namespace_hash: "a".repeat(64),
            provider: CandidateProvider::LocalRules,
            transport: AssessmentTransport::InProcess,
            schema_revision: ASSESSMENT_SCHEMA,
            attempt: Some(attempt),
            cache_source: None,
        };
        audit.record_assessment(entry.clone()).await.unwrap();
        assert_commit_unavailable(audit.record_assessment(entry.clone()).await.unwrap_err());
        let stored = storage.entries_by_command_id("candidate-assessment", 10);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].entry_id, entry.id.to_string());
        assert_eq!(stored[0].status, AuditStatus::Completed);
        assert_eq!(stored[0].execution_time_ms, Some(11));
        let details: serde_json::Value =
            serde_json::from_str(stored[0].details.as_ref().unwrap()).unwrap();
        assert_eq!(details["provider"], "local_rules");
        assert_eq!(details["transport"], "in_process");
        assert_eq!(details["attempt"]["usage"], serde_json::Value::Null);
        assert!(details["attempt"].get("estimated_cost_microusd").is_none());
        assert!(details.get("prompt").is_none());
        assert!(storage.verify_audit_chain().ok);
        assert_eq!(storage.verify_audit_chain().verified_count, 1);
    }

    #[tokio::test]
    async fn candidate_assessment_audit_preserves_each_phase_outcome() {
        use maekon_core::models::candidate_assessment::{
            AssessmentAuditPhase, AssessmentAuditRecord, AssessmentTransport,
            LocalAssessmentAttempt, ASSESSMENT_SCHEMA,
        };
        use maekon_core::models::candidate_decision::DecisionUnavailable;
        use maekon_core::models::candidate_decision_policy::CandidateProvider;
        use maekon_core::ports::candidate_assessment::CandidateAssessmentAuditPort;
        let (_dir, storage) = disk();
        let audit = SqliteCandidateDecisionAudit::new(storage.clone());
        for (phase, failure, expected) in [
            (
                AssessmentAuditPhase::BeforeAttempt,
                None,
                AuditStatus::Started,
            ),
            (
                AssessmentAuditPhase::AfterAttempt,
                None,
                AuditStatus::Completed,
            ),
            (
                AssessmentAuditPhase::AfterAttempt,
                Some(DecisionUnavailable::Timeout),
                AuditStatus::Failed,
            ),
            (AssessmentAuditPhase::CacheHit, None, AuditStatus::Completed),
        ] {
            let attempt = LocalAssessmentAttempt {
                id: Uuid::new_v4(),
                request_hash: Some("b".repeat(64)),
                attempted: true,
                elapsed_ms: 7,
                usage: None,
                failure,
            };
            let record = AssessmentAuditRecord {
                id: Uuid::new_v4(),
                decision_id: Uuid::new_v4(),
                attempt_id: Some(attempt.id),
                phase,
                namespace_hash: "a".repeat(64),
                provider: CandidateProvider::LocalModel,
                transport: AssessmentTransport::LoopbackHttp,
                schema_revision: ASSESSMENT_SCHEMA,
                attempt: Some(attempt),
                cache_source: (phase == AssessmentAuditPhase::CacheHit).then(Uuid::new_v4),
            };
            audit.record_assessment(record.clone()).await.unwrap();
            let stored = storage
                .entries_by_command_id("candidate-assessment", 10)
                .into_iter()
                .find(|row| row.entry_id == record.id.to_string())
                .unwrap();
            assert_eq!(stored.status, expected);
            assert_eq!(stored.session_id, record.decision_id.to_string());
            assert_eq!(stored.action_type, "candidate_assessment.advisory");
            assert_eq!(stored.execution_time_ms, Some(7));
            let details: serde_json::Value =
                serde_json::from_str(stored.details.as_deref().unwrap()).unwrap();
            assert_eq!(details, serde_json::to_value(record).unwrap());
        }
        assert!(storage.verify_audit_chain().ok);
        assert_eq!(storage.verify_audit_chain().verified_count, 4);
    }
}
