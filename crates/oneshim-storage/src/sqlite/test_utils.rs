use chrono::Utc;
use oneshim_core::models::event::{Event, UserEvent, UserEventType};
use uuid::Uuid;

pub(crate) fn make_user_event() -> Event {
    Event::User(UserEvent {
        event_id: Uuid::new_v4(),
        event_type: UserEventType::WindowChange,
        timestamp: Utc::now(),
        app_name: "Code".to_string(),
        window_title: "test.rs".to_string(),
    })
}
