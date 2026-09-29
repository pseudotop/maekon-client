use super::*;
use maekon_core::models::candidate_decision::{DecisionStage, DecisionUnavailable, DecisionUsage};
use maekon_core::models::candidate_gateway::*;
use tempfile::TempDir;
use uuid::Uuid;

fn disk() -> (TempDir, Arc<SqliteStorage>) {
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap());
    (dir, storage)
}

fn record() -> GatewayAuditRecord {
    GatewayAuditRecord {
        id: Uuid::new_v4(),
        decision_id: Uuid::new_v4(),
        namespace_hash: "a".repeat(64),
        schema: GATEWAY_SCHEMA,
        phase: GatewayAuditPhase::BeforeAttempt,
        attempt: GatewayAttempt {
            id: Uuid::new_v4(),
            stage: DecisionStage::Choice,
            request_hash: None,
            send_state: GatewaySendState::Pending,
            elapsed_ms: 0,
            reserved_picos: 9,
            estimated_cost_picos: None,
            funding: GatewayFundingKind::FreeCredits,
            observation: None,
            failure: None,
        },
    }
}

fn observation() -> GatewayObservation {
    GatewayObservation {
        routing: GatewayRouting {
            response_model: GATEWAY_MODEL.into(),
            original_model_id: GATEWAY_MODEL.into(),
            canonical_slug: GATEWAY_MODEL.into(),
            resolved_provider: "typesafe-ai".into(),
            final_provider: "typesafe-ai".into(),
        },
        usage: Some(DecisionUsage {
            input_tokens: 3,
            output_tokens: 4,
        }),
        costs: GatewayReportedCosts {
            cost: Some("0.000000000007".into()),
            market_cost: Some("0.000000000009".into()),
            surcharge_cost: None,
            gateway_cost: Some("0".into()),
        },
        generation_hash: Some("b".repeat(64)),
    }
}

fn completed() -> GatewayAuditRecord {
    let mut value = record();
    value.phase = GatewayAuditPhase::AfterAttempt;
    value.attempt.send_state = GatewaySendState::Attempted;
    value.attempt.request_hash = Some("c".repeat(64));
    value.attempt.elapsed_ms = 12;
    value.attempt.estimated_cost_picos = Some(7);
    value.attempt.observation = Some(Box::new(observation()));
    value
}

fn assert_unavailable(error: CoreError) {
    let CoreError::Storage { code, message } = error else {
        panic!("expected the closed Gateway audit failure, got {error:?}");
    };
    assert_eq!(code, StorageCode::Failed);
    assert_eq!(message, "Gateway candidate audit commit unavailable");
}

#[tokio::test]
async fn gateway_audit_commits_unique_metadata_and_reopens_a_gap_free_chain() {
    let (dir, storage) = disk();
    let audit = SqliteGatewayCandidateAudit::new(storage.clone());
    let first = record();
    audit.record_gateway(first.clone()).await.unwrap();
    assert_unavailable(audit.record_gateway(first.clone()).await.unwrap_err());
    let rows = storage.entries_by_command_id("candidate-gateway", 10);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.entry_id, first.id.to_string());
    assert_eq!(row.session_id, first.decision_id.to_string());
    assert_eq!(row.action_type, "candidate_gateway.advisory");
    assert_eq!(row.status, AuditStatus::Started);
    assert_eq!(row.execution_time_ms, Some(0));
    let details = row.details.as_deref().unwrap();
    assert!(details.len() < 4096);
    let saved: serde_json::Value = serde_json::from_str(details).unwrap();
    assert_eq!(saved, serde_json::to_value(first).unwrap());
    assert_eq!(saved["attempt"]["send_state"], "pending");
    assert_eq!(saved["attempt"]["observation"], serde_json::Value::Null);
    assert_eq!(
        saved["attempt"]["estimated_cost_picos"],
        serde_json::Value::Null
    );

    storage
        .conn
        .test_lock()
        .execute_batch(
            "CREATE TRIGGER deny_gateway BEFORE INSERT ON audit_log BEGIN
         SELECT RAISE(ABORT, 'private untrusted SQL text'); END;",
        )
        .unwrap();
    assert_unavailable(audit.record_gateway(record()).await.unwrap_err());
    storage
        .conn
        .test_lock()
        .execute_batch("DROP TRIGGER deny_gateway")
        .unwrap();
    audit.record_gateway(completed()).await.unwrap();
    drop(audit);
    drop(storage);
    let reopened = SqliteStorage::open(&dir.path().join("audit.db"), 30, None).unwrap();
    let chain = reopened.verify_audit_chain();
    assert!(chain.ok);
    assert_eq!(chain.verified_count, 2);
    assert_eq!(chain.last_seq, Some(1));
    assert_eq!(
        reopened
            .entries_by_command_id("candidate-gateway", 10)
            .len(),
        2
    );
}

