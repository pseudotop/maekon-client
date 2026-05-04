[English](./oneshim-vision.md) | [한국어](./oneshim-vision.ko.md)

# oneshim-vision

The crate responsible for edge image processing. Performs screen capture, delta encoding, compression, and OCR on the client side.

## Role

- **Screen Capture**: Multi-monitor support, active window capture
- **Delta Encoding**: Extracts only changed regions to minimize transfer volume
- **Adaptive Processing**: Adjusts processing level based on importance
- **Privacy Protection**: PII filtering + OCR-region redaction before external OCR
- **UI Scene Extraction**: OCR boxes to `UiElement` / `UiScene` conversion

## Directory Structure

```
oneshim-vision/src/
├── lib.rs                  # Crate root
├── capture.rs              # ScreenCapture - multi-monitor capture via xcap
├── trigger.rs              # SmartCaptureTrigger - event classification + importance + throttle
├── delta.rs                # DeltaEncoder - 16×16 tile comparison → changed region extraction
├── encoder.rs              # WebpEncoder - Low/Medium/High quality + stat-based quality prediction
├── thumbnail.rs            # ThumbnailGenerator - fast_image_resize + LRU cache (100 entries, FNV-1a)
├── ring_buffer.rs          # Bounded frame ring buffer
├── processor.rs            # EdgeFrameProcessor - importance-branched unified processing
├── ocr.rs                  # OcrExtractor - leptess/Tesseract OCR (#[cfg(feature = "ocr")])
├── local_ocr_provider.rs   # Local OCR provider adapter
├── element_finder.rs       # ElementFinder - OCR text matching + UiScene builder + R-tree spatial index (rstar)
├── work_classifier.rs      # Activity classification from frame features
├── privacy.rs              # PII filter levels (Off/Basic/Standard/Strict) + sensitive-app auto-detection
├── privacy_gateway.rs      # Central privacy gateway wrapping filter + OCR-region blur
├── timeline.rs             # FrameTimeline - in-memory frame history + filters
├── gui_detector/           # GUI element detection (directory module, ADR-003)
├── contour_classifier/     # OpenCV-like contour-based classifier (directory module)
├── ml_classifier/          # ML-based classifier inference pipeline (directory module)
├── native_detect/          # Native platform GUI detection (directory module)
├── native_ocr/             # Native platform OCR (directory module)
├── accessibility/          # Platform accessibility adapters:
│   ├── macos/              # AX tree extractor + notification observer (directory module)
│   ├── windows.rs          # Windows UIA CacheRequest
│   ├── linux.rs            # Linux AT-SPI (atspi 0.29)
│   └── ffi_macos.rs        # macOS FFI helpers
└── error.rs                # VisionError (ADR-019 typed codes)
```

## Core Concept: Importance-Based Processing

Adjusts processing level based on event importance for resource efficiency:

| Importance | Processing Method | Use Case |
|------------|-------------------|----------|
| ≥ 0.8 | Full + OCR | Window switch, important input |
| ≥ 0.5 | Delta encoding | Normal activity |
| ≥ 0.3 | Thumbnail | Idle state |
| < 0.3 | Metadata only | Background |

## Key Components

### ScreenCapture (capture.rs)

Screen capture based on `xcap`:

```rust
pub struct ScreenCapture {
    monitors: Vec<Monitor>,
}

impl ScreenCapture {
    /// Full screen capture
    pub fn capture_screen(&self, monitor_index: usize) -> Result<CapturedFrame, CoreError>;

    /// Specific window capture
    pub fn capture_window(&self, window_id: u64) -> Result<CapturedFrame, CoreError>;

    /// Specific region capture
    pub fn capture_region(&self, x: i32, y: i32, w: u32, h: u32) -> Result<CapturedFrame, CoreError>;
}
```

### SmartCaptureTrigger (trigger.rs)

Decides whether to capture and the processing level (`CaptureTrigger` port):

```rust
pub struct SmartCaptureTrigger {
    throttle_ms: u64,
    last_capture: RwLock<Option<Instant>>,
}

impl CaptureTrigger for SmartCaptureTrigger {
    async fn should_capture(&self, event: &ContextEvent) -> Result<CaptureDecision, CoreError> {
        // 1. Throttling check
        // 2. Calculate importance by event type
        // 3. Return CaptureDecision
    }
}
```

**Event Importance Mapping**:
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

Changed region extraction based on 16x16 tiles:

