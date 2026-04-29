use chrono::Utc;
use oneshim_core::models::suggestion::{FeedbackType, Priority, Suggestion, SuggestionType};
use oneshim_suggestion::history::SuggestionHistory;
use oneshim_suggestion::presenter;
use oneshim_suggestion::queue::SuggestionQueue;

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
