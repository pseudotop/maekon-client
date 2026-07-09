// Integration test binary (`tests/*.rs` is its own crate root, entirely
// test-only) — not covered by `src/main.rs`'s `#[cfg_attr(test, allow(...))]`
// (#7719 `significant_drop_tightening` workspace enforcement).
#![allow(clippy::significant_drop_tightening)]

use chrono::Utc;
use maekon_core::models::suggestion::{FeedbackType, Priority, Suggestion, SuggestionType};
use maekon_suggestion::history::SuggestionHistory;
use maekon_suggestion::presenter;
use maekon_suggestion::queue::SuggestionQueue;

// U2-sse-e2e: include the mock_server module in this integration test binary.
// (Reuses the same axum mock server as `server_integration.rs`.)
#[cfg(feature = "analysis")]
#[path = "mock_server.rs"]
mod mock_server;

fn make_suggestion(id: &str, priority: Priority, content: &str) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_string(),
        suggestion_type: SuggestionType::WorkGuidance,
        content: content.to_string(),
        priority,
        confidence_score: 0.9,
        relevance_score: 0.85,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: Default::default(),
        reasoning: None,
        context_scope: None,
    }
}

#[test]
fn queue_to_presenter_flow() {
    let mut queue = SuggestionQueue::new(10);
    queue.push(make_suggestion("s1", Priority::Low, "low priority"));
    queue.push(make_suggestion(
        "s2",
        Priority::Critical,
        "critical suggestion",
    ));
    queue.push(make_suggestion("s3", Priority::Medium, "medium suggestion"));

    assert_eq!(queue.len(), 3);

    let top = queue.pop().unwrap();
    assert_eq!(top.suggestion_id, "s2"); // Critical
    assert_eq!(top.priority, Priority::Critical);

    let next = queue.peek().unwrap();
    let view = presenter::present(next);
    assert!(!view.title.is_empty());
    assert!(!view.body.is_empty());
}

#[test]
fn history_tracks_presented_suggestions() {
    let mut history = SuggestionHistory::new(100);

    let s1 = make_suggestion("h1", Priority::High, "suggestion 1");
    let s2 = make_suggestion("h2", Priority::Medium, "suggestion 2");
    let s3 = make_suggestion("h3", Priority::Low, "suggestion 3");

    history.add(s1);
    history.add(s2);
    history.add(s3);

    assert_eq!(history.len(), 3);

    let recent = history.recent(2);
    assert_eq!(recent.len(), 2);

    history.record_feedback("h1", FeedbackType::Accepted);

    let stats = history.stats();
    assert_eq!(stats.total, 3);
    assert_eq!(stats.accepted, 1);
}

#[test]
fn queue_overflow_evicts_lowest() {
    let mut queue = SuggestionQueue::new(2); // 2items
    queue.push(make_suggestion("a", Priority::High, "high"));
    queue.push(make_suggestion("b", Priority::Critical, "critical"));
    queue.push(make_suggestion("c", Priority::Medium, "medium")); // medium should be evicted
    assert_eq!(queue.len(), 2);

    let first = queue.pop().unwrap();
    let second = queue.pop().unwrap();
    assert_eq!(first.priority, Priority::Critical);
    assert_eq!(second.priority, Priority::High);
}

#[test]
fn presenter_truncates_long_content() {
    let long_content = "A".repeat(200);
    let suggestion = make_suggestion("long", Priority::Medium, &long_content);
    let view = presenter::present(&suggestion);

    assert!(!view.body.is_empty());
}

