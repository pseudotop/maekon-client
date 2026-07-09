use async_trait::async_trait;
use chrono::Utc;
use image::DynamicImage;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::frame::{FrameMetadata, ImagePayload, ProcessedFrame};
use maekon_core::ports::vision::{CaptureRequest, FrameProcessor};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

use crate::capture::ScreenCapture;
use crate::delta;
use crate::encoder::{self, WebPQuality};
use crate::privacy;
use crate::thumbnail;

pub struct EdgeFrameProcessor {
    capture: ScreenCapture,
    prev_frame: Mutex<Option<Arc<DynamicImage>>>,
    thumbnail_width: u32,
    thumbnail_height: u32,
    /// PII filter level applied to OCR output at the source, including the
    /// per-word `ProcessedFrame.ocr_regions` text (#6088).
    pii_level: PiiFilterLevel,
    /// Long-lived OCR extractor shared across frames. Wrapped in `Arc` so it can
    /// be cloned cheaply into the per-frame `spawn_blocking` closure while
    /// preserving the cached `LepTess` instance (`OcrExtractor` is `Send + Sync`
    /// via `Mutex<Option<LepTess>>`). Reusing the same extractor avoids
    /// re-initializing Tesseract (~10-50ms) on every high-importance frame
    /// (#6132).
    #[cfg(feature = "ocr")]
    ocr_extractor: Option<Arc<crate::ocr::OcrExtractor>>,
    /// Fallback OCR source for shipped builds that do NOT compile the leptess
    /// (`feature = "ocr"`) Tesseract path. Release + CI builds run with
    /// `--features grpc`, which does not enable `ocr`, so without this the
    /// capture pipeline emitted empty OCR forever — starving `last_ocr_regions`
    /// (GUI element click-to-element correlation) and the ML crop classifier.
    /// Wired to `native_ocr::create_native_ocr()` (macOS Vision.framework /
    /// Windows Media.Ocr); `None` where no native provider exists (e.g. Linux)
    /// or when explicitly overridden. Wrapped in `Arc` for cheap per-frame
    /// cloning into the async OCR call (#7477).
    #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
    ocr_provider: Option<Arc<dyn maekon_core::ports::ocr_provider::OcrProvider>>,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureMetadataInput<'a> {
    pub trigger_type: &'a str,
    pub app_name: &'a str,
    pub window_title: &'a str,
    pub resolution: (u32, u32),
    pub importance: f32,
    pub monitor_id: Option<usize>,
    pub app_bundle_id: Option<&'a str>,
    /// Configured PII filter level applied to the window title (review4 V8). The
    /// title was previously masked at a hardcoded Standard, silently downgrading
    /// Strict (leaving IP / API-key / passport in the persisted + SSE'd title).
    pub pii_level: PiiFilterLevel,
}

/// Mask the raw text of every OCR region with the configured PII level (#6088).
///
/// `OcrExtractor` produces per-word regions whose `text` is the raw recognized
/// string. Sanitizing here — at the single source where regions are populated —
/// guarantees that no downstream consumer (e.g. the `analyze_current_scene`
/// Tauri command, which surfaces region text directly and as GUI labels) can
/// leak unredacted PII, mirroring the masking already applied to the full OCR
/// text.
fn sanitize_ocr_regions(
    regions: Vec<maekon_core::models::frame::OcrRegion>,
    pii_level: PiiFilterLevel,
) -> Vec<maekon_core::models::frame::OcrRegion> {
    regions
        .into_iter()
        .map(|mut region| {
            region.text = privacy::sanitize_title_with_level(&region.text, pii_level);
            region
        })
        .collect()
}