#[tokio::test]
async fn gateway_audit_observes_synced_settings_and_restores_the_connection() {
    let (_dir, storage) = disk();
    {
        let conn = storage.conn.test_lock();
        conn.pragma_update(Some("main"), "synchronous", 0).unwrap();
        conn.pragma_update(None, "fullfsync", 0).unwrap();
        conn.execute_batch(
            "CREATE TABLE sync_probe(sync INTEGER, full INTEGER);
             CREATE TRIGGER observe_gateway BEFORE INSERT ON audit_log BEGIN
             INSERT INTO sync_probe SELECT synchronous, fullfsync
             FROM pragma_synchronous, pragma_fullfsync; END;",
        )
        .unwrap();
    }
    SqliteGatewayCandidateAudit::new(storage.clone())
        .record_gateway(record())
        .await
        .unwrap();
    let conn = storage.conn.test_lock();
    let observed: (i64, i64) = conn
        .query_row("SELECT sync, full FROM sync_probe", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(observed, (2, 1));
    let restored: (i64, i64) = (
        conn.pragma_query_value(Some("main"), "synchronous", |row| row.get(0))
            .unwrap(),
        conn.pragma_query_value(None, "fullfsync", |row| row.get(0))
            .unwrap(),
    );
    assert_eq!(restored, (0, 0));
    assert!(conn.is_autocommit());
}

#[tokio::test]
async fn gateway_audit_rejects_memory_pending_transactions_and_duration_overflow() {
    let memory = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    assert_unavailable(
        SqliteGatewayCandidateAudit::new(memory.clone())
            .record_gateway(record())
            .await
            .unwrap_err(),
    );
    assert_eq!(memory.verify_audit_chain().verified_count, 0);

    let (_dir, storage) = disk();
    let audit = SqliteGatewayCandidateAudit::new(storage.clone());
    storage.conn.test_lock().execute_batch("BEGIN").unwrap();
    assert_unavailable(audit.record_gateway(record()).await.unwrap_err());
    assert!(!storage.conn.test_lock().is_autocommit());
    storage.conn.test_lock().execute_batch("ROLLBACK").unwrap();
    let mut value = completed();
    value.attempt.elapsed_ms = u64::MAX;
    assert_unavailable(audit.record_gateway(value).await.unwrap_err());
    assert_eq!(storage.verify_audit_chain().verified_count, 0);
    audit.record_gateway(record()).await.unwrap();
    assert_eq!(storage.verify_audit_chain().verified_count, 1);
}

#[tokio::test]
async fn gateway_audit_preserves_outcomes_funding_and_unknown_failure_evidence() {
    let (_dir, storage) = disk();
    let audit = SqliteGatewayCandidateAudit::new(storage.clone());
    for funding in [
        GatewayFundingKind::Promotion,
        GatewayFundingKind::FreeCredits,
        GatewayFundingKind::PaidApi,
    ] {
        for stage in [DecisionStage::Choice, DecisionStage::Suitability] {
            for outcome in 0..4 {
                let mut value = completed();
                value.attempt.funding = funding;
                value.attempt.stage = stage;
                let expected = if outcome == 0 {
                    AuditStatus::Completed
                } else {
                    AuditStatus::Failed
                };
                if outcome != 0 {
                    value.attempt.failure = Some(DecisionUnavailable::Timeout);
                }
                if outcome >= 2 {
                    value.attempt.observation = None;
                    value.attempt.estimated_cost_picos = None;
                    value.attempt.request_hash = None;
                }
                if outcome == 3 {
                    value.attempt.send_state = GatewaySendState::NotSent;
                }
                audit.record_gateway(value.clone()).await.unwrap();
                let saved = storage
                    .entries_by_command_id("candidate-gateway", 100)
                    .into_iter()
                    .find(|row| row.entry_id == value.id.to_string())
                    .unwrap();
                assert_eq!(saved.status, expected);
                assert_eq!(saved.execution_time_ms, Some(12));
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(saved.details.as_deref().unwrap())
                        .unwrap(),
                    serde_json::to_value(value).unwrap()
                );
            }
        }
    }
    let chain = storage.verify_audit_chain();
    assert!(chain.ok);
    assert_eq!(chain.verified_count, 24);
}

#[tokio::test]
async fn gateway_audit_keeps_real_overrun_and_missing_cost_distinct_from_zero() {
    let (_dir, storage) = disk();
    let audit = SqliteGatewayCandidateAudit::new(storage.clone());
    let mut overrun = completed();
    overrun.attempt.estimated_cost_picos = Some(u64::MAX);
    overrun.attempt.observation.as_mut().unwrap().costs.cost = Some("18446744.073709551615".into());
    overrun.attempt.failure = Some(DecisionUnavailable::BudgetExceeded);
    audit.record_gateway(overrun.clone()).await.unwrap();
    let saved = storage.entries_by_command_id("candidate-gateway", 10);
    assert_eq!(saved[0].status, AuditStatus::Failed);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(saved[0].details.as_deref().unwrap()).unwrap(),
        serde_json::to_value(overrun).unwrap()
    );

    let mut unknown = completed();
    unknown.attempt.failure = Some(DecisionUnavailable::InvalidResponse);
    unknown.attempt.estimated_cost_picos = None;
    let observed = unknown.attempt.observation.as_mut().unwrap();
    observed.usage = None;
    observed.costs = GatewayReportedCosts::default();
    observed.generation_hash = None;
    audit.record_gateway(unknown.clone()).await.unwrap();
    let saved = storage
        .entries_by_command_id("candidate-gateway", 10)
        .into_iter()
        .find(|row| row.entry_id == unknown.id.to_string())
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(saved.details.as_deref().unwrap()).unwrap(),
        serde_json::to_value(unknown).unwrap()
    );
}

