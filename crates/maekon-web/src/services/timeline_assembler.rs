use chrono::{DateTime, Utc};
use maekon_api_contracts::timeline::{AppSegment, SessionInfo, TimelineItem, TimelineResponse};
use maekon_core::id_generation::generate_id;
use maekon_core::models::activity::IdlePeriod;
use maekon_core::models::event::Event;
use maekon_core::models::storage_records::FrameRecord;

const APP_COLORS: &[&str] = &[
    "#3B82F6", "#10B981", "#F59E0B", "#EF4444", "#8B5CF6", "#EC4899", "#06B6D4", "#84CC16",
];

pub(crate) fn assemble_event_timeline_item(event: &Event) -> TimelineItem {
    let (event_id, event_type, timestamp, app_name, window_title) = match event {
        Event::User(value) => (
            value.event_id.to_string(),
            format!("{:?}", value.event_type),
            value.timestamp.to_rfc3339(),
            Some(value.app_name.clone()),
            Some(value.window_title.clone()),
        ),
        Event::System(value) => (
            value.event_id.to_string(),
            format!("{:?}", value.event_type),
            value.timestamp.to_rfc3339(),
            None,
            None,
        ),
        Event::Context(value) => (
            generate_id("ctx"),
            "ContextChange".to_string(),
            value.timestamp.to_rfc3339(),
            Some(value.app_name.clone()),
            Some(value.window_title.clone()),
        ),
        Event::Input(value) => (
            generate_id("input"),
            "InputActivity".to_string(),
            value.timestamp.to_rfc3339(),
            Some(value.app_name.clone()),
            None,
        ),
        Event::Process(value) => (
            generate_id("proc"),
            "ProcessSnapshot".to_string(),
            value.timestamp.to_rfc3339(),
            None,
            None,
        ),
        Event::Window(value) => (
            generate_id("win"),
            format!("{:?}", value.event_type),
            value.timestamp.to_rfc3339(),
            Some(value.window.app_name.clone()),
            Some(value.window.window_title.clone()),
        ),
        Event::Clipboard(value) => (
            generate_id("clip"),
            "ClipboardChange".to_string(),
            value.timestamp.to_rfc3339(),
            None,
            None,
        ),
        Event::FileAccess(value) => (
            generate_id("fa"),
            format!("FileAccess_{:?}", value.event_type),
            value.timestamp.to_rfc3339(),
            None,
            None,
        ),
    };

    TimelineItem::Event {
        id: event_id,
        timestamp,
        event_type,
        app_name,
        window_title,
    }
}

pub(crate) fn assemble_frame_timeline_item(frame: &FrameRecord) -> TimelineItem {
    TimelineItem::Frame {
        id: frame.id,
        timestamp: frame.timestamp.clone(),
        app_name: frame.app_name.clone(),
        window_title: frame.window_title.clone(),
        importance: frame.importance,
        image_url: format!("/api/frames/{}/image", frame.id),
        ocr_text: frame.ocr_text.clone(),
    }
}

pub(crate) fn assemble_idle_timeline_item(idle: &IdlePeriod) -> Option<TimelineItem> {
    match (idle.end_time, idle.duration_secs) {
        (Some(end_time), Some(duration_secs)) => Some(TimelineItem::IdlePeriod {
            start: idle.start_time.to_rfc3339(),
            end: end_time.to_rfc3339(),
            duration_secs: duration_secs as i64,
        }),
        _ => None,
    }
}

pub(crate) fn assemble_session_info(
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    total_events: usize,
    total_frames: usize,
    total_idle_secs: i64,
) -> SessionInfo {
    SessionInfo {
        start: from.to_rfc3339(),
        end: to.to_rfc3339(),
        duration_secs: (to - from).num_seconds(),
        total_events: total_events as i64,
        total_frames: total_frames as i64,
        total_idle_secs,
    }
}

pub(crate) fn assemble_timeline_response(
    session: SessionInfo,
    items: Vec<TimelineItem>,
    segments: Vec<AppSegment>,
) -> TimelineResponse {
    TimelineResponse {
        session,
        items,
        segments,
    }
}

pub(crate) fn app_to_color(app_name: &str) -> String {
    let hash = app_name.bytes().fold(0usize, |accumulator, byte| {
        accumulator.wrapping_add(byte as usize)
    });
    APP_COLORS[hash % APP_COLORS.len()].to_string()
}

pub(crate) fn calculate_app_segments(items: &[TimelineItem]) -> Vec<AppSegment> {
    let mut segments: Vec<AppSegment> = Vec::new();

    for item in items {
        let (app_name, timestamp) = match item {
            TimelineItem::Event {
                app_name: Some(name),
                timestamp,
                ..
            } => (name.clone(), timestamp.clone()),
            TimelineItem::Frame {
                app_name,
                timestamp,
                ..
            } => (app_name.clone(), timestamp.clone()),
            _ => continue,
        };

        if let Some(last) = segments.last_mut() {
            if last.app_name == app_name {
                last.end = timestamp;
                continue;
            }
        }

        segments.push(AppSegment {
            color: app_to_color(&app_name),
            app_name,
            start: timestamp.clone(),
            end: timestamp,
        });
    }

    segments
}

pub(crate) fn timeline_item_timestamp(item: &TimelineItem) -> &str {
    match item {
        TimelineItem::Event { timestamp, .. } => timestamp,
        TimelineItem::Frame { timestamp, .. } => timestamp,
        TimelineItem::IdlePeriod { start, .. } => start,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_prv_cap_014_frame_timeline_item_includes_ocr_text() {
        let frame = FrameRecord {
            id: 42,
            timestamp: "2026-05-16T04:00:00Z".to_string(),
            trigger_type: "manual".to_string(),
            app_name: "Notes".to_string(),
            window_title: "meeting notes".to_string(),
            importance: 0.9,
            resolution_w: 1280,
            resolution_h: 720,
            file_path: Some("frames/42.webp".to_string()),
            ocr_text: Some("follow up with Alex at 3pm".to_string()),
        };

        let item = assemble_frame_timeline_item(&frame);

        match item {
            TimelineItem::Frame { ocr_text, .. } => {
                assert_eq!(ocr_text.as_deref(), Some("follow up with Alex at 3pm"));
            }
            _ => panic!("expected frame timeline item"),
        }
    }
}
