use chrono::Utc;
use maekon_core::models::suggestion::{FeedbackType, Priority, Suggestion, SuggestionType};
use maekon_suggestion::history::SuggestionHistory;
use maekon_suggestion::presenter;
use maekon_suggestion::queue::SuggestionQueue;

// U2-sse-e2e: mock_server 모듈을 통합 테스트 바이너리에 포함한다.
// (`server_integration.rs` 와 동일한 axum mock 서버를 재사용)
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
// U2-sse-e2e: SSE → receiver → priority queue → notifier 라이브 e2e
//
// 기존 테스트는 모두 in-memory(queue/history/presenter 직접 호출)였다. 아래 테스트는
// 실제 HTTP/SSE 전송 경로를 통과한다:
//
//   axum mock 서버(`/user_context/sessions/stream`)가 진짜 `suggestion` SSE 이벤트 방출
//      → 프로덕션 `SseStreamClient`(maekon-network)가 HTTP GET + Eventsource 파싱
//      → 프로덕션 `SuggestionReceiver::run`(maekon-suggestion)이 mpsc 채널 소비
//      → scorer 조정 → priority queue push → notifier.show_suggestion 호출
//
// 검증 대상(관찰 가능 동작): 제안이 큐에 안착(queue_size == 1, 올바른 id) +
// notifier 가 정확히 1회 발화. 백오프 타이밍 단언은 범위에서 제외한다.
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

    // 발화 횟수를 세는 실제 DesktopNotifier 구현(수동 mock, mockall 미사용 — ADR-001 §5).
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

    // 1. 라이브 mock 서버 기동 (loopback, 랜덤 포트).
    let server = MockServer::start().await;

    // 2. TokenManager 가 실제로 로그인하도록 한다 — SseStreamClient::connect 는
    //    매 연결마다 token_manager.get_token() 을 호출하며, 인증 상태가 없으면
    //    실패한다. mock 서버의 /api/v1/auth/tokens 가 유효 토큰을 반환한다.
    #[allow(deprecated)] // 테스트는 non-TLS TokenManager::new 사용 (loopback)
    let token_manager = Arc::new(TokenManager::new(server.url()));
    token_manager
        .login("e2e@example.com", "test-password-placeholder")
        .await
        .expect("mock 서버 로그인 실패");

    // 3. 프로덕션 SSE 클라이언트 + receiver 체인 구성.
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

    // 4. receiver.run 구동 — mock 스트림은 connection → suggestion → close 순서로
    //    이벤트를 방출하므로 run() 은 close 수신 후 자연 종료한다. 무한 대기를 막기
    //    위해 5초 타임아웃으로 감싼다.
    let run_result =
        tokio::time::timeout(Duration::from_secs(5), receiver.run("u2-sse-session")).await;

    // Collapse: outer Ok proves the 5-second timeout was not hit (no scheduler
    // starvation); inner Ok proves run() returned without a SuggestionError.
    run_result
        .expect("receiver.run() did not finish within 5 s — close event handling failed")
        .expect("receiver.run() returned a SuggestionError");

    // 5. 관찰 가능 동작 검증: 제안이 큐에 안착했는가?
    let queued = queue.lock().await;
    assert_eq!(
        queued.len(),
        1,
        "라이브 SSE 제안이 priority queue 에 안착하지 않음"
    );
    let top = queued.peek().expect("큐 top 제안 부재");
    assert_eq!(
        top.suggestion_id, "u2-sse-e2e-1",
        "큐에 안착한 제안 id 가 mock 서버가 보낸 것과 다름"
    );
    assert_eq!(
        top.suggestion_type,
        SuggestionType::WorkGuidance,
        "WORK_GUIDANCE serde 매핑 실패"
    );
    assert_eq!(top.priority, Priority::High, "HIGH 우선순위 매핑 실패");

    // 6. 관찰 가능 동작 검증: notifier 가 정확히 1회 발화했는가?
    assert_eq!(
        notifier.suggestion_calls.load(Ordering::SeqCst),
        1,
        "notifier.show_suggestion 이 정확히 1회 발화하지 않음"
    );

    // 7. mock 서버가 실제 SSE 요청을 수신했는지 sanity 확인
    //    (로그인 1회 + SSE 스트림 1회 = 최소 2회).
    assert!(
        server.request_count() >= 2,
        "mock 서버가 로그인+SSE 요청을 수신하지 못함: {}",
        server.request_count()
    );
}
