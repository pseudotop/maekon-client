use chrono::Utc;
use maekon_vision::processor::{build_frame_metadata, CaptureMetadataInput};

#[test]
fn crt_prv_cap_013_frame_metadata_preserves_source_fields() {
    let before = Utc::now();

    let metadata = build_frame_metadata(CaptureMetadataInput {
        trigger_type: "active_window_change",
        app_name: "Notes",
        window_title: "meeting notes",
        resolution: (1280, 720),
        importance: 0.9,
        monitor_id: Some(0),
        app_bundle_id: Some("com.apple.Notes"),
    });

    let after = Utc::now();

    assert!(metadata.timestamp >= before);
    assert!(metadata.timestamp <= after);
    assert_eq!(metadata.trigger_type, "active_window_change");
    assert_eq!(metadata.app_name, "Notes");
    assert_eq!(metadata.window_title, "meeting notes");
    assert_eq!(metadata.resolution, (1280, 720));
    assert_eq!(metadata.monitor_id, Some(0));
    assert_eq!(metadata.app_bundle_id.as_deref(), Some("com.apple.Notes"));
}
