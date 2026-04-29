[English](./oneshim-suggestion.md) | [한국어](./oneshim-suggestion.ko.md)

# oneshim-suggestion

The crate responsible for AI suggestion reception, processing, and feedback.

## Role

- **Suggestion Reception**: Receives suggestion events from SSE streams
- **Priority Management**: Sorts suggestions based on importance
- **Feedback Sending**: Sends user reactions (accept/reject) to the server
- **History Management**: Suggestion history cache

## Directory Structure

```
oneshim-suggestion/src/
├── lib.rs            # Crate root
├── receiver.rs       # SuggestionReceiver - SSE event → suggestion conversion
├── queue.rs          # PriorityQueue - BTreeSet priority queue (max 50)
├── feedback.rs       # FeedbackSender - Accept/Reject HTTP POST (+ fires FeedbackSignalSink per ADR-017)
├── feedback_retry.rs # FeedbackRetryQueue - persists failed posts for scheduler-driven retry
├── deferred.rs       # Deferred-suggestion handling (snooze + re-surface windows)
├── presenter.rs      # SuggestionPresenter - UI data conversion
├── history.rs        # SuggestionHistory - FIFO history cache
├── scorer.rs         # Suggestion scoring helpers
└── error.rs          # SuggestionError (ADR-019 typed codes)
```

## Key Components

### SuggestionReceiver (receiver.rs)

Converts SSE events into suggestions:

```rust
pub struct SuggestionReceiver {
    sse_client: Arc<dyn SseClient>,
    queue: Arc<PriorityQueue>,
    notifier: Arc<dyn DesktopNotifier>,
}

impl SuggestionReceiver {
    pub async fn start(&self, session_id: &str) -> Result<(), CoreError> {
        let (tx, mut rx) = mpsc::channel::<SseEvent>(100);

        // SSE connection task
        let sse = self.sse_client.clone();
        let sid = session_id.to_string();
        tokio::spawn(async move {
            sse.connect(&sid, tx).await
        });

        // Event processing loop
        let queue = self.queue.clone();
        let notifier = self.notifier.clone();
        while let Some(event) = rx.recv().await {
            match event {
                SseEvent::Suggestion(s) => {
                    queue.push(s.clone()).await;
                    notifier.notify(&s).await?;
                }
                SseEvent::Heartbeat { .. } => {
                    tracing::debug!("Heartbeat received");
                }
                SseEvent::Error(msg) => {
                    tracing::warn!("SSE error: {}", msg);
                }
                _ => {}
            }
        }

        Ok(())
    }
}
```

### PriorityQueue (queue.rs)

Priority queue based on `BTreeSet`:

```rust
pub struct PriorityQueue {
    queue: RwLock<BTreeSet<PrioritizedSuggestion>>,
    max_size: usize,
}

#[derive(Eq, PartialEq)]
struct PrioritizedSuggestion {
    priority_score: u32,  // Higher = more priority
    suggestion: Suggestion,
}

impl Ord for PrioritizedSuggestion {
    fn cmp(&self, other: &Self) -> Ordering {
        // Priority descending, then time ascending for ties
        other.priority_score.cmp(&self.priority_score)
            .then(self.suggestion.created_at.cmp(&other.suggestion.created_at))
    }
}

impl PriorityQueue {
    pub async fn push(&self, suggestion: Suggestion) {
        let mut queue = self.queue.write().await;

        let priority_score = self.calculate_score(&suggestion);
        queue.insert(PrioritizedSuggestion { priority_score, suggestion });

        // Remove lowest priority when exceeding max size
        while queue.len() > self.max_size {
            queue.pop_last();
        }
    }

    pub async fn pop(&self) -> Option<Suggestion> {
        let mut queue = self.queue.write().await;
        queue.pop_first().map(|p| p.suggestion)
    }

    fn calculate_score(&self, s: &Suggestion) -> u32 {
        let priority_weight = match s.priority {
            Priority::Critical => 1000,
            Priority::High => 750,
            Priority::Medium => 500,
            Priority::Low => 250,
        };

        let confidence_bonus = (s.confidence_score * 100.0) as u32;
        let relevance_bonus = (s.relevance_score * 100.0) as u32;
        let actionable_bonus = if s.is_actionable { 50 } else { 0 };

        priority_weight + confidence_bonus + relevance_bonus + actionable_bonus
    }
}
```

**Priority Criteria**:
| Factor | Weight |
|--------|--------|
| Critical priority | +1000 |
| High priority | +750 |
| Medium priority | +500 |
| Low priority | +250 |
| Confidence (0-1) | +0~100 |
| Relevance (0-1) | +0~100 |
| Actionable | +50 |

### FeedbackSender (feedback.rs)

User feedback transmission:

```rust
pub struct FeedbackSender {
    api_client: Arc<dyn ApiClient>,
    history: Arc<SuggestionHistory>,
}

impl FeedbackSender {
    pub async fn accept(&self, suggestion_id: &str) -> Result<(), CoreError> {
        self.api_client.send_feedback(suggestion_id, true).await?;
        self.history.record_feedback(suggestion_id, FeedbackType::Accepted).await;
        Ok(())
    }

    pub async fn reject(&self, suggestion_id: &str, reason: Option<&str>) -> Result<(), CoreError> {
        self.api_client.send_feedback(suggestion_id, false).await?;
        self.history.record_feedback(suggestion_id, FeedbackType::Rejected(reason.map(String::from))).await;
        Ok(())
    }

    pub async fn dismiss(&self, suggestion_id: &str) -> Result<(), CoreError> {
        self.history.record_feedback(suggestion_id, FeedbackType::Dismissed).await;
        Ok(())
    }
}
```