/// Whether a high-importance, OCR-permitted capture should emit the
/// missing-OCR-capability warning (#7602 audit ①).
///
/// `true` exactly when OCR was requested (`ocr_processing_permitted`) but no
/// OCR source actually ran (`!ocr_active`) — i.e. the emptiness is a platform
/// capability gap (e.g. Linux, where `native_ocr::create_native_ocr()` always
/// returns `None` and leptess is not compiled into shipped builds), NOT the
/// expected "consent withheld" case (`ocr_processing_permitted == false`,
/// which also yields `ocr_active == false` but must stay silent).
///
/// Pure predicate, decoupled from the `Once`-guarded logging side effect in
/// [`warn_ocr_platform_unavailable_once`] so the decision itself is
/// unit-testable without a tracing subscriber harness.
const fn should_warn_ocr_platform_unavailable(
    ocr_processing_permitted: bool,
    ocr_active: bool,
) -> bool {
    ocr_processing_permitted && !ocr_active
}

/// Emits a single `warn!` for the lifetime of the process the first time a
/// high-importance capture requests OCR but no OCR source (leptess or
/// platform-native) is available.
///
/// Before #7602, this platform gap was completely silent: every Linux
/// shipped build (no leptess, `native_ocr::create_native_ocr()` == `None`)
/// emitted `ocr_regions: []` on every high-importance frame forever, with no
/// error and no signal an operator could act on. Guarded by [`std::sync::Once`]
/// so the gap logs exactly once per process instead of once per frame.
fn warn_ocr_platform_unavailable_once() {
    static WARN_ONCE: std::sync::Once = std::sync::Once::new();
    WARN_ONCE.call_once(|| {
        tracing::warn!(
            "OCR was requested for a high-importance capture, but no OCR engine is \
             compiled/available on this platform (the 'ocr' leptess feature is off \
             and no native OCR provider exists for this OS). OCR regions and OCR \
             text will be empty for every frame until a supported OCR source is \
             enabled. This message logs once per process."
        );
    });
}

/// Convert an `OcrProvider` element (absolute source pixels with an `i32`
/// origin) into a frame `OcrRegion` (`u32` pixel bbox). Negative origins are
/// clamped to `0`. Used by the native-OCR capture path that supplies OCR to
/// shipped builds when leptess (`feature = "ocr"`) is not compiled in (#7477).
#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
fn ocr_result_to_region(
    result: &maekon_core::ports::ocr_provider::OcrResult,
) -> maekon_core::models::frame::OcrRegion {
    maekon_core::models::frame::OcrRegion {
        text: result.text.clone(),
        bbox: maekon_core::models::frame::BoundingBox {
            x: result.x.max(0) as u32,
            y: result.y.max(0) as u32,
            width: result.width,
            height: result.height,
        },
        confidence: result.confidence as f32,
    }
}

pub fn build_frame_metadata(input: CaptureMetadataInput<'_>) -> FrameMetadata {
    FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: input.trigger_type.to_string(),
        app_name: input.app_name.to_string(),
        window_title: privacy::sanitize_title_with_level(input.window_title, input.pii_level),
        resolution: input.resolution,
        importance: input.importance,
        monitor_id: input.monitor_id,
        app_bundle_id: input.app_bundle_id.map(ToString::to_string),
    }
}

impl EdgeFrameProcessor {
    #[allow(unused_variables)]
    pub fn new(thumbnail_width: u32, thumbnail_height: u32, ocr_tessdata: Option<PathBuf>) -> Self {
        Self::with_pii_level(
            thumbnail_width,
            thumbnail_height,
            ocr_tessdata,
            PiiFilterLevel::Standard,
        )
    }

    /// Construct a processor with an explicit PII filter level so OCR text
    /// (full text + per-region `ocr_regions`) is sanitized at the source with
    /// the configured policy rather than the `Standard` default (#6088).
    #[allow(unused_variables)]
    pub fn with_pii_level(
        thumbnail_width: u32,
        thumbnail_height: u32,
        ocr_tessdata: Option<PathBuf>,
        pii_level: PiiFilterLevel,
    ) -> Self {
        Self {
            capture: ScreenCapture::new(),
            prev_frame: Mutex::new(None),
            thumbnail_width,
            thumbnail_height,
            pii_level,
            #[cfg(feature = "ocr")]
            ocr_extractor: Some(Arc::new(
                ocr_tessdata
                    .map(|p| crate::ocr::OcrExtractor::new(Some(p)))
                    .unwrap_or_else(|| crate::ocr::OcrExtractor::new(None)),
            )),
            // Default the fallback OCR source to the platform-native provider so
            // shipped (non-leptess) builds actually produce OCR text + regions.
            // `None` on platforms without a native provider (e.g. Linux) (#7477).
            #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
            ocr_provider: crate::native_ocr::create_native_ocr(),
        }
    }

