use chrono::{Duration, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::integration::{
    InsightPacket, InsightSourceWindow, IntegrationCapabilityScope, IntegrationEnvelope,
    IntegrationInboxItemStatus, IntegrationMessageType, IntegrationOrigin,
    IntegrationOutboundPayload, IntegrationPrivacyClassification, IntegrationPromptReceipt,
    IntegrationPromptReceiptAction, IntegrationSessionStatus, ProactivePrompt,
    ProactivePromptCategory, ProactivePromptPriority, PromptProvenance,
};
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationCheckpointStorePort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationPromptReceiptStorePort, IntegrationSessionStorePort,
};

use super::*;
use crate::encryption::EncryptionKey;
use std::sync::Arc;

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
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
    let session = reloaded.session_store().load().await.unwrap().unwrap();
    assert_eq!(session.session_id, "session-1");

    reloaded.session_store().clear().await.unwrap();
    assert!(reloaded.session_store().load().await.unwrap().is_none());
}

#[tokio::test]
async fn integration_outbox_store_roundtrips_queue_and_ack_cursor() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
                    maekon_core::models::integration::IntegrationMessageType::PromptReceipt,
                timestamp: Utc::now(),
                nonce: "nonce-receipt-1".to_string(),
                origin: maekon_core::models::integration::IntegrationOrigin {
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
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
    let inbox = store.inbox_store();

    inbox
        .upsert_prompts(vec![sample_prompt("prompt-1", "body")])
        .await
        .unwrap();

    let envelope = IntegrationEnvelope {
        envelope_id: "env-receipt-1".to_string(),
        schema_version: "integration.prompt_receipt.v1".to_string(),
        message_type: maekon_core::models::integration::IntegrationMessageType::PromptReceipt,
        timestamp: Utc::now(),
        nonce: "nonce-receipt-1".to_string(),
        origin: maekon_core::models::integration::IntegrationOrigin {
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
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
    let inbox = store.inbox_store();

    inbox
        .upsert_prompts(vec![sample_prompt("prompt-redact", "body-secret")])
        .await
        .unwrap();
    inbox
        .update_status("prompt-redact", IntegrationInboxItemStatus::Dismissed, None)
        .await
        .unwrap();

    let registry = FileIntegrationStateRegistry::load_or_default(
        &temp_dir.path().join("integration.json"),
        None,
    )
    .unwrap();
    assert_eq!(
        registry
            .inbox
            .get("prompt-redact")
            .map(|prompt| prompt.prompt.body.as_str()),
        Some("")
    );
}

/// #6241: re-upserting a prompt that is already in a completed (redacted) state
/// must not resurrect the prompt body in plaintext at rest. The upsert update
/// branch overwrites the stored body with the inbound one, so it must re-apply
/// the at-rest redaction against the preserved lifecycle status.
#[tokio::test]
async fn integration_inbox_store_upsert_does_not_resurrect_redacted_completed_body() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
    let inbox = store.inbox_store();

    // Store a prompt, then complete it so its body is redacted at rest.
    inbox
        .upsert_prompts(vec![sample_prompt("prompt-resurrect", "body-secret")])
        .await
        .unwrap();
    inbox
        .update_status(
            "prompt-resurrect",
            IntegrationInboxItemStatus::Dismissed,
            None,
        )
        .await
        .unwrap();

    // A subsequent upsert carrying the plaintext body again (e.g. a server
    // re-delivery) must not resurrect the body — the completed lifecycle status
    // is preserved and redaction re-applied.
    inbox
        .upsert_prompts(vec![sample_prompt("prompt-resurrect", "body-secret")])
        .await
        .unwrap();

    let registry = FileIntegrationStateRegistry::load_or_default(
        &temp_dir.path().join("integration.json"),
        None,
    )
    .unwrap();
    let stored = registry
        .inbox
        .get("prompt-resurrect")
        .expect("prompt must still be present");
    assert_eq!(
        stored.status,
        IntegrationInboxItemStatus::Dismissed,
        "lifecycle status must be preserved across upsert"
    );
    assert_eq!(
        stored.prompt.body.as_str(),
        "",
        "completed-prompt body must stay redacted at rest after re-upsert"
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
            ..Default::default()
        },
        None,
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

    let registry = FileIntegrationStateRegistry::load_or_default(
        &temp_dir.path().join("integration.json"),
        None,
    )
    .unwrap();
    assert_eq!(registry.inbox.len(), 2);
    assert!(!registry.inbox.contains_key("prompt-1"));
    assert!(registry.inbox.contains_key("prompt-2"));
    assert!(registry.inbox.contains_key("prompt-3"));
}

#[tokio::test]
async fn integration_outbox_store_prunes_oldest_messages_at_the_cap() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = FileIntegrationStateStore::with_policy(
        temp_dir.path().join("integration.json"),
        IntegrationStateStorePolicy {
            max_outbox_messages: 3,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    let outbox = store.outbox_store();

    // Enqueue more than the cap directly at the store layer (this path bypasses
    // the egress coordinator caps, so the store-level cap is the only bound).
    let mut queue_ids = Vec::new();
    for index in 0..5 {
        let queue_id = outbox
            .enqueue_message(
                sample_envelope(),
                IntegrationOutboundPayload::Insight(sample_packet(&format!("packet-{index}"))),
            )
            .await
            .unwrap();
        queue_ids.push(queue_id);
    }

    // Only the most recent `max_outbox_messages` survive; the two oldest are
    // dropped (FIFO drop-oldest), keeping the persisted outbox bounded.
    let pending = outbox.list_pending(100).await.unwrap();
    assert_eq!(pending.len(), 3);
    let surviving_ids: Vec<_> = pending.iter().map(|item| item.queue_id.clone()).collect();
    assert!(!surviving_ids.contains(&queue_ids[0]));
    assert!(!surviving_ids.contains(&queue_ids[1]));
    assert!(surviving_ids.contains(&queue_ids[2]));
    assert!(surviving_ids.contains(&queue_ids[3]));
    assert!(surviving_ids.contains(&queue_ids[4]));

    // The cap is enforced at rest as well (after reload from disk).
    let registry = FileIntegrationStateRegistry::load_or_default(
        &temp_dir.path().join("integration.json"),
        None,
    )
    .unwrap();
    assert_eq!(registry.outbox.len(), 3);
}

#[tokio::test]
async fn integration_audit_store_roundtrips_recent_records() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
    let audit = store.audit_store();

    audit
        .record_insight_decision(IntegrationInsightAuditRecord {
            record_id: "audit-1".to_string(),
            envelope_id: "env-1".to_string(),
            packet_id: "packet-1".to_string(),
            disposition: maekon_core::models::integration::IntegrationEgressDisposition::Allow,
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
            disposition: maekon_core::models::integration::IntegrationEgressDisposition::Deny,
            reason: Some("policy denied".to_string()),
            privacy_classification: IntegrationPrivacyClassification::DeviceLocal,
            capability_scope: IntegrationCapabilityScope::InsightWrite,
            occurred_at: Utc::now(),
        })
        .await
        .unwrap();

    let reloaded =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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
    let store =
        FileIntegrationStateStore::new(temp_dir.path().join("integration.json"), None).unwrap();
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

/// #6102-6: the persisted registry must be owner-only (mode 0o600) at rest — it
/// can carry integration session/insight metadata, so it must not be
/// world-readable like a default-umask file.
#[cfg(unix)]
#[tokio::test]
async fn integration_registry_file_is_owner_only_0o600() {
    use std::os::unix::fs::PermissionsExt;

    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("integration.json");
    let store = FileIntegrationStateStore::new(path.clone(), None).unwrap();
    // Trigger a persist.
    store
        .session_store()
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

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "integration registry must be persisted owner-only (0o600), got {mode:o}"
    );
}

// ── #7073: at-rest AES-256-GCM encryption (mirrors FileSecretRegistry) ────────

fn enc_key(fill: u8) -> Arc<EncryptionKey> {
    Arc::new(EncryptionKey::from_bytes([fill; 32]))
}

/// #7073 (MS-002): with an encryption key the persisted registry must be an
/// AES-256-GCM blob (MKINT1 header) — the privacy-relevant pending proactive-
/// prompt body must NOT appear in cleartext on disk. Regression: before the fix
/// the registry was persisted as plaintext JSON (protected only by 0o600/DACL).
#[tokio::test]
async fn integration_registry_is_encrypted_at_rest_not_plaintext() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("integration.json");
    let store = FileIntegrationStateStore::new(path.clone(), Some(enc_key(0x42))).unwrap();

    // A Pending prompt retains its body at rest (redaction only fires once the
    // prompt leaves Pending), so this body is exactly what would otherwise leak.
    store
        .inbox_store()
        .upsert_prompts(vec![sample_prompt(
            "prompt-1",
            "SUPER_SECRET_PROMPT_BODY_XYZ",
        )])
        .await
        .unwrap();

    let raw = std::fs::read(&path).expect("registry file must exist after a write");
    assert!(
        raw.starts_with(INTEGRATION_STATE_MAGIC),
        "on-disk registry must start with INTEGRATION_STATE_MAGIC (MKINT1\\n), got first 16 bytes: {:?}",
        &raw[..raw.len().min(16)]
    );
    assert!(
        !raw.windows(b"SUPER_SECRET_PROMPT_BODY_XYZ".len())
            .any(|w| w == b"SUPER_SECRET_PROMPT_BODY_XYZ"),
        "cleartext prompt body must not appear in the encrypted registry file"
    );
}

