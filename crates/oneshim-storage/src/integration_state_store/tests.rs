use chrono::{Duration, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::integration::{
    InsightPacket, InsightSourceWindow, IntegrationCapabilityScope, IntegrationEnvelope,
    IntegrationInboxItemStatus, IntegrationMessageType, IntegrationOrigin,
    IntegrationOutboundPayload, IntegrationPrivacyClassification, IntegrationPromptReceipt,
    IntegrationPromptReceiptAction, IntegrationSessionStatus, ProactivePrompt,
    ProactivePromptCategory, ProactivePromptPriority, PromptProvenance,
};
use oneshim_core::ports::integration::{
    IntegrationAuditPort, IntegrationCheckpointStorePort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationPromptReceiptStorePort, IntegrationSessionStorePort,
};

use super::*;

fn sample_envelope() -> IntegrationEnvelope {
    IntegrationEnvelope {
        envelope_id: "env-1".to_string(),
        schema_version: "integration.envelope.v1".to_string(),
        message_type: IntegrationMessageType::InsightPacket,
        timestamp: Utc::now(),
        nonce: "nonce-1".to_string(),
        origin: IntegrationOrigin {
            device_id: "device-1".to_string(),
            workspace_id: None,
            session_id: Some("session-1".to_string()),
            source: "desktop-client".to_string(),
        },
        capability_scope: IntegrationCapabilityScope::InsightWrite,
    }
}

fn sample_packet(packet_id: &str) -> InsightPacket {
    InsightPacket {
        packet_id: packet_id.to_string(),
        summary: "summary".to_string(),
        derived_tags: vec!["focus".to_string()],
        source_window: InsightSourceWindow {
            started_at: Utc::now(),
            ended_at: Utc::now(),
        },
        privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
        audit_reference_id: Some("audit-ref-1".to_string()),
    }
}

fn sample_prompt(prompt_id: &str, body: &str) -> StoredProactivePrompt {
    StoredProactivePrompt {
        prompt: ProactivePrompt {
            prompt_id: prompt_id.to_string(),
            category: ProactivePromptCategory::Reminder,
            title: "title".to_string(),
            body: body.to_string(),
            priority: ProactivePromptPriority::Medium,
            actions: Vec::new(),
            expires_at: None,
            provenance: PromptProvenance {
                source_system: "integration-server".to_string(),
                source_actor: None,
                correlation_id: None,
            },
        },
        received_at: Utc::now(),
        status: IntegrationInboxItemStatus::Pending,
        status_updated_at: Utc::now(),
        presented_at: None,
        dismiss_reason: None,
    }
}

#[tokio::test]
async fn integration_session_store_persists_and_clears_state() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let session_store = store.session_store();

    session_store
        .store(IntegrationSessionState {
            session_id: "session-1".to_string(),
            device_id: "device-1".to_string(),
            status: IntegrationSessionStatus::Connected,
            transport_kind: Default::default(),
            auth_scheme: Default::default(),
            connected_at: Some(Utc::now()),
            last_heartbeat_at: Some(Utc::now()),
            requested_scopes: vec![IntegrationCapabilityScope::InsightWrite],
            granted_scopes: vec![IntegrationCapabilityScope::InsightWrite],
            ack_cursors: vec![],
        })
        .await
        .unwrap();

    let reloaded =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let session = reloaded.session_store().load().await.unwrap().unwrap();
    assert_eq!(session.session_id, "session-1");

    reloaded.session_store().clear().await.unwrap();
    assert!(reloaded.session_store().load().await.unwrap().is_none());
}

