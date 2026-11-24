//! Batch event upload port — server synchronization abstraction.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::CoreError;
use crate::models::event::Event;

/// Private storage/queue proof format. Unsupported formats remain unknown.
pub const UPLOAD_PROVENANCE_VERSION: u8 = 1;

/// Non-content evidence about a title before capture/egress masking.
///
/// This is stored separately from the public `Event`, never in an upload batch.
/// The fingerprint binds the stored primary key AND the masked event, so proof
/// from a different row or payload cannot authorize an upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadProvenance {
    pub version: u8,
    pub title_policy_fingerprint: [u8; 32],
    pub title_information_lost: bool,
    pub title_allowed: bool,
    pub event_fingerprint: [u8; 32],
}

impl UploadProvenance {
    pub fn new(
        storage_id: &str,
        event: &Event,
        title_policy_fingerprint: [u8; 32],
        title_information_lost: bool,
        title_allowed: bool,
    ) -> Result<Self, CoreError> {
        Ok(Self {
            version: UPLOAD_PROVENANCE_VERSION,
            title_policy_fingerprint,
            title_information_lost,
            title_allowed,
            event_fingerprint: Self::fingerprint(storage_id, event)?,
        })
    }

    pub fn is_bound_to(&self, storage_id: &str, event: &Event) -> bool {
        self.version == UPLOAD_PROVENANCE_VERSION
            && Self::fingerprint(storage_id, event)
                .is_ok_and(|fingerprint| fingerprint == self.event_fingerprint)
    }

    fn fingerprint(storage_id: &str, event: &Event) -> Result<[u8; 32], CoreError> {
        // Sort every object, including nested System.data maps, independently
        // of serde_json's preserve_order feature and insertion order.
        fn canonicalize(value: serde_json::Value) -> serde_json::Value {
            match value {
                serde_json::Value::Object(object) => {
                    let mut entries: Vec<_> = object.into_iter().collect();
                    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
                    serde_json::Value::Object(
                        entries
                            .into_iter()
                            .map(|(key, value)| (key, canonicalize(value)))
                            .collect(),
                    )
                }
                serde_json::Value::Array(values) => {
                    serde_json::Value::Array(values.into_iter().map(canonicalize).collect())
                }
                other => other,
            }
        }
        let event = canonicalize(serde_json::to_value(event)?);
        let bytes = serde_json::to_vec(&event)?;
        let mut digest = Sha256::new();
        digest.update(b"maekon-upload-provenance-v1\0");
        digest.update((storage_id.len() as u64).to_le_bytes());
        digest.update(storage_id.as_bytes());
        digest.update(bytes);
        Ok(digest.finalize().into())
    }
}

/// A durable pending row. A legacy adapter without an exact persisted key or
/// provenance must leave those fields unknown, never invent either value.
#[derive(Debug, Clone)]
pub struct PendingEvent {
    pub storage_id: Option<String>,
    pub event: Event,
    pub provenance: Option<UploadProvenance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadDecision {
    Allow,
    Block,
    Defer,
}

/// Evaluate a temporary payload clone using its original storage identity and
/// historical evidence. Queue/transport integration follows separately.
pub trait UploadPolicy: Send + Sync {
    fn evaluate(
        &self,
        storage_id: &str,
        event: &mut Event,
        provenance: Option<&UploadProvenance>,
    ) -> UploadDecision;
}

impl<F> UploadPolicy for F
where
    F: Fn(&str, &mut Event, Option<&UploadProvenance>) -> UploadDecision + Send + Sync,
{
    fn evaluate(
        &self,
        storage_id: &str,
        event: &mut Event,
        provenance: Option<&UploadProvenance>,
    ) -> UploadDecision {
        self(storage_id, event, provenance)
    }
}

/// A queued upload item: the (possibly egress-filtered) event payload paired
/// with the STORAGE id of the original persisted event (#7946).
///
/// The two must travel together because the storage id is derived from the
/// original event's content at persist time, while the uploaded payload may
/// have been transformed by the egress policy (PII masking / title redaction)
/// — deriving an id from the payload would stamp the wrong row as sent.
/// Producers compute `storage_id` from the ORIGINAL event (the same value the
/// persistence layer derives) before handing the event to the egress policy.
#[derive(Debug, Clone)]
pub struct QueuedUpload {
    /// Storage primary key of the persisted original event.
    pub storage_id: String,
    /// The upload payload (post egress-policy transformation).
    pub event: Event,
}

/// Port for uploading events to the server in batches.
/// Implemented by `maekon-network::BatchUploader`.
///
/// # Errors
/// - `CoreError::Network` (wire: `network.*`) — connection failure,
///   DNS error, refused connection (from `ApiClient::upload_batch`).
/// - `CoreError::RequestTimeout` (wire: `network.timeout`) — flush
///   exceeding configured upload timeout.
/// - `CoreError::RateLimit` (wire: `network.rate_limit`) — server 429
///   surfaced by the inner HTTP client.
/// - `CoreError::ServiceUnavailable` (wire: `service.unavailable`) —
///   5xx class responses.
/// - `enqueue` / `enqueue_many` are infallible by contract; queue
///   overflow is silently dropped and surfaced via
///   `take_dropped_since_last()` rather than an error variant.
#[async_trait]
pub trait BatchSink: Send + Sync {
    /// Add an event to the upload queue.
    fn enqueue(&self, item: QueuedUpload);

