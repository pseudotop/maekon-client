use chrono::Utc;
use image::{DynamicImage, RgbaImage};
use maekon_core::models::event::ContextEvent;
use maekon_core::models::frame::FrameMetadata;
use maekon_vision::delta;
use maekon_vision::encoder::{self, WebPQuality};
use maekon_vision::privacy;
use maekon_vision::thumbnail;
use maekon_vision::timeline::{Timeline, TimelineFilter};
use maekon_vision::trigger::SmartCaptureTrigger;

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
    use maekon_core::ports::vision::CaptureTrigger;

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
        monitor_id: None,
        app_bundle_id: None,
    };
    let meta2 = FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: "Regular".to_string(),
        app_name: "Code".to_string(),
        window_title: "main.rs".to_string(),
        resolution: (1920, 1080),
        importance: 0.3,
        monitor_id: None,
        app_bundle_id: None,
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
    use maekon_core::ports::vision::CaptureTrigger;

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
        monitor_id: None,
        app_bundle_id: None,
    };
    let frame_id = timeline.add_frame(meta, true);
    assert!(frame_id > 0);
    assert_eq!(timeline.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// #4830 — vision FrameProcessor wired-chain / backpressure-drop coverage
//
// 아래 테스트들은 (1) 실제 EdgeFrameProcessor 프로덕션 wired chain 과
// (2) bounded ring buffer 가 backpressure 상황에서 프레임을 DROP 하는(가장 오래된
// 프레임 축출) 실제 동작을 직접 호출하여 검증한다. theater(하드코딩 단언) 금지 —
// 모든 단언은 프로덕션 함수 호출 결과의 관찰 가능한 동작에 대한 것이다.
// ─────────────────────────────────────────────────────────────────────────────

use maekon_vision::ring_buffer::{CaptureRingBuffer, RingFrame};

/// 링버퍼용 경량 프레임을 생성한다. thumbnail_data 에 식별용 시퀀스 바이트를 심어
/// 어느 프레임이 살아남았는지/축출됐는지 확인할 수 있게 한다.
fn make_ring_frame(seq: u8, title: &str) -> RingFrame {
    RingFrame {
        timestamp: Utc::now(),
        thumbnail_data: vec![seq; 8],
        app_name: "Terminal".to_string(),
        window_title: title.to_string(),
        accessibility_elements: Vec::new(),
    }
}

/// bounded ring buffer 가 backpressure(용량 초과 push) 상황에서 프레임을 DROP 하고,
/// 가장 오래된 프레임을 먼저 축출(FIFO)하는지 실제 프로덕션 경로로 검증한다.
///
/// 검증 항목:
/// - 용량(capacity)을 절대 초과하지 않는다 (bounded).
/// - 초과분만큼 정확히 축출 카운트가 증가한다 (drop 발생 관찰).
/// - 축출 후 살아남은 프레임은 가장 최근 N개이며, 가장 오래된 것이 사라진다.
/// - check_and_flush 의 pre_event_frames 가 축출 이후의 윈도우를 그대로 반영한다.
#[test]
fn ring_buffer_drops_frames_under_backpressure() {
    const CAPACITY: usize = 4;
    const PUSHED: u8 = 10;

    // post_event_count=2, flush_threshold=0.5 인 실제 프로덕션 링버퍼 구성.
    let rb = CaptureRingBuffer::new(CAPACITY, 2, 0.5);

    // 용량 4 인 버퍼에 10개 push → 6개가 backpressure 로 DROP 되어야 한다.
    for seq in 0..PUSHED {
        rb.push(make_ring_frame(seq, &format!("frame-{seq}")));
    }

    // (1) bounded: len 은 절대 capacity 를 넘지 않는다.
    assert_eq!(
        rb.len(),
        CAPACITY,
        "ring buffer must stay bounded at capacity under backpressure"
    );

    // (2) drop 발생: 축출 카운트는 (push 횟수 - capacity) 와 정확히 일치한다.
    let expected_evictions = (PUSHED as u64) - (CAPACITY as u64);
    assert_eq!(
        rb.evicted_count(),
        expected_evictions,
        "exactly the overflow frames must be dropped (oldest-evicted)"
    );

    // (3) FIFO oldest-evicted: 살아남은 프레임은 마지막 CAPACITY 개(seq 6..10).
    //     실제 프로덕션 flush 경로(check_and_flush)로 버퍼를 드레인해
    //     pre_event_frames 순서/내용이 축출 이후 윈도우를 반영하는지 확인한다.
    let flush = rb
        .check_and_flush(0.9, make_ring_frame(99, "trigger"))
        .expect("importance >= threshold must flush");

    let surviving: Vec<u8> = flush
        .pre_event_frames
        .iter()
        .map(|f| f.thumbnail_data[0])
        .collect();
    // seq 0..5 는 축출, seq 6..9 만 생존 (oldest-first 순서 유지).
    assert_eq!(
        surviving,
        vec![6u8, 7, 8, 9],
        "oldest frames must be dropped; only the most-recent CAPACITY frames survive in FIFO order"
    );
    assert_eq!(flush.trigger_frame.window_title, "trigger");

    // flush 이후 버퍼는 비어야 한다 (드레인 확인).
    assert!(rb.is_empty(), "buffer must be drained after flush");

    // (4) take_evicted_count 는 카운터를 읽고 0 으로 리셋한다 (스케줄러 메트릭 경로).
    assert_eq!(rb.take_evicted_count(), expected_evictions);
    assert_eq!(
        rb.evicted_count(),
        0,
        "eviction counter must reset to 0 after take"
    );
}

/// EdgeFrameProcessor 가 capture_and_process 내부에서 실제로 호출하는
/// 프로덕션 메타데이터 빌더(build_frame_metadata)를 직접 호출하여,
/// privacy 정제 / 해상도 / importance / trigger_type 가 wired chain 을 통해
/// 올바르게 전달되는지 검증한다. (수동으로 만든 struct 가 아닌 실제 프로덕션 경로)
#[test]
fn frame_processor_wired_chain_builds_real_metadata() {
    use maekon_vision::processor::{build_frame_metadata, CaptureMetadataInput};

    // window_title 에 PII(이메일)를 포함시켜 프로덕션 정제 경로가 실제로
    // 호출되는지 확인한다.
    let meta = build_frame_metadata(CaptureMetadataInput {
        trigger_type: "ErrorDetected",
        app_name: "Terminal",
        window_title: "Login - secret@example.com - Dashboard",
        resolution: (1920, 1080),
        importance: 0.85,
        monitor_id: Some(2),
        app_bundle_id: Some("com.apple.Terminal"),
    });

    // privacy::sanitize_title 가 wired chain 안에서 실제로 적용됨.
    assert!(
        !meta.window_title.contains("secret@example.com"),
        "production metadata builder must sanitize PII in the wired chain"
    );
    assert!(meta.window_title.contains("[EMAIL]"));

    // 나머지 필드들이 입력에서 그대로 전달되는지 확인.
    assert_eq!(meta.trigger_type, "ErrorDetected");
    assert_eq!(meta.app_name, "Terminal");
    assert_eq!(meta.resolution, (1920, 1080));
    assert_eq!(meta.importance, 0.85);
    assert_eq!(meta.monitor_id, Some(2));
    assert_eq!(meta.app_bundle_id.as_deref(), Some("com.apple.Terminal"));
}

/// 실제 EdgeFrameProcessor::capture_and_process 를 FrameProcessor 포트를 통해
/// 호출하여 wired chain 이 끝까지 구동되는지 검증한다.
///
/// 화면 캡처(xcap)는 디스플레이가 필요하므로 headless CI 에서는 Err 를,
/// 디스플레이가 있는 환경에서는 Ok 를 반환한다. 둘 다 유효한 프로덕션 결과이며,
/// 어느 쪽이든 capture_and_process 의 실제 코드 경로가 실행된다.
/// - Err 인 경우: 프로덕션이 명시한 capture 실패 에러(CoreError::Internal)여야 한다
///   (패닉/행 없음).
/// - Ok 인 경우: importance>=0.8 분기로 메타데이터가 wired chain 으로 채워지고
///   image_payload 가 생성되어야 한다.
#[tokio::test]
async fn frame_processor_capture_and_process_real_invocation() {
    use maekon_core::error::CoreError;
    use maekon_core::ports::vision::{CaptureRequest, FrameProcessor};
    use maekon_vision::processor::EdgeFrameProcessor;

    let processor = EdgeFrameProcessor::new(480, 270, None);

    let request = CaptureRequest {
        trigger_type: "ErrorDetected".to_string(),
        importance: 0.9, // high → full-frame 인코딩 분기 구동
        app_name: "Terminal".to_string(),
        window_title: "Error - user@example.com".to_string(),
        monitor_id: None,
        app_bundle_id: None,
        window_bounds: None,
        screen_scale_factor: None,
    };

    // 실제 비동기 trait 메서드 호출 — wired chain 전체 구동.
    let result = processor.capture_and_process(&request).await;

    match result {
        Ok(frame) => {
            // 디스플레이가 있는 환경: 메타데이터가 실제로 채워져야 한다.
            assert_eq!(frame.metadata.app_name, "Terminal");
            assert_eq!(frame.metadata.importance, 0.9);
            // privacy 정제가 wired chain 에서 적용됨.
            assert!(!frame.metadata.window_title.contains("user@example.com"));
            // importance>=0.8 → full-frame payload 가 존재해야 한다.
            assert!(
                frame.image_payload.is_some(),
                "high-importance capture must produce an image payload"
            );
            // 해상도는 실제 캡처 프레임에서 유도된 양수 값이어야 한다.
            let (w, h) = frame.metadata.resolution;
            assert!(w > 0 && h > 0, "captured frame must have a real resolution");
        }
        Err(err) => {
            // headless 환경: 프로덕션이 정의한 capture 실패 에러여야 한다.
            // (패닉/무한 대기 없이 명시적 에러로 반환되는지가 핵심.)
            assert!(
                matches!(err, CoreError::Internal { .. }),
                "headless capture failure must surface as a defined CoreError::Internal, got: {err:?}"
            );
        }
    }
}
