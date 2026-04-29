use chrono::Utc;
use image::{DynamicImage, RgbaImage};
use oneshim_core::models::event::ContextEvent;
use oneshim_core::models::frame::FrameMetadata;
use oneshim_vision::delta;
use oneshim_vision::encoder::{self, WebPQuality};
use oneshim_vision::privacy;
use oneshim_vision::thumbnail;
use oneshim_vision::timeline::{Timeline, TimelineFilter};
use oneshim_vision::trigger::SmartCaptureTrigger;

fn make_test_image(w: u32, h: u32, color: [u8; 4]) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(w, h, image::Rgba(color)))
}

fn make_event(app: &str, title: &str, prev: Option<&str>) -> ContextEvent {
    ContextEvent {
        app_name: app.to_string(),
        window_title: title.to_string(),
        prev_app_name: prev.map(String::from),
        timestamp: Utc::now(),
        ..Default::default()
    }
}

#[test]
fn trigger_produces_capture_requests() {
    use oneshim_core::ports::vision::CaptureTrigger;

    let trigger = SmartCaptureTrigger::new(0);

    let error_event = make_event("Terminal", "Error: panic at line 42", None);
    let req = trigger.should_capture(&error_event);
    assert!(req.is_some());
    let req = req.unwrap();
    assert!(
        req.importance >= 0.8,
        "error event severity should be >= 0.8"
    );

    let switch_event = make_event("Firefox", "Google", Some("Code"));
    let req = trigger.should_capture(&switch_event);
    assert!(req.is_some());
    assert!(req.unwrap().importance >= 0.5);
}

#[test]
fn encode_decode_roundtrip() {
    let img = make_test_image(320, 240, [100, 150, 200, 255]);

    let bytes = encoder::encode_webp(&img, WebPQuality::Medium).unwrap();
    assert!(!bytes.is_empty());

    let b64 = encoder::encode_webp_base64(&img, WebPQuality::Low).unwrap();
    assert!(!b64.is_empty());

    use base64::{engine::general_purpose::STANDARD, Engine};
    let decoded = STANDARD.decode(&b64).unwrap();
    assert!(!decoded.is_empty());
}

#[test]
fn adaptive_encoding_respects_size_limit() {
    let img = make_test_image(200, 200, [50, 100, 150, 255]);

    let (bytes, _quality) = encoder::encode_adaptive(&img, 1_000_000).unwrap();
    assert!(!bytes.is_empty());
}

#[test]
fn thumbnail_then_encode() {
    let img = make_test_image(1920, 1080, [80, 120, 160, 255]);

    let thumb = thumbnail::fast_resize(&img, 480, 270).unwrap();
    assert_eq!(thumb.width(), 480);
    assert_eq!(thumb.height(), 270);

    let encoded = encoder::encode_webp(&thumb, WebPQuality::Low).unwrap();
    assert!(!encoded.is_empty());

    let original_encoded = encoder::encode_webp(&img, WebPQuality::Low).unwrap();
    assert!(encoded.len() < original_encoded.len());
}

#[test]
fn delta_detection() {
    let img1 = make_test_image(320, 240, [100, 100, 100, 255]);
    let img2 = make_test_image(320, 240, [100, 100, 100, 255]);
    let img3 = make_test_image(320, 240, [200, 50, 50, 255]);
    let d1 = delta::compute_delta(&img1, &img2);
    assert!(d1.is_none());

    let d2 = delta::compute_delta(&img1, &img3);
    assert!(d2.is_some());
    let region = d2.unwrap();
    assert!(region.changed_ratio > 0.0);
}

#[test]
fn privacy_sanitization() {
    let sanitized = privacy::sanitize_title("Login - user@example.com - Dashboard");
    assert!(!sanitized.contains("user@example.com"));
    assert!(sanitized.contains("[EMAIL]"));

    let sanitized = privacy::sanitize_title("Edit: /Users/johndoe/project/main.rs");
    assert!(!sanitized.contains("johndoe"));
    assert!(sanitized.contains("[USER]"));

    let clean = "Visual Studio Code - Cargo.toml";
    assert_eq!(privacy::sanitize_title(clean), clean);
}

#[test]
fn timeline_add_and_filter() {
    let mut timeline = Timeline::new(100);

    let meta1 = FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: "ErrorDetected".to_string(),
        app_name: "Terminal".to_string(),
        window_title: "Error output".to_string(),
        resolution: (1920, 1080),
        importance: 0.9,
    };
    let meta2 = FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: "Regular".to_string(),
        app_name: "Code".to_string(),
        window_title: "main.rs".to_string(),
        resolution: (1920, 1080),
        importance: 0.3,
    };

    let id1 = timeline.add_frame(meta1, true);
    let id2 = timeline.add_frame(meta2, false);
    assert!(id1 < id2);
    assert_eq!(timeline.len(), 2);

    let code_only = timeline.query(&TimelineFilter::new(10).with_app("Code"));
    assert_eq!(code_only.len(), 1);

    let high_only = timeline.query(&TimelineFilter::new(10).with_min_importance(0.5));
    assert_eq!(high_only.len(), 1);

    let error_results = timeline.query(&TimelineFilter::new(10).with_text("Error"));
    assert_eq!(error_results.len(), 1);
}

#[test]
fn full_vision_pipeline() {
    use oneshim_core::ports::vision::CaptureTrigger;

    let trigger = SmartCaptureTrigger::new(5000);
    let event = make_event("Terminal", "Error: segfault", None);
    let capture_req = trigger.should_capture(&event).unwrap();
    assert!(capture_req.importance >= 0.8);

    let sanitized_title = privacy::sanitize_title(&capture_req.window_title);
    assert!(!sanitized_title.is_empty());

    let img = make_test_image(1920, 1080, [128, 64, 200, 255]);
    let encoded = encoder::encode_webp_base64(&img, WebPQuality::High).unwrap();
    assert!(!encoded.is_empty());

    let mut timeline = Timeline::new(100);
    let meta = FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: capture_req.trigger_type,
        app_name: capture_req.app_name,
        window_title: sanitized_title,
        resolution: (1920, 1080),
        importance: capture_req.importance,
    };
    let frame_id = timeline.add_frame(meta, true);
    assert!(frame_id > 0);
    assert_eq!(timeline.len(), 1);
}