```rust
pub struct DeltaEncoder {
    tile_size: usize,  // Default 16
    threshold: f64,    // Change detection threshold
}

impl DeltaEncoder {
    /// Compare two frames, extract only changed tiles
    pub fn encode(&self, prev: &[u8], curr: &[u8], width: u32, height: u32)
        -> Result<DeltaFrame, CoreError>;
}

pub struct DeltaFrame {
    pub changed_tiles: Vec<Tile>,
    pub tile_positions: Vec<(u32, u32)>,
    pub compression_ratio: f64,
}
```

**Algorithm**:
1. Divide image into 16x16 tiles
2. Compare hash of each tile
3. Collect only changed tiles
4. Return with tile position information

### WebpEncoder (encoder.rs)

WebP format encoding:

```rust
pub struct WebpEncoder;

pub enum QualityLevel {
    Low,     // 50% - for thumbnails
    Medium,  // 75% - standard
    High,    // 90% - high quality
}

impl WebpEncoder {
    pub fn encode(&self, frame: &CapturedFrame, quality: QualityLevel)
        -> Result<Vec<u8>, CoreError>;
}
```

### ThumbnailGenerator (thumbnail.rs)

Fast resize based on `fast_image_resize`:

```rust
pub struct ThumbnailGenerator {
    width: u32,   // Default 480
    height: u32,  // Default 270
}

impl ThumbnailGenerator {
    pub fn generate(&self, frame: &CapturedFrame) -> Result<Vec<u8>, CoreError>;
}
```

### EdgeFrameProcessor (processor.rs)

Unified processing pipeline (`FrameProcessor` port):

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

Tesseract-based OCR (optional feature):

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

- `LocalOcrProvider`: local OCR adapter used by standalone and fallback paths
- `ElementFinder`: converts OCR results into:
  - `Vec<UiElement>` for element-level automation
  - `UiScene` / `UiSceneElement` for scene overlays and coordinate-driven actions

### Privacy Rules (`privacy.rs`)

PII detection is level-based (`Off`, `Basic`, `Standard`, `Strict`) and exposes marker-level results:

- Marker enum: `PiiMarker::{Email, Phone, Card, KoreanId, ApiKey, Ip, UserPath}`
- APIs:
  - `sanitize_title_with_level()`
  - `detect_pii_markers_with_level()`
  - `is_sensitive_segment_with_level()`
- Includes sensitive app/pattern exclusion checks used by upload and OCR gateways

### PrivacyGateway (`privacy_gateway.rs`)

`PrivacyGateway` handles external OCR boundary controls:

- Gate checks:
  - consent (`ConsentManager`)
  - sensitive app deny
  - app/title exclusion policy
- Sanitized output:
  - `SanitizedImage { image_data, metadata_stripped, redacted_regions }`
- Redaction pipeline (`blur_pii_regions()`):
  - OCR word-box extraction
  - single-word PII detection
  - 2~5 word segment PII detection for split tokens (email/phone, etc.)
  - region merge (`merge_sensitive_regions`)
  - blur application over merged bounding boxes
- Opt-out:
  - `allow_unredacted_external_ocr=true` allows raw image pass-through

## Processing Pipeline

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
                            │ (by importance) │
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

## External OCR Privacy Path

For remote OCR providers, Maekon uses:

1. `PrivacyGateway::sanitize_image_for_external_policy()`
2. send sanitized image to remote OCR
3. consume results with calibration/validation in app-layer adapter

## Dependencies

- `xcap`: Cross-platform screen capture
- `image`: Image processing
- `fast_image_resize`: Fast resize
- `webp`: WebP encoding
- `leptess`: Tesseract OCR (optional)
- `regex`: PII pattern matching

## Performance Optimizations

1. **Tile-based delta**: Transmit only changed regions instead of full frames
2. **Adaptive quality**: Adjust compression ratio based on importance
3. **Throttling**: Limit CPU load with minimum capture intervals
4. **Async processing**: Heavy tasks like OCR run in separate tasks

## Tests

```rust
#[test]
fn test_delta_encoding() {
    let encoder = DeltaEncoder::new(16, 0.01);

    // Identical frames: no changes
    let frame1 = vec![0u8; 1024];
    let frame2 = frame1.clone();
    let delta = encoder.encode(&frame1, &frame2, 32, 32).unwrap();
    assert!(delta.changed_tiles.is_empty());

    // Partial change: only the affected tile included
    let mut frame3 = frame1.clone();
    frame3[0] = 255;
    let delta = encoder.encode(&frame1, &frame3, 32, 32).unwrap();
    assert_eq!(delta.changed_tiles.len(), 1);
}
```