/// #7073: opening an encrypted registry (MKINT1 header) without a key must fail
/// closed (Err naming the header), not silently return an empty registry.
#[tokio::test]
async fn integration_encrypted_file_without_key_fails_closed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("integration.json");
    {
        let store = FileIntegrationStateStore::new(path.clone(), Some(enc_key(0x42))).unwrap();
        store
            .checkpoint_store()
            .store_checkpoint("ns", "cursor-1".to_string())
            .await
            .unwrap();
    }
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(INTEGRATION_STATE_MAGIC),
        "pre-condition: file must be encrypted"
    );

    // `let-else` (not `expect_err`) because the Ok type FileIntegrationStateStore
    // is intentionally not `Debug` (it must never format its at-rest contents).
    let Err(err) = FileIntegrationStateStore::new(path.clone(), None) else {
        panic!("opening an encrypted registry without a key must fail closed, not return Ok");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("MKINT1"),
        "error must mention the MKINT1 header; got: {msg}"
    );
}

/// #7073: opening an encrypted registry with the WRONG key must fail (AES-GCM
/// auth-tag mismatch), confirming wrong-key access is rejected.
#[tokio::test]
async fn integration_wrong_key_fails_closed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("integration.json");
    {
        let store = FileIntegrationStateStore::new(path.clone(), Some(enc_key(0xAA))).unwrap();
        store
            .checkpoint_store()
            .store_checkpoint("ns", "cursor-1".to_string())
            .await
            .unwrap();
    }

    // `let-else` (not `expect_err`) because FileIntegrationStateStore is intentionally not `Debug`.
    let Err(err) = FileIntegrationStateStore::new(path.clone(), Some(enc_key(0xBB))) else {
        panic!("opening an encrypted registry with the wrong key must fail closed, not return Ok");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("decrypt"),
        "error must mention decryption failure; got: {msg}"
    );
}

