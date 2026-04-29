[English](./oneshim-vision.md) | [한국어](./oneshim-vision.ko.md)

# oneshim-vision

Edge 이미지 처리를 담당하는 크레이트. 스크린 캡처, 델타 인코딩, 압축, OCR을 클라이언트에서 수행.

## 역할

- **스크린 캡처**: 멀티모니터 지원, 활성 창 캡처
- **델타 인코딩**: 변경 영역만 추출하여 전송량 최소화
- **적응형 처리**: 중요도에 따른 처리 수준 조절
- **개인정보 보호**: PII 필터링 + 외부 OCR 전송 전 영역 블러
- **UI Scene 추출**: OCR 박스를 `UiElement` / `UiScene`으로 변환

## 디렉토리 구조

```
oneshim-vision/src/
├── lib.rs                  # 크레이트 루트
├── capture.rs              # ScreenCapture - xcap 기반 멀티모니터 캡처
├── trigger.rs              # SmartCaptureTrigger - 이벤트 분류 + 중요도 + 쓰로틀
├── delta.rs                # DeltaEncoder - 16×16 tile 비교 → 변경 영역 추출
├── encoder.rs              # WebpEncoder - Low/Medium/High 품질 + 통계 기반 품질 예측
├── thumbnail.rs            # ThumbnailGenerator - fast_image_resize + LRU 캐시 (100 entry, FNV-1a)
├── ring_buffer.rs          # 프레임 ring buffer (경계 있음)
├── processor.rs            # EdgeFrameProcessor - 중요도 기반 분기 통합 처리
├── ocr.rs                  # OcrExtractor - leptess/Tesseract OCR (#[cfg(feature = "ocr")])
├── local_ocr_provider.rs   # 로컬 OCR 제공자 어댑터
├── element_finder.rs       # ElementFinder - OCR 텍스트 매칭 + UiScene 구성 + R-tree 공간 인덱스(rstar)
├── work_classifier.rs      # 프레임 특성 기반 활동 분류
├── privacy.rs              # PII 필터 레벨 (Off/Basic/Standard/Strict) + 민감 앱 자동 감지
├── privacy_gateway.rs      # 중앙 프라이버시 게이트웨이 (필터 + OCR 영역 블러 래핑)
├── timeline.rs             # FrameTimeline - 인메모리 프레임 이력 + 필터
├── gui_detector/           # GUI 요소 감지 (디렉토리 모듈, ADR-003)
├── contour_classifier/     # OpenCV-like contour 기반 분류기 (디렉토리 모듈)
├── ml_classifier/          # ML 기반 분류기 추론 파이프라인 (디렉토리 모듈)
├── native_detect/          # 네이티브 플랫폼 GUI 감지 (디렉토리 모듈)
├── native_ocr/             # 네이티브 플랫폼 OCR (디렉토리 모듈)
├── accessibility/          # 플랫폼 접근성 어댑터:
│   ├── macos/              # AX tree extractor + notification observer (디렉토리 모듈)
│   ├── windows.rs          # Windows UIA CacheRequest
│   ├── linux.rs            # Linux AT-SPI (atspi 0.29)
│   └── ffi_macos.rs        # macOS FFI 헬퍼
└── error.rs                # VisionError (ADR-019 typed code)
```

## 핵심 개념: 중요도 기반 처리

이벤트 중요도에 따라 처리 수준을 조절하여 리소스 효율화:

| 중요도 | 처리 방식 | 사용 상황 |
|--------|----------|----------|
| ≥ 0.8 | Full + OCR | 창 전환, 중요 입력 |
| ≥ 0.5 | Delta 인코딩 | 일반 활동 |
| ≥ 0.3 | Thumbnail | 유휴 상태 |
| < 0.3 | Metadata only | 백그라운드 |

## 주요 컴포넌트

### ScreenCapture (capture.rs)

`xcap` 기반 화면 캡처:

```rust
pub struct ScreenCapture {
    monitors: Vec<Monitor>,
}

impl ScreenCapture {
    /// 전체 화면 캡처
    pub fn capture_screen(&self, monitor_index: usize) -> Result<CapturedFrame, CoreError>;

    /// 특정 창 캡처
    pub fn capture_window(&self, window_id: u64) -> Result<CapturedFrame, CoreError>;

    /// 특정 영역 캡처
    pub fn capture_region(&self, x: i32, y: i32, w: u32, h: u32) -> Result<CapturedFrame, CoreError>;
}
```