#[tokio::test]
async fn integration_outbox_store_roundtrips_queue_and_ack_cursor() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let outbox = store.outbox_store();

    let queue_id = outbox
        .enqueue_message(
            sample_envelope(),
            IntegrationOutboundPayload::Insight(sample_packet("packet-1")),
        )
        .await
        .unwrap();
    let items = outbox.list_pending(10).await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].queue_id, queue_id);

    outbox
        .store_ack_cursor(IntegrationAckCursor {
            stream_id: "insights".to_string(),
            cursor: "42".to_string(),
            acknowledged_at: Utc::now(),
        })
        .await
        .unwrap();

    let reloaded =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let outbox = reloaded.outbox_store();
    assert_eq!(outbox.list_pending(10).await.unwrap().len(), 1);
    assert_eq!(
        outbox.last_ack_cursor().await.unwrap().unwrap().cursor,
        "42"
    );

    outbox.delete(&[queue_id]).await.unwrap();
    assert!(outbox.list_pending(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn prompt_receipt_store_updates_inbox_and_outbox_atomically() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let inbox = store.inbox_store();

    inbox
        .upsert_prompts(vec![sample_prompt("prompt-1", "body")])
        .await
        .unwrap();

    let queue_id = inbox
        .record_prompt_receipt(
            "prompt-1",
            IntegrationEnvelope {
                envelope_id: "env-receipt-1".to_string(),
                schema_version: "integration.prompt_receipt.v1".to_string(),
                message_type:
                    oneshim_core::models::integration::IntegrationMessageType::PromptReceipt,
                timestamp: Utc::now(),
                nonce: "nonce-receipt-1".to_string(),
                origin: oneshim_core::models::integration::IntegrationOrigin {
                    device_id: "device-1".to_string(),
                    workspace_id: None,
                    session_id: Some("session-1".to_string()),
                    source: "desktop-client".to_string(),
                },
                capability_scope: IntegrationCapabilityScope::PromptAck,
            },
            IntegrationPromptReceipt {
                receipt_id: "receipt-1".to_string(),
                prompt_id: "prompt-1".to_string(),
                action: IntegrationPromptReceiptAction::Dismissed,
                occurred_at: Utc::now(),
                reason: Some("handled".to_string()),
            },
        )
        .await
        .unwrap();

    let prompt = inbox.list_pending().await.unwrap();
    assert!(prompt.is_empty());

    let outbox = store.outbox_store();
    let queued = outbox.list_pending(10).await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].queue_id, queue_id);
    match &queued[0].payload {
        IntegrationOutboundPayload::PromptReceipt(receipt) => {
            assert_eq!(receipt.prompt_id, "prompt-1");
            assert_eq!(receipt.action, IntegrationPromptReceiptAction::Dismissed);
        }
        IntegrationOutboundPayload::Insight(_) => panic!("expected prompt receipt payload"),
    }
}

#[tokio::test]
async fn prompt_receipt_store_rejects_duplicate_lifecycle_recording() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let inbox = store.inbox_store();

    inbox
        .upsert_prompts(vec![sample_prompt("prompt-1", "body")])
        .await
        .unwrap();

    let envelope = IntegrationEnvelope {
        envelope_id: "env-receipt-1".to_string(),
        schema_version: "integration.prompt_receipt.v1".to_string(),
        message_type: oneshim_core::models::integration::IntegrationMessageType::PromptReceipt,
        timestamp: Utc::now(),
        nonce: "nonce-receipt-1".to_string(),
        origin: oneshim_core::models::integration::IntegrationOrigin {
            device_id: "device-1".to_string(),
            workspace_id: None,
            session_id: Some("session-1".to_string()),
            source: "desktop-client".to_string(),
        },
        capability_scope: IntegrationCapabilityScope::PromptAck,
    };

    inbox
        .record_prompt_receipt(
            "prompt-1",
            envelope.clone(),
            IntegrationPromptReceipt {
                receipt_id: "receipt-1".to_string(),
                prompt_id: "prompt-1".to_string(),
                action: IntegrationPromptReceiptAction::Acknowledged,
                occurred_at: Utc::now(),
                reason: None,
            },
        )
        .await
        .unwrap();

    let err = inbox
        .record_prompt_receipt(
            "prompt-1",
            envelope,
            IntegrationPromptReceipt {
                receipt_id: "receipt-2".to_string(),
                prompt_id: "prompt-1".to_string(),
                action: IntegrationPromptReceiptAction::Dismissed,
                occurred_at: Utc::now(),
                reason: Some("duplicate".to_string()),
            },
        )
        .await
        .expect_err("duplicate prompt receipt should fail");

    assert!(matches!(err, CoreError::Validation { .. }));
}

#[tokio::test]
async fn integration_inbox_store_preserves_lifecycle_and_expires_stale_prompts() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let inbox = store.inbox_store();

    let original = sample_prompt("prompt-1", "body-1");
    inbox.upsert_prompts(vec![original]).await.unwrap();
    inbox
        .update_status("prompt-1", IntegrationInboxItemStatus::Acknowledged, None)
        .await
        .unwrap();
    inbox.mark_presented("prompt-1", Utc::now()).await.unwrap();
    inbox
        .upsert_prompts(vec![sample_prompt("prompt-1", "body-2")])
        .await
        .unwrap();

    assert!(inbox.list_pending().await.unwrap().is_empty());
    assert!(inbox.list_unpresented(10).await.unwrap().is_empty());

    let expiring = StoredProactivePrompt {
        prompt: ProactivePrompt {
            expires_at: Some(Utc::now() - Duration::seconds(1)),
            ..sample_prompt("prompt-2", "body-expiring").prompt
        },
        ..sample_prompt("prompt-2", "body-expiring")
    };
    inbox.upsert_prompts(vec![expiring]).await.unwrap();
    assert_eq!(inbox.expire_stale().await.unwrap(), 1);
    assert!(inbox.list_pending().await.unwrap().is_empty());

    let reloaded =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let prompt = reloaded
        .inbox_store()
        .list_pending()
        .await
        .unwrap()
        .into_iter()
        .find(|prompt| prompt.prompt.prompt_id == "prompt-1");
    assert!(prompt.is_none());
}

