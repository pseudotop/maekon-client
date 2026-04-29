//! TDD-red tests for `BatchUploader::with_suppression_predicate` (Plan §3.3 A.10).
//!
//! A.11 will implement the real suppression storage + flush gate; A.12 will
//! wire it through the composition root. Until then the stub builder returns
//! `self` unchanged, so the predicate is completely ignored during flush.
//!
//! # Per-test state at A.10
//!
//! | Test | Expected at A.10 | Reason |
//! |------|-----------------|--------|
//! | `flush_returns_zero_when_suppression_predicate_true`  | **RED** (FAILS)  | Stub ignores `|| true`; flush drains 3 events, `assert_eq!(sent, 0)` fails |
//! | `flush_drains_when_suppression_predicate_false`       | **GREEN** (PASSES) | Stub ignores `|| false`; flush drains normally — expected behaviour happens to match |
//! | `predicate_reads_latest_config`                       | **RED** (FAILS)  | Stub ignores predicate; flush runs even when shared flag says "suppress" |
//!
//! A.11 greens all three tests by wiring the predicate into the flush gate.

use oneshim_core::models::event::{ContextEvent, Event};
use oneshim_core::{
    error::CoreError,
    models::{event::EventBatch, frame::ContextUpload, suggestion::SuggestionFeedback},
    ports::api_client::{ApiClient, SessionCreateResponse},
};
use oneshim_network::batch_uploader::BatchUploader;
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// Test-local mock API client (always succeeds — no network calls made)
// ---------------------------------------------------------------------------

struct AlwaysOkApiClient;

#[async_trait::async_trait]
impl ApiClient for AlwaysOkApiClient {
    async fn create_session(&self, client_id: &str) -> Result<SessionCreateResponse, CoreError> {
        Ok(SessionCreateResponse {
            session_id: format!("sess_{client_id}"),
            user_id: "u1".to_string(),
            client_id: client_id.to_string(),
            capabilities: vec![],
        })
    }

    async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
        Ok(())
    }

    async fn upload_context(&self, _upload: &ContextUpload) -> Result<(), CoreError> {
        Ok(())
    }

    async fn send_feedback(&self, _feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        Ok(())
    }

    async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn make_event() -> Event {
    Event::Context(ContextEvent {
        app_name: "test-app".to_string(),
        window_title: "Test Window".to_string(),
        prev_app_name: None,
        timestamp: chrono::Utc::now(),
        ..Default::default()
    })
}

fn make_uploader() -> BatchUploader {
    BatchUploader::new(
        Arc::new(AlwaysOkApiClient),
        "sess-suppression-test".to_string(),
        100, // max_batch_size
        0,   // max_retries — keeps tests fast
    )
    .with_dynamic_batch(false) // stable batch size for deterministic assertions
}

// ---------------------------------------------------------------------------
// Test 1 — RED at A.10
//
// A predicate that always returns `true` should cause `flush()` to skip
// draining the queue and return `Ok(0)`.
//
// At A.10 the stub ignores the predicate, so flush drains all 3 events and
// returns `Ok(3)`. The `assert_eq!(sent, 0)` line then fails → red state.
// A.11 greens this by gating flush on `predicate()`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn flush_returns_zero_when_suppression_predicate_true() {
    let uploader = make_uploader().with_suppression_predicate(Arc::new(|| true));

    uploader.enqueue(make_event());
    uploader.enqueue(make_event());
    uploader.enqueue(make_event());
    assert_eq!(uploader.queue_size(), 3, "pre-flush: 3 events queued");

    let sent = uploader
        .flush()
        .await
        .expect("flush should not return a network error");

    // When suppression is active, flush must return 0 and leave the queue intact.
    assert_eq!(
        sent, 0,
        "flush() must return 0 when the suppression predicate returns true \
         (A.10: EXPECTED FAILURE — stub does not suppress)"
    );
    assert_eq!(
        uploader.queue_size(),
        3,
        "queue must be unchanged when suppression predicate returns true \
         (A.10: EXPECTED FAILURE — stub does not suppress)"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — GREEN at A.10 (stays green after A.11)
//
// A predicate that always returns `false` must not suppress the flush.
// The stub ignores the predicate, so flush runs normally — this matches
// the expected behaviour, making the test pass even before A.11.
// A.11 keeps it green: `predicate() == false` → suppress=no → flush runs.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn flush_drains_when_suppression_predicate_false() {
    let uploader = make_uploader().with_suppression_predicate(Arc::new(|| false));

    uploader.enqueue(make_event());
    uploader.enqueue(make_event());
    uploader.enqueue(make_event());
    assert_eq!(uploader.queue_size(), 3, "pre-flush: 3 events queued");

    let sent = uploader
        .flush()
        .await
        .expect("flush should not return a network error");

    // Predicate=false → no suppression → queue must be drained.
    assert_eq!(
        sent, 3,
        "flush() must drain all 3 events when predicate returns false"
    );
    assert_eq!(uploader.queue_size(), 0, "queue must be empty after flush");
}

// ---------------------------------------------------------------------------
// Test 3 — RED at A.10
//
// The predicate closure captures shared state (an `Arc<RwLock<bool>>`).
// Changing that state mid-test should be reflected on the next `flush()`.
//
// Scenario:
//   - Initial state: suppress=false  → first flush drains 2 events (both pass).
//   - Flip state: suppress=true      → second flush must return Ok(0) and leave
//     2 newly-enqueued events in the queue.
//
// At A.10 the stub ignores the predicate, so the second flush drains the 2
// new events and returns `Ok(2)`. The `assert_eq!(sent2, 0)` fails → red.
// A.11 greens this by re-evaluating the closure on every flush call.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn predicate_reads_latest_config() {
    // Shared flag: false = do not suppress, true = suppress.
    let suppress_flag = Arc::new(RwLock::new(false));

    let flag_for_pred = Arc::clone(&suppress_flag);
    let uploader = make_uploader()
        .with_suppression_predicate(Arc::new(move || *flag_for_pred.read().unwrap()));

    // Phase 1: suppression off — flush should drain.
    uploader.enqueue(make_event());
    uploader.enqueue(make_event());
    assert_eq!(uploader.queue_size(), 2);

    let sent1 = uploader
        .flush()
        .await
        .expect("flush should not return a network error");
    assert_eq!(
        sent1, 2,
        "phase 1: predicate=false, flush must drain both events"
    );
    assert_eq!(
        uploader.queue_size(),
        0,
        "phase 1: queue must be empty after flush"
    );

    // Flip the flag → suppression now active.
    *suppress_flag.write().unwrap() = true;

    // Phase 2: suppression on — flush must return 0 and leave queue intact.
    uploader.enqueue(make_event());
    uploader.enqueue(make_event());
    assert_eq!(uploader.queue_size(), 2, "phase 2: 2 new events queued");

    let sent2 = uploader
        .flush()
        .await
        .expect("flush should not return a network error");

    assert_eq!(
        sent2, 0,
        "phase 2: flush() must return 0 when predicate was flipped to true \
         (A.10: EXPECTED FAILURE — stub does not re-evaluate predicate)"
    );
    assert_eq!(
        uploader.queue_size(),
        2,
        "phase 2: queue must remain intact when predicate suppresses flush \
         (A.10: EXPECTED FAILURE — stub does not suppress)"
    );
}