### SuggestionPresenter (presenter.rs)

Data conversion for UI display:

```rust
pub struct SuggestionPresenter;

pub struct SuggestionView {
    pub suggestion_id: String,
    pub title: String,
    pub body: String,
    pub priority_badge: String,
    pub action_buttons: Vec<ActionButton>,
    pub created_ago: String,
}

impl SuggestionPresenter {
    pub fn to_view(suggestion: &Suggestion) -> SuggestionView {
        SuggestionView {
            suggestion_id: suggestion.suggestion_id.clone(),
            title: Self::extract_title(&suggestion.content),
            body: Self::format_body(&suggestion.content),
            priority_badge: Self::priority_to_badge(&suggestion.priority),
            action_buttons: Self::create_actions(suggestion),
            created_ago: Self::format_time_ago(suggestion.created_at),
        }
    }

    fn priority_to_badge(priority: &Priority) -> String {
        match priority {
            Priority::Critical => "🔴 Critical".to_string(),
            Priority::High => "🟠 High".to_string(),
            Priority::Medium => "🟡 Medium".to_string(),
            Priority::Low => "🟢 Low".to_string(),
        }
    }

    fn format_time_ago(created_at: DateTime<Utc>) -> String {
        let duration = Utc::now() - created_at;
        if duration.num_minutes() < 1 {
            "Just now".to_string()
        } else if duration.num_hours() < 1 {
            format!("{} min ago", duration.num_minutes())
        } else if duration.num_days() < 1 {
            format!("{} hr ago", duration.num_hours())
        } else {
            format!("{} days ago", duration.num_days())
        }
    }
}
```

### SuggestionHistory (history.rs)

Suggestion history cache:

```rust
pub struct SuggestionHistory {
    cache: RwLock<VecDeque<HistoryEntry>>,
    max_entries: usize,
}

pub struct HistoryEntry {
    pub suggestion: Suggestion,
    pub feedback: Option<FeedbackType>,
    pub received_at: DateTime<Utc>,
    pub actioned_at: Option<DateTime<Utc>>,
}

impl SuggestionHistory {
    pub async fn record(&self, suggestion: Suggestion) {
        let mut cache = self.cache.write().await;

        if cache.len() >= self.max_entries {
            cache.pop_front();
        }

        cache.push_back(HistoryEntry {
            suggestion,
            feedback: None,
            received_at: Utc::now(),
            actioned_at: None,
        });
    }

    pub async fn get_recent(&self, count: usize) -> Vec<HistoryEntry> {
        let cache = self.cache.read().await;
        cache.iter().rev().take(count).cloned().collect()
    }
}
```

## Suggestion Flow

```
┌─────────────┐    ┌───────────────────┐    ┌───────────────┐
│   Server    │───▶│ SuggestionReceiver │───▶│ PriorityQueue │
│  (SSE)      │    │                   │    │               │
└─────────────┘    └───────────────────┘    └───────────────┘
                          │                        │
                          ▼                        ▼
                   ┌─────────────────┐    ┌───────────────────┐
                   │DesktopNotifier  │    │ SuggestionPresenter│
                   │  (notification) │    │   (UI display)     │
                   └─────────────────┘    └───────────────────┘
                                                   │
                          ┌────────────────────────┘
                          ▼
                   ┌─────────────────┐    ┌───────────────────┐
                   │ FeedbackSender  │◀───│  User reaction     │
                   └─────────────────┘    └───────────────────┘
                          │
                          ▼
                   ┌─────────────────┐
                   │ SuggestionHistory│
                   └─────────────────┘
```

## Suggestion Types

```rust
pub enum SuggestionType {
    WorkGuidance,      // Work guidance
    RiskAlert,         // Risk alert
    ProductivityTip,   // Productivity tip
    ContextAwareness,  // Context awareness
    ScheduleReminder,  // Schedule reminder
}
```

## Dependencies

- `oneshim-core`: Models, ports
- `oneshim-network`: SSE client (indirect)
- `tokio`: Async runtime, mpsc channels

## Tests

```rust
#[tokio::test]
async fn test_priority_queue_ordering() {
    let queue = PriorityQueue::new(50);

    // Add Low first
    let low = Suggestion {
        priority: Priority::Low,
        confidence_score: 0.5,
        // ...
    };
    queue.push(low).await;

    // Add High later
    let high = Suggestion {
        priority: Priority::High,
        confidence_score: 0.9,
        // ...
    };
    queue.push(high).await;

    // High should come out first
    let first = queue.pop().await.unwrap();
    assert_eq!(first.priority, Priority::High);
}

#[test]
fn test_presenter_time_ago() {
    let now = Utc::now();

    // 30 seconds ago
    let recent = now - Duration::seconds(30);
    assert_eq!(SuggestionPresenter::format_time_ago(recent), "Just now");

    // 5 minutes ago
    let minutes = now - Duration::minutes(5);
    assert_eq!(SuggestionPresenter::format_time_ago(minutes), "5 min ago");
}
```
