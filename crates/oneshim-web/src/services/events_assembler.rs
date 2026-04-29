use oneshim_api_contracts::events::EventResponse;
use oneshim_core::models::event::Event;

pub(crate) fn assemble_event_response(event: Event) -> EventResponse {
    match event {
        Event::User(value) => EventResponse {
            event_id: value.event_id.to_string(),
            event_type: "User".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: Some(value.app_name.clone()),
            window_title: Some(value.window_title.clone()),
            data: serde_json::json!({
                "event_type": format!("{:?}", value.event_type),
                "app_name": value.app_name,
                "window_title": value.window_title,
            }),
        },
        Event::System(value) => EventResponse {
            event_id: value.event_id.to_string(),
            event_type: "System".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: None,
            window_title: None,
            data: serde_json::json!({
                "event_type": format!("{:?}", value.event_type),
                "data": value.data,
            }),
        },
        Event::Context(value) => EventResponse {
            event_id: format!("ctx_{}", uuid::Uuid::new_v4()),
            event_type: "Context".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: Some(value.app_name.clone()),
            window_title: Some(value.window_title.clone()),
            data: serde_json::json!({
                "app_name": value.app_name,
                "window_title": value.window_title,
                "prev_app_name": value.prev_app_name,
            }),
        },
        Event::Input(value) => EventResponse {
            event_id: format!("input_{}", uuid::Uuid::new_v4()),
            event_type: "Input".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: Some(value.app_name.clone()),
            window_title: None,
            data: serde_json::json!({
                "period_secs": value.period_secs,
                "mouse": {
                    "click_count": value.mouse.click_count,
                    "move_distance": value.mouse.move_distance,
                    "scroll_count": value.mouse.scroll_count,
                    "double_click_count": value.mouse.double_click_count,
                    "right_click_count": value.mouse.right_click_count,
                },
                "keyboard": {
                    "keystrokes_per_min": value.keyboard.keystrokes_per_min,
                    "total_keystrokes": value.keyboard.total_keystrokes,
                    "typing_bursts": value.keyboard.typing_bursts,
                    "shortcut_count": value.keyboard.shortcut_count,
                    "correction_count": value.keyboard.correction_count,
                },
            }),
        },
        Event::Process(value) => EventResponse {
            event_id: format!("proc_{}", uuid::Uuid::new_v4()),
            event_type: "Process".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: None,
            window_title: None,
            data: serde_json::json!({
                "total_process_count": value.total_process_count,
                "processes": value.processes.iter().map(|process| serde_json::json!({
                    "name": process.name,
                    "pid": process.pid,
                    "cpu_percent": process.cpu_percent,
                    "memory_mb": process.memory_mb,
                    "window_count": process.window_count,
                    "is_foreground": process.is_foreground,
                })).collect::<Vec<_>>(),
            }),
        },
        Event::Window(value) => EventResponse {
            event_id: format!("win_{}", uuid::Uuid::new_v4()),
            event_type: "Window".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: Some(value.window.app_name.clone()),
            window_title: Some(value.window.window_title.clone()),
            data: serde_json::json!({
                "event_type": format!("{:?}", value.event_type),
                "position": value.window.position,
                "size": value.window.size,
                "screen_ratio": value.window.screen_ratio,
                "is_fullscreen": value.window.is_fullscreen,
                "z_order": value.window.z_order,
                "screen_resolution": value.screen_resolution,
                "monitor_index": value.monitor_index,
            }),
        },
        Event::Clipboard(value) => EventResponse {
            event_id: format!("clip_{}", uuid::Uuid::new_v4()),
            event_type: "Clipboard".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: None,
            window_title: None,
            data: serde_json::json!({
                "content_type": format!("{:?}", value.content_type),
                "char_count": value.char_count,
                "preview": value.preview,
            }),
        },
        Event::FileAccess(value) => EventResponse {
            event_id: format!("fa_{}", uuid::Uuid::new_v4()),
            event_type: "FileAccess".to_string(),
            timestamp: value.timestamp.to_rfc3339(),
            app_name: None,
            window_title: None,
            data: serde_json::json!({
                "event_type": format!("{:?}", value.event_type),
                "relative_path": value.relative_path.display().to_string(),
                "extension": value.extension,
            }),
        },
    }
}