    /// Override the fallback OCR provider used when leptess (`feature = "ocr"`)
    /// is not compiled in. Lets the composition root — or a unit test — inject a
    /// specific `OcrProvider` in place of the default
    /// `native_ocr::create_native_ocr()` platform provider (#7477).
    #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
    pub fn with_ocr_provider(
        mut self,
        provider: Option<Arc<dyn maekon_core::ports::ocr_provider::OcrProvider>>,
    ) -> Self {
        self.ocr_provider = provider;
        self
    }
}

#[async_trait]
impl FrameProcessor for EdgeFrameProcessor {
    async fn capture_and_process(
        &self,
        capture_request: &CaptureRequest,
    ) -> Result<ProcessedFrame, CoreError> {
        // Screen capture is a blocking OS syscall (xcap). Offload to the
        // blocking thread pool so the Tokio scheduler is not stalled.
        let capture = self.capture.clone();
        let window_bounds = capture_request.window_bounds;
        let (captured_frame, selected_monitor_id) = tokio::task::spawn_blocking(move || {
            capture.capture_for_window_with_monitor(window_bounds.as_ref())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("capture task panicked: {e}"),
        })?
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("screen capture failed: {e}"),
        })?;
        self.process_captured_frame(captured_frame, capture_request, selected_monitor_id)
            .await
    }

    async fn capture_thumbnail(&self) -> Result<Vec<u8>, CoreError> {
        let capture = self.capture.clone();
        let tw = self.thumbnail_width;
        let th = self.thumbnail_height;
        tokio::task::spawn_blocking(move || {
            let frame = capture.capture_primary()?;
            let thumb = thumbnail::resize_to_fit(&frame, tw, th)?;
            let encoded = encoder::encode_webp_base64(&thumb, WebPQuality::Low)?;
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&encoded)
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("base64 decode failed: {e}"),
                })
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("thumbnail task panicked: {e}"),
        })?
    }
}

