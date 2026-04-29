use chrono::Utc;
use oneshim_core::models::event::{ContextEvent, Event};
use oneshim_core::ports::storage::StorageService;
use oneshim_storage::sqlite::SqliteStorage;

fn make_context_event(app: &str, title: &str) -> Event {
    Event::Context(ContextEvent {
        app_name: app.to_string(),
        window_title: title.to_string(),
        prev_app_name: None,
        timestamp: Utc::now(),
        ..Default::default()
    })
}

#[tokio::test]
async fn save_and_retrieve_events() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let event1 = make_context_event("Code", "main.rs");
    let event2 = make_context_event("Firefox", "Google");
    storage.save_event(&event1).await.unwrap();
    storage.save_event(&event2).await.unwrap();

    let from = Utc::now() - chrono::Duration::hours(1);
    let to = Utc::now() + chrono::Duration::hours(1);
    let events = storage.get_events(from, to, 100).await.unwrap();
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn pending_events_and_mark_as_sent() {
    use oneshim_core::models::event::{UserEvent, UserEventType};
    use uuid::Uuid;

    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    let id3 = Uuid::new_v4();

    for id in [id1, id2, id3] {
        let event = Event::User(UserEvent {
            event_id: id,
            event_type: UserEventType::WindowChange,
            timestamp: Utc::now(),
            app_name: "App".to_string(),
            window_title: "Window".to_string(),
        });
        storage.save_event(&event).await.unwrap();
    }

    let pending = storage.get_pending_events(10).await.unwrap();
    assert_eq!(pending.len(), 3);

    storage.mark_as_sent(&[id1.to_string()]).await.unwrap();

    let pending = storage.get_pending_events(10).await.unwrap();
    assert_eq!(pending.len(), 2);
}

#[tokio::test]
async fn retention_policy_on_empty_db() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let deleted = storage.enforce_retention().await.unwrap();
    assert_eq!(deleted, 0);
}

#[tokio::test]
async fn multiple_event_types() {
    use oneshim_core::models::event::{SystemEvent, SystemEventType, UserEvent, UserEventType};
    use uuid::Uuid;

    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let ctx = Event::Context(ContextEvent {
        app_name: "Code".to_string(),
        window_title: "test.rs".to_string(),
        prev_app_name: Some("Firefox".to_string()),
        timestamp: Utc::now(),
        ..Default::default()
    });
    let user = Event::User(UserEvent {
        event_id: Uuid::new_v4(),
        event_type: UserEventType::WindowChange,
        timestamp: Utc::now(),
        app_name: "Code".to_string(),
        window_title: "lib.rs".to_string(),
    });
    let sys = Event::System(SystemEvent {
        event_id: Uuid::new_v4(),
        event_type: SystemEventType::MetricsUpdate,
        timestamp: Utc::now(),
        data: serde_json::json!({"cpu": 45.0}),
    });

    storage.save_event(&ctx).await.unwrap();
    storage.save_event(&user).await.unwrap();
    storage.save_event(&sys).await.unwrap();

    let from = Utc::now() - chrono::Duration::hours(1);
    let to = Utc::now() + chrono::Duration::hours(1);
    let all = storage.get_events(from, to, 100).await.unwrap();
    assert_eq!(all.len(), 3);
}