### SmartCaptureTrigger (trigger.rs)

캡처 여부 및 처리 수준 결정 (`CaptureTrigger` 포트):

```rust
pub struct SmartCaptureTrigger {
    throttle_ms: u64,
    last_capture: RwLock<Option<Instant>>,
}

impl CaptureTrigger for SmartCaptureTrigger {
    async fn should_capture(&self, event: &ContextEvent) -> Result<CaptureDecision, CoreError> {
        // 1. 쓰로틀링 체크
        // 2. 이벤트 타입별 중요도 계산
        // 3. CaptureDecision 반환
    }
}
```

**이벤트 중요도 매핑**:
```rust
fn calculate_importance(event: &ContextEvent) -> f64 {
    match event.event_type {
        EventType::WindowFocus => 0.9,
        EventType::ApplicationSwitch => 0.85,
        EventType::KeyboardInput => 0.7,
        EventType::MouseClick => 0.6,
        EventType::MouseMove => 0.2,
        EventType::Idle => 0.1,
        _ => 0.5,
    }
}
```

### DeltaEncoder (delta.rs)

16x16 타일 기반 변경 영역 추출:

```rust
pub struct DeltaEncoder {
    tile_size: usize,  // 기본 16
    threshold: f64,    // 변경 감지 임계값
}

impl DeltaEncoder {
    /// 두 프레임 비교, 변경된 타일만 추출
    pub fn encode(&self, prev: &[u8], curr: &[u8], width: u32, height: u32)
        -> Result<DeltaFrame, CoreError>;
}

pub struct DeltaFrame {
    pub changed_tiles: Vec<Tile>,
    pub tile_positions: Vec<(u32, u32)>,
    pub compression_ratio: f64,
}
```

**알고리즘**:
1. 이미지를 16x16 타일로 분할
2. 각 타일의 해시 비교
3. 변경된 타일만 수집
4. 타일 위치 정보와 함께 반환

### WebpEncoder (encoder.rs)

WebP 포맷 인코딩:

```rust
pub struct WebpEncoder;

pub enum QualityLevel {
    Low,     // 50% - 썸네일용
    Medium,  // 75% - 일반
    High,    // 90% - 고품질
}

impl WebpEncoder {
    pub fn encode(&self, frame: &CapturedFrame, quality: QualityLevel)
        -> Result<Vec<u8>, CoreError>;
}
```

### ThumbnailGenerator (thumbnail.rs)

`fast_image_resize` 기반 고속 리사이즈:

```rust
pub struct ThumbnailGenerator {
    width: u32,   // 기본 480
    height: u32,  // 기본 270
}

impl ThumbnailGenerator {
    pub fn generate(&self, frame: &CapturedFrame) -> Result<Vec<u8>, CoreError>;
}
```

### EdgeFrameProcessor (processor.rs)

통합 처리 파이프라인 (`FrameProcessor` 포트):

```rust
pub struct EdgeFrameProcessor {
    delta_encoder: DeltaEncoder,
    webp_encoder: WebpEncoder,
    thumbnail_gen: ThumbnailGenerator,
    ocr_extractor: Option<OcrExtractor>,
    privacy_sanitizer: PrivacySanitizer,
    prev_frame: RwLock<Option<Vec<u8>>>,
}

impl FrameProcessor for EdgeFrameProcessor {
    async fn process(&self, frame: CapturedFrame) -> Result<ProcessedFrame, CoreError> {
        let importance = frame.metadata.importance;

        let processed = if importance >= 0.8 {
            self.process_full_with_ocr(frame).await?
        } else if importance >= 0.5 {
            self.process_delta(frame).await?
        } else if importance >= 0.3 {
            self.process_thumbnail(frame).await?
        } else {
            self.process_metadata_only(frame)?
        };

        Ok(self.privacy_sanitizer.sanitize(processed)?)
    }
}
```

### OcrExtractor (ocr.rs)

Tesseract 기반 OCR (선택적 기능):

```rust
#[cfg(feature = "ocr")]
pub struct OcrExtractor {
    tesseract: leptess::LepTess,
}

impl OcrExtractor {
    pub fn extract_text(&self, image: &[u8]) -> Result<String, CoreError>;
}
```

### LocalOcrProvider + ElementFinder (`local_ocr_provider.rs`, `element_finder.rs`)