/// #7073: a legacy plaintext registry (no MKINT1 header) must load transparently,
/// and the next save with a key present must upgrade it to the encrypted format
/// while preserving the legacy data.
#[tokio::test]
async fn integration_legacy_plaintext_is_migrated_to_encrypted() {
    let temp_dir = tempfile::tempdir().unwrap();
    let path = temp_dir.path().join("integration.json");

    // Write a legacy plaintext registry directly (bypassing the store).
    let mut legacy = FileIntegrationStateRegistry::new();
    legacy
        .producer_checkpoints
        .insert("ns".to_string(), "cursor-legacy".to_string());
    std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap())
        .expect("write legacy plaintext registry");
    assert!(
        !std::fs::read(&path)
            .unwrap()
            .starts_with(INTEGRATION_STATE_MAGIC),
        "pre-condition: legacy file must not have the magic header"
    );

    // Open with a key — the legacy plaintext file must load, exposing its data.
    let store = FileIntegrationStateStore::new(path.clone(), Some(enc_key(0x42))).unwrap();
    assert_eq!(
        store
            .checkpoint_store()
            .load_checkpoint("ns")
            .await
            .unwrap()
            .as_deref(),
        Some("cursor-legacy"),
        "legacy value must be retrievable after transparent plaintext load"
    );

    // A new write triggers the encrypted save (auto-upgrade).
    store
        .checkpoint_store()
        .store_checkpoint("ns2", "cursor-new".to_string())
        .await
        .unwrap();
    assert!(
        std::fs::read(&path)
            .unwrap()
            .starts_with(INTEGRATION_STATE_MAGIC),
        "file must be upgraded to the encrypted format after the first keyed write"
    );

    // A fresh instance (same key) must surface both the legacy and new values.
    let reopened = FileIntegrationStateStore::new(path, Some(enc_key(0x42))).unwrap();
    assert_eq!(
        reopened
            .checkpoint_store()
            .load_checkpoint("ns")
            .await
            .unwrap()
            .as_deref(),
        Some("cursor-legacy"),
        "legacy value must survive the encrypted upgrade"
    );
    assert_eq!(
        reopened
            .checkpoint_store()
            .load_checkpoint("ns2")
            .await
            .unwrap()
            .as_deref(),
        Some("cursor-new"),
        "value stored after upgrade must be retrievable"
    );
}