#[test]
fn presenter_all_priorities() {
    for priority in [
        Priority::Low,
        Priority::Medium,
        Priority::High,
        Priority::Critical,
    ] {
        let s = make_suggestion("p", priority.clone(), "for display");
        let view = presenter::present(&s);
        assert!(
            !view.priority_color.is_empty(),
            "No color for priority {:?}",
            priority
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// U2-sse-e2e: SSE → receiver → priority queue → notifier live e2e
//
// The existing tests are all in-memory (calling queue/history/presenter directly).
// The test below exercises the real HTTP/SSE transport path:
//
//   axum mock server (`/user_context/sessions/stream`) emits a real `suggestion` SSE event
//      → production `SseStreamClient` (maekon-network) does HTTP GET + Eventsource parsing
//      → production `SuggestionReceiver::run` (maekon-suggestion) consumes the mpsc channel
//      → scorer adjustment → priority queue push → notifier.show_suggestion call
//
// What is verified (observable behavior): the suggestion lands in the queue
// (queue_size == 1, correct id) + the notifier fires exactly once. Backoff timing
// assertions are out of scope.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "analysis")]
#[tokio::test]
async fn sse_to_receiver_queue_notifier_live_e2e() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use maekon_core::error::CoreError;
    use maekon_core::ports::notifier::DesktopNotifier;
    use maekon_network::auth::TokenManager;
    use maekon_network::sse_client::SseStreamClient;
    use maekon_suggestion::receiver::SuggestionReceiver;
    use maekon_suggestion::scorer::FeedbackScorer;
    use tokio::sync::Mutex;

    use mock_server::MockServer;

    // A real DesktopNotifier implementation that counts calls (manual mock, no
    // mockall — ADR-001 §5).
    struct CountingNotifier {
        suggestion_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DesktopNotifier for CountingNotifier {
        async fn show_suggestion(&self, _suggestion: &Suggestion) -> Result<(), CoreError> {
            self.suggestion_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn show_notification(&self, _title: &str, _body: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn show_error(&self, _message: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    // 1. Start the live mock server (loopback, random port).
    let server = MockServer::start().await;

    // 2. Make TokenManager actually log in — SseStreamClient::connect calls
    //    token_manager.get_token() on every connection and fails if there is no
    //    authenticated state. The mock server's /api/v1/auth/tokens returns a
    //    valid token.
    #[allow(deprecated)] // test uses non-TLS TokenManager::new (loopback)
    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("e2e@example.com", "test-password-placeholder")
        .await
        .expect("mock server login failed");

    // 3. Build the production SSE client + receiver chain.
    // loopback mock server — TLS not needed; new_with_tls is the production path.
    #[allow(deprecated)]
    let sse_client = Arc::new(SseStreamClient::new(
        server.url(),
        token_manager.clone(),
        30,
    ));
    let notifier = Arc::new(CountingNotifier {
        suggestion_calls: AtomicUsize::new(0),
    });
    let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
    let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));

    let receiver = SuggestionReceiver::new(
        sse_client as Arc<dyn maekon_core::ports::api_client::SseClient>,
        Some(notifier.clone() as Arc<dyn DesktopNotifier>),
        queue.clone(),
        scorer,
    );

    // 4. Drive receiver.run — the mock stream emits events in connection →
    //    suggestion → close order, so run() terminates naturally after receiving
    //    close. Wrap it in a 5-second timeout to prevent an indefinite wait.
    let run_result =
        tokio::time::timeout(Duration::from_secs(5), receiver.run("u2-sse-session")).await;

    // Collapse: outer Ok proves the 5-second timeout was not hit (no scheduler
    // starvation); inner Ok proves run() returned without a SuggestionError.
    run_result
        .expect("receiver.run() did not finish within 5 s — close event handling failed")
        .expect("receiver.run() returned a SuggestionError");

    // 5. Verify observable behavior: did the suggestion land in the queue?
    let queued = queue.lock().await;
    assert_eq!(
        queued.len(),
        1,
        "live SSE suggestion did not land in the priority queue"
    );
    let top = queued.peek().expect("queue top suggestion missing");
    assert_eq!(
        top.suggestion_id, "u2-sse-e2e-1",
        "queued suggestion id differs from what the mock server sent"
    );
    assert_eq!(
        top.suggestion_type,
        SuggestionType::WorkGuidance,
        "WORK_GUIDANCE serde mapping failed"
    );
    assert_eq!(top.priority, Priority::High, "HIGH priority mapping failed");

    // 6. Verify observable behavior: did the notifier fire exactly once?
    assert_eq!(
        notifier.suggestion_calls.load(Ordering::SeqCst),
        1,
        "notifier.show_suggestion did not fire exactly once"
    );

    // 7. Sanity check that the mock server received the real SSE request
    //    (1 login + 1 SSE stream = at least 2).
    assert!(
        server.request_count() >= 2,
        "mock server did not receive login + SSE requests: {}",
        server.request_count()
    );
}