#[tokio::test]
async fn gateway_audit_rejects_unbounded_or_invalid_metadata_before_storage() {
    let (_dir, storage) = disk();
    let audit = SqliteGatewayCandidateAudit::new(storage.clone());
    let invalid: &[fn(&mut GatewayAuditRecord)] = &[
        |v| v.schema = "private-schema",
        |v| v.id = Uuid::nil(),
        |v| v.decision_id = Uuid::nil(),
        |v| v.attempt.id = Uuid::nil(),
        |v| v.namespace_hash = "a".repeat(63),
        |v| v.namespace_hash = "A".repeat(64),
        |v| v.namespace_hash = "private".repeat(10000),
        |v| v.attempt.request_hash = Some("private-request".into()),
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .routing
                .response_model = "private".into()
        },
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .routing
                .original_model_id = "private".into()
        },
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .routing
                .canonical_slug = "private".into()
        },
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .routing
                .resolved_provider = "private".into()
        },
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .routing
                .final_provider = "private".into()
        },
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .usage
                .as_mut()
                .unwrap()
                .input_tokens = GATEWAY_MAX_TOKENS + 1
        },
        |v| {
            v.attempt
                .observation
                .as_mut()
                .unwrap()
                .usage
                .as_mut()
                .unwrap()
                .output_tokens = GATEWAY_MAX_TOKENS + 1
        },
        |v| v.attempt.observation.as_mut().unwrap().costs.cost = Some("private".into()),
        |v| v.attempt.observation.as_mut().unwrap().costs.market_cost = Some("-1".into()),
        |v| v.attempt.observation.as_mut().unwrap().costs.surcharge_cost = Some("1e2".into()),
        |v| {
            v.attempt.observation.as_mut().unwrap().costs.gateway_cost =
                Some("0.0000000000001".into())
        },
        |v| {
            v.attempt.observation.as_mut().unwrap().generation_hash =
                Some("raw-generation-id".into())
        },
    ];
    for corrupt in invalid {
        let mut value = completed();
        corrupt(&mut value);
        assert_unavailable(audit.record_gateway(value).await.unwrap_err());
    }
    assert_eq!(storage.verify_audit_chain().verified_count, 0);
    audit.record_gateway(completed()).await.unwrap();
    assert_eq!(storage.verify_audit_chain().verified_count, 1);
}

#[tokio::test]
async fn gateway_audit_rejects_contradictory_phases_and_preserves_prepared_not_sent_hash() {
    let (_dir, storage) = disk();
    let audit = SqliteGatewayCandidateAudit::new(storage.clone());
    let invalid_before: &[fn(&mut GatewayAuditRecord)] = &[
        |v| v.attempt.send_state = GatewaySendState::NotSent,
        |v| v.attempt.send_state = GatewaySendState::Attempted,
        |v| v.attempt.elapsed_ms = 1,
        |v| v.attempt.request_hash = Some("c".repeat(64)),
        |v| v.attempt.observation = Some(Box::new(observation())),
        |v| v.attempt.estimated_cost_picos = Some(0),
        |v| v.attempt.failure = Some(DecisionUnavailable::Cancelled),
    ];
    for corrupt in invalid_before {
        let mut value = record();
        corrupt(&mut value);
        assert_unavailable(audit.record_gateway(value).await.unwrap_err());
    }
    let invalid_after: &[fn(&mut GatewayAuditRecord)] = &[
        |v| v.attempt.send_state = GatewaySendState::Pending,
        |v| v.attempt.request_hash = None,
        |v| v.attempt.observation = None,
        |v| v.attempt.estimated_cost_picos = None,
    ];
    for corrupt in invalid_after {
        let mut value = completed();
        corrupt(&mut value);
        assert_unavailable(audit.record_gateway(value).await.unwrap_err());
    }
    let mut not_sent = completed();
    not_sent.attempt.send_state = GatewaySendState::NotSent;
    not_sent.attempt.observation = None;
    not_sent.attempt.estimated_cost_picos = None;
    let invalid_not_sent: &[fn(&mut GatewayAuditRecord)] = &[
        |v| v.attempt.failure = None,
        |v| v.attempt.observation = Some(Box::new(observation())),
        |v| v.attempt.estimated_cost_picos = Some(0),
    ];
    not_sent.attempt.failure = Some(DecisionUnavailable::ConsentOrPolicyDenied);
    for corrupt in invalid_not_sent {
        let mut value = not_sent.clone();
        corrupt(&mut value);
        assert_unavailable(audit.record_gateway(value).await.unwrap_err());
    }
    assert_eq!(storage.verify_audit_chain().verified_count, 0);
    audit.record_gateway(not_sent).await.unwrap();
    assert_eq!(storage.verify_audit_chain().verified_count, 1);
}