    /// Add multiple events to the upload queue.
    fn enqueue_many(&self, items: Vec<QueuedUpload>);

    /// Flush queued events to the server. Returns the storage ids of the
    /// events that were confirmed uploaded in this flush (#7946) — the caller
    /// marks exactly these rows as sent, never a time-based bulk stamp.
    async fn flush(&self) -> Result<Vec<String>, CoreError>;

    /// Return the number of events dropped since the last call and reset the counter.
    fn take_dropped_since_last(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::event::{SystemEvent, SystemEventType};

    #[test]
    fn provenance_binds_identity_and_canonical_nested_payload() {
        let mut event = Event::System(SystemEvent {
            event_id: uuid::Uuid::new_v4(),
            event_type: SystemEventType::MetricsUpdate,
            timestamp: chrono::Utc::now(),
            data: serde_json::from_str(r#"{"z":[{"b":2,"a":1}],"a":{"y":4,"x":3}}"#).unwrap(),
        });
        let proof = UploadProvenance::new("persisted-id", &event, [7; 32], true, true).unwrap();
        if let Event::System(system) = &mut event {
            system.data =
                serde_json::from_str(r#"{"a":{"x":3,"y":4},"z":[{"a":1,"b":2}]}"#).unwrap();
        }
        assert!(proof.is_bound_to("persisted-id", &event));
        assert!(!proof.is_bound_to("other-id", &event));
        if let Event::System(system) = &mut event {
            system.data["z"][0]["a"] = serde_json::json!(99);
        }
        assert!(!proof.is_bound_to("persisted-id", &event));
    }

    #[test]
    fn provenance_unknown_version_is_not_trusted() {
        let event = Event::Context(Default::default());
        let mut proof = UploadProvenance::new("id", &event, [0; 32], false, true).unwrap();
        proof.version += 1;
        assert!(!proof.is_bound_to("id", &event));
    }

    #[test]
    fn policy_closure_delegates_identity_evidence_payload_and_every_decision() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let original = Event::Context(Default::default());
        let proof = UploadProvenance::new("persisted-id", &original, [9; 32], true, true)
            .expect("bound fixture");
        for decision in [
            UploadDecision::Allow,
            UploadDecision::Block,
            UploadDecision::Defer,
        ] {
            for evidence in [Some(&proof), None] {
                let calls = AtomicUsize::new(0);
                let delegate =
                    |storage_id: &str, event: &mut Event, supplied: Option<&UploadProvenance>| {
                        assert_eq!(storage_id, "persisted-id");
                        assert_eq!(supplied, evidence);
                        calls.fetch_add(1, Ordering::SeqCst);
                        let Event::Context(context) = event else {
                            panic!("context payload")
                        };
                        context.window_title = "delegate-mutated".into();
                        decision
                    };
                let policy: &dyn UploadPolicy = &delegate;
                let mut payload = original.clone();
                assert_eq!(
                    policy.evaluate("persisted-id", &mut payload, evidence),
                    decision
                );
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                let Event::Context(context) = payload else {
                    panic!("context payload")
                };
                assert_eq!(context.window_title, "delegate-mutated");
            }
        }
    }
}