#[tokio::test]
async fn integration_inbox_store_redacts_completed_prompt_bodies_by_default() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let inbox = store.inbox_store();

    inbox
        .upsert_prompts(vec![sample_prompt("prompt-redact", "body-secret")])
        .await
        .unwrap();
    inbox
        .update_status("prompt-redact", IntegrationInboxItemStatus::Dismissed, None)
        .await
        .unwrap();

    let registry =
        FileIntegrationStateRegistry::load_or_default(&temp_dir.path().join("integration.json"))
            .unwrap();
    assert_eq!(
        registry
            .inbox
            .get("prompt-redact")
            .map(|prompt| prompt.prompt.body.as_str()),
        Some("")
    );
}

#[tokio::test]
async fn integration_inbox_store_prunes_oldest_completed_prompts_when_retention_limit_exceeded() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::with_policy(
        temp_dir.path().join("integration.json"),
        IntegrationStateStorePolicy {
            max_stored_prompts: 2,
            redact_completed_prompt_bodies: true,
        },
    )
    .unwrap();
    let inbox = store.inbox_store();

    let mut first = sample_prompt("prompt-1", "body-1");
    first.received_at = Utc::now() - Duration::minutes(3);
    first.status = IntegrationInboxItemStatus::Acknowledged;

    let mut second = sample_prompt("prompt-2", "body-2");
    second.received_at = Utc::now() - Duration::minutes(2);
    second.status = IntegrationInboxItemStatus::Dismissed;

    let mut third = sample_prompt("prompt-3", "body-3");
    third.received_at = Utc::now() - Duration::minutes(1);

    inbox
        .upsert_prompts(vec![first, second, third])
        .await
        .unwrap();

    let registry =
        FileIntegrationStateRegistry::load_or_default(&temp_dir.path().join("integration.json"))
            .unwrap();
    assert_eq!(registry.inbox.len(), 2);
    assert!(!registry.inbox.contains_key("prompt-1"));
    assert!(registry.inbox.contains_key("prompt-2"));
    assert!(registry.inbox.contains_key("prompt-3"));
}

#[tokio::test]
async fn integration_audit_store_roundtrips_recent_records() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let audit = store.audit_store();

    audit
        .record_insight_decision(IntegrationInsightAuditRecord {
            record_id: "audit-1".to_string(),
            envelope_id: "env-1".to_string(),
            packet_id: "packet-1".to_string(),
            disposition: oneshim_core::models::integration::IntegrationEgressDisposition::Allow,
            reason: None,
            privacy_classification: IntegrationPrivacyClassification::DerivedSummary,
            capability_scope: IntegrationCapabilityScope::InsightWrite,
            occurred_at: Utc::now(),
        })
        .await
        .unwrap();
    audit
        .record_insight_decision(IntegrationInsightAuditRecord {
            record_id: "audit-2".to_string(),
            envelope_id: "env-2".to_string(),
            packet_id: "packet-2".to_string(),
            disposition: oneshim_core::models::integration::IntegrationEgressDisposition::Deny,
            reason: Some("policy denied".to_string()),
            privacy_classification: IntegrationPrivacyClassification::DeviceLocal,
            capability_scope: IntegrationCapabilityScope::InsightWrite,
            occurred_at: Utc::now(),
        })
        .await
        .unwrap();

    let reloaded =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let recent = reloaded
        .audit_store()
        .recent_insight_decisions(10)
        .await
        .unwrap();
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].record_id, "audit-2");
    assert_eq!(recent[1].record_id, "audit-1");
}

#[tokio::test]
async fn integration_checkpoint_store_roundtrips_namespaced_cursors() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::new(temp_dir.path().join("integration.json")).unwrap();
    let checkpoints = store.checkpoint_store();

    assert_eq!(
        checkpoints
            .load_checkpoint("focus.local_suggestions")
            .await
            .unwrap(),
        None
    );

    checkpoints
        .store_checkpoint("focus.local_suggestions", "42".to_string())
        .await
        .unwrap();
    checkpoints
        .store_checkpoint("focus.other_stream", "cursor-7".to_string())
        .await
        .unwrap();

    assert_eq!(
        checkpoints
            .load_checkpoint("focus.local_suggestions")
            .await
            .unwrap()
            .as_deref(),
        Some("42")
    );
    assert_eq!(
        checkpoints
            .load_checkpoint("focus.other_stream")
            .await
            .unwrap()
            .as_deref(),
        Some("cursor-7")
    );
}