- `LocalOcrProvider`: standalone/폴백 경로에서 쓰는 로컬 OCR 어댑터
- `ElementFinder`: OCR 결과를 아래 타입으로 변환
  - 요소 단위 자동화용 `Vec<UiElement>`
  - 오버레이/좌표 실행용 `UiScene` / `UiSceneElement`

### Privacy Rules (`privacy.rs`)

PII 감지는 레벨 기반(`Off`, `Basic`, `Standard`, `Strict`)으로 동작하고 마커 단위 결과를 제공한다.

- 마커 enum: `PiiMarker::{Email, Phone, Card, KoreanId, ApiKey, Ip, UserPath}`
- 주요 API:
  - `sanitize_title_with_level()`
  - `detect_pii_markers_with_level()`
  - `is_sensitive_segment_with_level()`
- 업로드/OCR 게이트에서 사용하는 민감 앱/패턴 제외 규칙도 포함

### PrivacyGateway (`privacy_gateway.rs`)

`PrivacyGateway`는 외부 OCR 경계에서 프라이버시 통제를 담당한다.

- 게이트 검사:
  - 동의(`ConsentManager`)
  - 민감 앱 차단
  - 앱/제목 제외 정책
- 세정 결과:
  - `SanitizedImage { image_data, metadata_stripped, redacted_regions }`
- 블러 파이프라인 (`blur_pii_regions()`):
  - OCR 워드 박스 추출
  - 단일 워드 PII 감지
  - 분절된 토큰(이메일/전화번호 등) 보강을 위한 2~5 워드 세그먼트 감지
  - 영역 병합(`merge_sensitive_regions`)
  - 병합 영역 바운딩 박스 블러 적용
- opt-out:
  - `allow_unredacted_external_ocr=true`면 원본 이미지 패스스루 허용

## 처리 파이프라인

```
┌─────────────┐    ┌────────────────┐    ┌───────────────┐
│  ContextEvent │──▶│ CaptureTrigger │──▶│ ScreenCapture │
└─────────────┘    └────────────────┘    └───────────────┘
                          │                      │
                          ▼                      ▼
                   CaptureDecision         CapturedFrame
                          │                      │
                          └───────────┬──────────┘
                                      ▼
                            ┌─────────────────┐
                            │ FrameProcessor  │
                            │  (importance별)  │
                            └─────────────────┘
                                      │
                    ┌─────────────────┼─────────────────┐
                    ▼                 ▼                 ▼
              ┌──────────┐    ┌─────────────┐    ┌───────────┐
              │ Full+OCR │    │ Delta Only  │    │ Thumbnail │
              └──────────┘    └─────────────┘    └───────────┘
                    │                 │                 │
                    └─────────────────┼─────────────────┘
                                      ▼
                            ┌─────────────────┐
                            │ PrivacySanitizer│
                            └─────────────────┘
                                      │
                                      ▼
                              ProcessedFrame
```

## 외부 OCR 프라이버시 경로

원격 OCR 제공자 사용 시 흐름:

1. `PrivacyGateway::sanitize_image_for_external_policy()` 실행
2. 세정된 이미지를 원격 OCR로 전송
3. 앱 계층 어댑터에서 calibration/validation 적용 후 결과 사용

## 의존성

- `xcap`: 크로스플랫폼 화면 캡처
- `image`: 이미지 처리
- `fast_image_resize`: 고속 리사이즈
- `webp`: WebP 인코딩
- `leptess`: Tesseract OCR (optional)
- `regex`: PII 패턴 매칭

## 성능 최적화

1. **타일 기반 델타**: 전체 프레임 대신 변경 영역만 전송
2. **적응형 품질**: 중요도에 따른 압축률 조절
3. **쓰로틀링**: 최소 캡처 간격으로 CPU 부하 제한
4. **비동기 처리**: OCR 등 무거운 작업은 별도 태스크

## 테스트

```rust
#[test]
fn test_delta_encoding() {
    let encoder = DeltaEncoder::new(16, 0.01);

    // 동일 프레임: 변경 없음
    let frame1 = vec![0u8; 1024];
    let frame2 = frame1.clone();
    let delta = encoder.encode(&frame1, &frame2, 32, 32).unwrap();
    assert!(delta.changed_tiles.is_empty());

    // 일부 변경: 해당 타일만 포함
    let mut frame3 = frame1.clone();
    frame3[0] = 255;
    let delta = encoder.encode(&frame1, &frame3, 32, 32).unwrap();
    assert_eq!(delta.changed_tiles.len(), 1);
}
```