impl EdgeFrameProcessor {
    /// Process an already-captured frame: build metadata, run the
    /// importance-branched encode/OCR pipeline, refresh the delta baseline, and
    /// return the `ProcessedFrame`. Split out of `capture_and_process` so the
    /// image-processing pipeline (including the OCR path) is unit-testable with a
    /// synthetic frame, without needing a live screen-capture display (#7477).
    pub async fn process_captured_frame(
        &self,
        captured_frame: DynamicImage,
        capture_request: &CaptureRequest,
        selected_monitor_id: Option<usize>,
    ) -> Result<ProcessedFrame, CoreError> {
        let importance = capture_request.importance;
        let current_frame = Arc::new(captured_frame);
        let (w, h) = (current_frame.width(), current_frame.height());

        let metadata = build_frame_metadata(CaptureMetadataInput {
            trigger_type: &capture_request.trigger_type,
            app_name: &capture_request.app_name,
            window_title: &capture_request.window_title,
            resolution: (w, h),
            importance,
            monitor_id: capture_request.monitor_id.or(selected_monitor_id),
            app_bundle_id: capture_request.app_bundle_id.as_deref(),
            pii_level: self.pii_level,
        });

        let mut ocr_regions = Vec::new();
        let mut raw_rgba: Option<Vec<u8>> = None;

        let image_payload = if importance >= 0.8 {
            debug!("frame (in progress {:.1})", importance);
            let frame_ref = Arc::clone(&current_frame);

            // Leptess (Tesseract) extractor — only compiled with `feature = "ocr"`.
            // Reuse the long-lived cached extractor (#6132); pass `None` when
            // ocr_processing consent is absent so no OCR work runs.
            #[cfg(feature = "ocr")]
            let ocr_extractor = if capture_request.ocr_processing_permitted {
                self.ocr_extractor.clone()
            } else {
                None
            };

            // Native OS OCR provider — the OCR source for shipped builds that do
            // NOT compile leptess (release + CI run `--features grpc`). Without
            // this the capture pipeline produced empty OCR on every frame (#7477).
            // Gated so it never runs when the leptess path is present, and cleared
            // to `None` when ocr_processing consent is absent.
            #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
            let ocr_provider = if capture_request.ocr_processing_permitted {
                self.ocr_provider.clone()
            } else {
                None
            };

            // Whether any OCR source will run for this frame. Drives raw-RGBA
            // population so ML crop data is available whenever OCR is attempted,
            // even if the frame legitimately yields zero regions — #7477 decouples
            // this from `sanitized.is_empty()`.
            #[cfg(feature = "ocr")]
            let ocr_active = ocr_extractor.is_some();
            #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
            let ocr_active = ocr_provider.is_some();
            #[cfg(all(not(feature = "ocr"), not(feature = "native-vision")))]
            let ocr_active = false;

            // #7602 (audit ①): surface a platform capability gap (no OCR
            // source at all — e.g. Linux) as an explicit, one-time signal
            // instead of a silent forever-empty `ocr_regions: []`. Does NOT
            // fire when OCR is simply unrequested (consent withheld).
            if should_warn_ocr_platform_unavailable(
                capture_request.ocr_processing_permitted,
                ocr_active,
            ) {
                warn_ocr_platform_unavailable_once();
            }

            let scale_factor = capture_request.screen_scale_factor;
            // #6315: pii_level is needed for the per-region sanitize that runs
            // INSIDE the blocking closure, so capture it unconditionally.
            let pii_level = self.pii_level;

            // Blocking stage: CPU-heavy WebP encode + (leptess OCR) + raw-pixel
            // prep, all off the async reactor. A single WebP encode is reused for
            // storage (base64) and, under the native path, as the OCR input (raw
            // bytes) — avoiding a second full-frame encode.
            #[allow(unused_variables)]
            let (encoded, ocr_input, blocking_text, blocking_regions, raw_rgba_val) =
                tokio::task::spawn_blocking(move || {
                    let webp_bytes = encoder::encode_webp(&frame_ref, WebPQuality::High)?;
                    let enc = {
                        use base64::Engine as _;
                        base64::engine::general_purpose::STANDARD.encode(&webp_bytes)
                    };

                    #[cfg(feature = "ocr")]
                    let (text_val, raw_regions) = match ocr_extractor.as_ref() {
                        Some(extractor) => {
                            let text = match extractor.extract(&frame_ref) {
                                Ok(t) if !t.is_empty() => {
                                    Some(crate::privacy::sanitize_title_with_level(&t, pii_level))
                                }
                                _ => None,
                            };
                            let regions = match extractor.extract_regions(&frame_ref) {
                                Ok(r) => r,
                                Err(e) => {
                                    tracing::warn!("OCR region extraction failure: {e}");
                                    Vec::new()
                                }
                            };
                            (text, regions)
                        }
                        None => (None, Vec::new()),
                    };
                    #[cfg(not(feature = "ocr"))]
                    let (text_val, raw_regions): (
                        Option<String>,
                        Vec<maekon_core::models::frame::OcrRegion>,
                    ) = (None, Vec::new());

                    // Leptess regions: scale + PII-sanitize here so this
                    // per-pixel/masking CPU stays off the async reactor (#6315,
                    // #6088). No-op empty under the native path (regions are filled
                    // on the async side below).
                    let scaled = crate::ocr_geometry::scale_ocr_regions_to_logical(
                        &raw_regions,
                        scale_factor,
                    );
                    let sanitized = sanitize_ocr_regions(scaled, pii_level);

                    // Reuse the WebP bytes as the native-OCR input only when a
                    // native provider is wired (otherwise drop them so the buffer
                    // is not retained).
                    #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
                    let ocr_input: Option<Vec<u8>> =
                        if ocr_active { Some(webp_bytes) } else { None };
                    #[cfg(any(feature = "ocr", not(feature = "native-vision")))]
                    let ocr_input: Option<Vec<u8>> = None;

                    // ML crop pixels: populated whenever an OCR attempt is active,
                    // decoupled from whether that attempt produced regions (#7477).
                    let raw_rgba_opt = if ocr_active {
                        Some(frame_ref.to_rgba8().into_vec())
                    } else {
                        None
                    };

                    Ok::<_, crate::error::VisionError>((
                        enc,
                        ocr_input,
                        text_val,
                        sanitized,
                        raw_rgba_opt,
                    ))
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("encode task panicked: {e}"),
                })??;

            // Async stage: native OS OCR (macOS Vision / Windows Media.Ocr). The
            // provider is an async port whose native impls offload their own
            // blocking work, so it runs here rather than inside the closure above.
            // Regions pass through the SAME `scale_ocr_regions_to_logical` +
            // `sanitize_ocr_regions` as the leptess path — PII masking is applied
            // identically and never weakened (#7477).
            #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
            let (ocr_text, ocr_regions_final) = match (ocr_provider.as_ref(), ocr_input.as_ref()) {
                (Some(provider), Some(webp)) => match provider.extract_elements(webp, "webp").await
                {
                    Ok(results) => {
                        let raw_regions: Vec<maekon_core::models::frame::OcrRegion> =
                            results.iter().map(ocr_result_to_region).collect();
                        let scaled = crate::ocr_geometry::scale_ocr_regions_to_logical(
                            &raw_regions,
                            scale_factor,
                        );
                        let sanitized = sanitize_ocr_regions(scaled, pii_level);
                        // Page-level OCR text hint = joined masked region texts
                        // (already sanitized above, so no PII leaks into the hint).
                        let text = if sanitized.is_empty() {
                            None
                        } else {
                            Some(
                                sanitized
                                    .iter()
                                    .map(|r| r.text.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            )
                        };
                        (text, sanitized)
                    }
                    Err(e) => {
                        tracing::warn!(err.code = %e.code(), "native OCR extraction failed: {e}");
                        (None, Vec::new())
                    }
                },
                _ => (None, Vec::new()),
            };
            #[cfg(any(feature = "ocr", not(feature = "native-vision")))]
            let (ocr_text, ocr_regions_final) = (blocking_text, blocking_regions);

            ocr_regions = ocr_regions_final;
            raw_rgba = raw_rgba_val;
            Some(ImagePayload::Full {
                data: encoded,
                format: "webp".to_string(),
                ocr_text,
            })
        } else if importance >= 0.5 {
            debug!("(in progress {:.1})", importance);
            // Snapshot the previous frame under a SHORT lock (cheap Arc clone), then
            // run the O(W*H) delta scan AND the encode together off the reactor in a
            // single spawn_blocking. The scan must not run on the async worker nor
            // hold prev_frame across the CPU burst (review4 V9). This also collapses
            // the previously-redundant second lock acquisition.
            let prev_snapshot = self
                .prev_frame
                .lock()
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("prev_frame lock poisoned: {e}"),
                })?
                .clone();

            if prev_snapshot.is_some() {
                let frame_ref = Arc::clone(&current_frame);
                let delta_encoded = tokio::task::spawn_blocking(move || {
                    let region = match prev_snapshot.as_ref() {
                        Some(prev) => delta::compute_delta(prev, &frame_ref),
                        None => None,
                    };
                    match region {
                        Some(region) => {
                            let encoded =
                                encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)?;
                            Ok::<_, crate::error::VisionError>(Some((encoded, region)))
                        }
                        None => Ok(None),
                    }
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("delta task panicked: {e}"),
                })??;
                delta_encoded.map(|(encoded, delta_region)| ImagePayload::Delta {
                    data: encoded,
                    region: delta_region.region,
                    changed_ratio: delta_region.changed_ratio,
                })
            } else {
                let frame_ref = Arc::clone(&current_frame);
                let encoded = tokio::task::spawn_blocking(move || {
                    encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("encode task panicked: {e}"),
                })??;
                Some(ImagePayload::Full {
                    data: encoded,
                    format: "webp".to_string(),
                    ocr_text: None,
                })
            }
        } else if importance >= 0.3 {
            debug!("(in progress {:.1})", importance);
            let tw = self.thumbnail_width;
            let th = self.thumbnail_height;
            let frame_ref = Arc::clone(&current_frame);
            let encoded = tokio::task::spawn_blocking(move || {
                let thumb = thumbnail::resize_to_fit(&frame_ref, tw, th)?;
                let (width, height) = (thumb.width(), thumb.height());
                let data = encoder::encode_webp_base64(&thumb, WebPQuality::Low)?;
                Ok::<_, crate::error::VisionError>((data, width, height))
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("encode task panicked: {e}"),
            })??;
            Some(ImagePayload::Thumbnail {
                data: encoded.0,
                width: encoded.1,
                height: encoded.2,
            })
        } else {
            debug!("(in progress {:.1})", importance);
            None
        };

        *self.prev_frame.lock().map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("prev_frame lock poisoned: {e}"),
        })? = Some(current_frame);

        Ok(ProcessedFrame {
            metadata,
            image_payload,
            ocr_regions,
            raw_rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use image::{DynamicImage, RgbaImage};

    fn make_test_image(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            w,
            h,
            image::Rgba([100, 150, 200, 255]),
        ))
    }

    #[test]
    fn processor_creation() {
        let proc = EdgeFrameProcessor::new(480, 270, None);
        assert_eq!(proc.thumbnail_width, 480);
        assert_eq!(proc.thumbnail_height, 270);
        assert!(proc.prev_frame.lock().unwrap().is_none());
    }

    // #7602 (audit ①): the missing-OCR-capability warning must fire ONLY when
    // OCR was actually requested but produced no active source (the platform
    // capability gap — e.g. Linux) — never when OCR was simply not requested
    // (consent withheld, which also yields `ocr_active == false` but is an
    // expected, silent case). This truth table has no prior equivalent: before
    // #7602 the pipeline never distinguished these two "empty OCR" causes at
    // all, so this predicate (and therefore this test) did not exist.
    #[test]
    fn ocr_unavailable_warning_fires_only_when_requested_but_inactive() {
        assert!(
            should_warn_ocr_platform_unavailable(true, false),
            "OCR requested but no source ran (platform gap) must warn"
        );
        assert!(
            !should_warn_ocr_platform_unavailable(true, true),
            "OCR requested and a source ran must not warn"
        );
        assert!(
            !should_warn_ocr_platform_unavailable(false, false),
            "OCR not requested (consent withheld) must stay silent even though ocr_active is also false"
        );
        assert!(
            !should_warn_ocr_platform_unavailable(false, true),
            "OCR not requested must stay silent regardless of ocr_active"
        );
    }

    // Smoke test: the `Once`-guarded logging side effect itself must not
    // panic when invoked (repeatedly, from concurrent-looking call sites)
    // even though its actual firing is exercised indirectly via the
    // predicate above and the pipeline integration tests below.
    #[test]
    fn ocr_unavailable_warning_once_does_not_panic_on_repeated_calls() {
        warn_ocr_platform_unavailable_once();
        warn_ocr_platform_unavailable_once();
    }

    #[test]
    fn full_frame_encoding_high_importance() {
        let img = make_test_image(640, 480);
        let encoded = encoder::encode_webp_base64(&img, WebPQuality::High).unwrap();
        assert!(!encoded.is_empty());
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        assert!(!decoded.is_empty());
    }

    #[test]
    fn delta_encoding_medium_importance() {
        let img1 = make_test_image(100, 100);
        let img2 = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            100,
            100,
            image::Rgba([200, 50, 50, 255]),
        ));
        let result = delta::compute_delta(&img1, &img2);
        assert!(result.is_some());
        let dr = result.unwrap();
        assert!(dr.changed_ratio > 0.0);
    }

    #[test]
    fn thumbnail_generation_low_importance() {
        let img = make_test_image(1920, 1080);
        let thumb = thumbnail::fast_resize(&img, 480, 270).unwrap();
        assert_eq!(thumb.width(), 480);
        assert_eq!(thumb.height(), 270);
    }

    #[test]
    fn privacy_sanitization_in_pipeline() {
        let title = "Login - admin@company.com - Firefox";
        let sanitized = privacy::sanitize_title(title);
        assert!(sanitized.contains("[EMAIL]"));
        assert!(!sanitized.contains("admin@company.com"));
    }

    fn region(text: &str) -> maekon_core::models::frame::OcrRegion {
        maekon_core::models::frame::OcrRegion {
            text: text.to_string(),
            bbox: maekon_core::models::frame::BoundingBox {
                x: 0,
                y: 0,
                width: 100,
                height: 20,
            },
            confidence: 0.9,
        }
    }

    // #6088: per-region OCR text must be masked at the source so that
    // analyze_current_scene cannot expose raw PII via region text or GUI labels.
    #[test]
    fn ocr_regions_text_is_sanitized_at_source() {
        let regions = vec![
            region("contact admin@company.com now"),
            region("plain label"),
        ];
        let masked = sanitize_ocr_regions(regions, PiiFilterLevel::Standard);

        assert!(masked[0].text.contains("[EMAIL]"));
        assert!(!masked[0].text.contains("admin@company.com"));
        // Geometry and non-PII text are preserved unchanged.
        assert_eq!(masked[0].bbox.width, 100);
        assert_eq!(masked[1].text, "plain label");
    }

    #[test]
    fn ocr_regions_respect_configured_pii_level() {
        // Off must leave region text untouched (explicit operator opt-out).
        let off = sanitize_ocr_regions(vec![region("admin@company.com")], PiiFilterLevel::Off);
        assert_eq!(off[0].text, "admin@company.com");
    }

    #[test]
    fn with_pii_level_stores_configured_level() {
        let proc = EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Strict);
        assert_eq!(proc.pii_level, PiiFilterLevel::Strict);
        // The default constructor keeps the Standard policy.
        let default_proc = EdgeFrameProcessor::new(480, 270, None);
        assert_eq!(default_proc.pii_level, PiiFilterLevel::Standard);
    }

    // #6132: the high-importance OCR branch must reuse the long-lived cached
    // extractor (the same `Arc<OcrExtractor>`) rather than constructing a fresh
    // one per frame, which would defeat the cached `LepTess` reuse. Cloning the
    // shared `Arc` is what the branch does; verify it yields the same instance.
    #[cfg(feature = "ocr")]
    #[test]
    fn ocr_extractor_is_shared_and_reused() {
        let proc = EdgeFrameProcessor::new(480, 270, None);
        let shared = proc
            .ocr_extractor
            .as_ref()
            .expect("processor builds a default OCR extractor");
        // The per-frame branch clones this Arc; cloning must alias the same
        // cached extractor instance, not allocate a new one.
        let cloned = proc
            .ocr_extractor
            .clone()
            .expect("clone keeps the extractor");
        assert!(
            Arc::ptr_eq(shared, &cloned),
            "cloned extractor must point at the same cached instance"
        );
        assert_eq!(
            Arc::strong_count(shared),
            2,
            "exactly the original field reference and our clone should exist"
        );
    }

    // #7477: shipped (non-leptess) builds run with `--features grpc`, which does
    // not enable `ocr`. Before the fix the `#[cfg(not(feature = "ocr"))]` branch
    // hardcoded empty OCR, so `capture_and_process` returned `ocr_regions: []`,
    // `ocr_text: None`, `raw_rgba: None` on every frame — starving GUI element
    // correlation and the ML crop classifier. These tests exercise the wired
    // native OCR provider through the real high-importance pipeline branch.
    #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
    mod native_ocr_pipeline {
        use super::*;
        use maekon_core::ports::ocr_provider::{FakeOcrProvider, OcrResult};

        fn text_frame() -> DynamicImage {
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                200,
                80,
                image::Rgba([255, 255, 255, 255]),
            ))
        }

        fn high_importance_request() -> CaptureRequest {
            CaptureRequest {
                trigger_type: "ErrorDetected".to_string(),
                importance: 0.9, // high → the full-frame + OCR branch
                app_name: "Terminal".to_string(),
                window_title: "session".to_string(),
                monitor_id: None,
                app_bundle_id: None,
                window_bounds: None,
                screen_scale_factor: None,
                ocr_processing_permitted: true,
            }
        }

        // Fails before the fix: with no OCR source wired into the processor, the
        // non-leptess pipeline returns unconditionally-empty OCR. Passes after:
        // the wired provider feeds ocr_regions + ocr_text + raw_rgba, with PII
        // masked at the source exactly like the leptess path.
        #[tokio::test]
        async fn shipped_pipeline_yields_masked_ocr_from_native_provider() {
            let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
                text: "contact admin@company.com".to_string(),
                x: 10,
                y: 20,
                width: 180,
                height: 24,
                confidence: 0.95,
            }]));
            let processor =
                EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Standard)
                    .with_ocr_provider(Some(provider));

            let frame = processor
                .process_captured_frame(text_frame(), &high_importance_request(), None)
                .await
                .expect("processing a high-importance frame must succeed");

            // The bug was empty regions in every shipped build; now populated.
            assert_eq!(
                frame.ocr_regions.len(),
                1,
                "the wired native OCR provider must feed ocr_regions in a non-leptess build"
            );

            // PII is masked at the source (not weakened relative to leptess).
            let region_text = &frame.ocr_regions[0].text;
            assert!(
                region_text.contains("[EMAIL]"),
                "region text must be PII-masked, got {region_text:?}"
            );
            assert!(!region_text.contains("admin@company.com"));

            // The full-frame payload carries a non-empty, masked OCR text hint.
            match frame
                .image_payload
                .expect("importance >= 0.8 must yield an image payload")
            {
                ImagePayload::Full { ocr_text, .. } => {
                    let text = ocr_text.expect("native OCR must produce a page text hint");
                    assert!(
                        text.contains("[EMAIL]"),
                        "hint must be masked, got {text:?}"
                    );
                    assert!(!text.contains("admin@company.com"));
                }
                other => panic!("expected ImagePayload::Full, got {other:?}"),
            }

            // raw_rgba is populated for ML crop data, decoupled from region count.
            let rgba = frame
                .raw_rgba
                .expect("raw_rgba must be populated when OCR is active");
            assert_eq!(rgba.len(), 200 * 80 * 4);
        }

        // Without a provider wired (e.g. Linux, where create_native_ocr() is
        // None), the pipeline stays empty — proving the provider wiring is the
        // load-bearing change, not an unrelated code path.
        #[tokio::test]
        async fn shipped_pipeline_without_provider_is_empty() {
            let processor =
                EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Standard)
                    .with_ocr_provider(None);

            let frame = processor
                .process_captured_frame(text_frame(), &high_importance_request(), None)
                .await
                .expect("processing must succeed even without an OCR provider");

            assert!(
                frame.ocr_regions.is_empty(),
                "no provider wired → no OCR regions"
            );
            assert!(
                frame.raw_rgba.is_none(),
                "no OCR attempt → no raw_rgba retained"
            );
        }
    }
}
