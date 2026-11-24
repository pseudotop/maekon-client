use async_trait::async_trait;
use chrono::Utc;
use image::DynamicImage;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::frame::{FrameMetadata, ImagePayload, ProcessedFrame};
use maekon_core::ports::vision::{
    capture_denied, CaptureAccess, CaptureAuthority, CapturePermit, CaptureRequest, FrameProcessor,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

use crate::capture::{check_processing_permit, CaptureSource, ScreenCapture};
use crate::delta;
use crate::encoder::{self, WebPQuality};
use crate::privacy;
use crate::thumbnail;

pub struct EdgeFrameProcessor {
    capture: ScreenCapture,
    authorized_capture: Arc<dyn CaptureSource>,
    capture_authority: Option<Arc<dyn CaptureAuthority>>,
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
            authorized_capture: Arc::new(ScreenCapture::new()),
            capture_authority: None,
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

    /// Set the source used by authorized capture; callers supply its original permit.
    pub fn with_capture_source(mut self, source: Arc<dyn CaptureSource>) -> Self {
        self.authorized_capture = source;
        self
    }

    /// Supply the authority used by `begin_capture` before acquisition is queued.
    pub fn with_capture_authority(mut self, authority: Arc<dyn CaptureAuthority>) -> Self {
        self.capture_authority = Some(authority);
        self
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

    /// Reconfigure the OCR source(s) for the given recognition `languages`
    /// (BCP-47 tags, e.g. `ko-KR`, `en-US`) so the frame processor recognizes
    /// non-English screen text (#8054). The leptess path is rebuilt for the
    /// mapped Tesseract language string (reusing the existing tessdata dir); the
    /// native path is rebuilt via `create_native_ocr_with_languages`. An empty
    /// list restores each engine's built-in default.
    #[allow(unused_variables, unused_mut)]
    pub fn with_ocr_languages(mut self, languages: Vec<String>) -> Self {
        #[cfg(feature = "ocr")]
        {
            let tessdata = self
                .ocr_extractor
                .as_ref()
                .and_then(|extractor| extractor.tessdata_path().cloned());
            let lang = if languages.is_empty() {
                crate::ocr::DEFAULT_TESSERACT_LANG.to_string()
            } else {
                crate::ocr::tesseract_lang_from_bcp47(&languages)
            };
            self.ocr_extractor = Some(Arc::new(crate::ocr::OcrExtractor::new_with_lang(
                tessdata, &lang,
            )));
        }
        #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
        {
            self.ocr_provider = crate::native_ocr::create_native_ocr_with_languages(&languages);
        }
        self
    }
}

#[async_trait]
impl FrameProcessor for EdgeFrameProcessor {
    fn begin_capture(
        &self,
        expected: Option<&maekon_core::models::context::WindowInfo>,
    ) -> Result<CapturePermit, CoreError> {
        self.capture_authority
            .as_ref()
            .ok_or_else(|| capture_denied("capture authority unavailable"))?
            .authorize(expected)
    }

    async fn capture_authorized(
        &self,
        request: &CaptureRequest,
        permit: &CapturePermit,
    ) -> Result<ProcessedFrame, CoreError> {
        permit.check(CaptureAccess::Retain)?;
        if permit.config_snapshot().is_none() {
            return Err(capture_denied("capture policy snapshot unavailable"));
        }
        let (frame, monitor) = crate::capture::acquire_frame(
            self.authorized_capture.clone(),
            request.window_bounds,
            permit.clone(),
        )
        .await?;
        self.process_frame_inner(frame, request, monitor, Some(permit.clone()))
            .await
    }

    async fn thumbnail_authorized(
        &self,
        window_bounds: Option<&maekon_core::models::context::WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<Vec<u8>, CoreError> {
        crate::capture::acquire_thumbnail(
            self.authorized_capture.clone(),
            window_bounds.copied(),
            self.thumbnail_width,
            self.thumbnail_height,
            permit.clone(),
        )
        .await
    }

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

    async fn capture_thumbnail(
        &self,
        window_bounds: Option<&maekon_core::models::context::WindowBounds>,
    ) -> Result<Vec<u8>, CoreError> {
        let capture = self.capture.clone();
        let tw = self.thumbnail_width;
        let th = self.thumbnail_height;
        // #8054 P2-3: target the monitor containing the active window (falls
        // back to primary when bounds are absent or resolve to no monitor) so
        // dashcam ring/post-event frames follow the user across displays.
        let window_bounds = window_bounds.copied();
        tokio::task::spawn_blocking(move || {
            let frame = capture.capture_for_window(window_bounds.as_ref())?;
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
        self.process_frame_inner(captured_frame, capture_request, selected_monitor_id, None)
            .await
    }

    async fn process_frame_inner(
        &self,
        captured_frame: DynamicImage,
        capture_request: &CaptureRequest,
        selected_monitor_id: Option<usize>,
        permit: Option<CapturePermit>,
    ) -> Result<ProcessedFrame, CoreError> {
        check_processing_permit(&permit, CaptureAccess::Retain)?;
        let config = permit
            .as_ref()
            .map(|p| {
                p.config_snapshot()
                    .ok_or_else(|| capture_denied("capture policy snapshot unavailable"))
            })
            .transpose()?;
        let pii_level = config
            .as_ref()
            .map_or(self.pii_level, |c| c.privacy.pii_filter_level);
        let importance = capture_request.importance;
        let current_frame = Arc::new(captured_frame);
        let (w, h) = (current_frame.width(), current_frame.height());

        let metadata = build_frame_metadata(CaptureMetadataInput {
            trigger_type: &capture_request.trigger_type,
            app_name: &capture_request.app_name,
            window_title: &capture_request.window_title,
            resolution: (w, h),
            importance,
            monitor_id: if permit.is_some() {
                selected_monitor_id
            } else {
                capture_request.monitor_id.or(selected_monitor_id)
            },
            app_bundle_id: capture_request.app_bundle_id.as_deref(),
            pii_level,
        });

        let mut ocr_regions = Vec::new();
        let mut raw_rgba: Option<Vec<u8>> = None;
        let mut ocr_attempted = false;

        let image_payload = if importance >= 0.8 {
            debug!("frame (in progress {:.1})", importance);
            let frame_ref = Arc::clone(&current_frame);

            // Leptess (Tesseract) extractor — only compiled with `feature = "ocr"`.
            // Guarded input must die with this worker, including denied results.
            // Legacy captures keep their long-lived engine reuse (#6132).
            #[cfg(feature = "ocr")]
            let ocr_extractor = if capture_request.ocr_processing_permitted {
                self.ocr_extractor.as_ref().map(|shared| {
                    if permit.is_some() {
                        Arc::new(crate::ocr::OcrExtractor::new_with_lang(
                            shared.tessdata_path().cloned(),
                            shared.lang(),
                        ))
                    } else {
                        Arc::clone(shared)
                    }
                })
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
            ocr_attempted = ocr_active;

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
            let blocking_permit = permit.clone();

            // Blocking stage: CPU-heavy WebP encode + (leptess OCR) + raw-pixel
            // prep, all off the async reactor. A single WebP encode is reused for
            // storage (base64) and, under the native path, as the OCR input (raw
            // bytes) — avoiding a second full-frame encode.
            #[allow(unused_variables)]
            let (
                encoded,
                ocr_input,
                blocking_text,
                blocking_regions,
                raw_rgba_val,
                blocking_raw_regions,
            ) = tokio::task::spawn_blocking(move || {
                check_processing_permit(&blocking_permit, CaptureAccess::Retain)?;
                if ocr_active {
                    check_processing_permit(&blocking_permit, CaptureAccess::Ocr)?;
                }
                let webp_bytes = encoder::encode_webp(&frame_ref, WebPQuality::High)?;
                let enc = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&webp_bytes)
                };

                #[cfg(feature = "ocr")]
                let (text_val, raw_regions) = match ocr_extractor.as_ref() {
                    Some(extractor) => {
                        check_processing_permit(&blocking_permit, CaptureAccess::Ocr)?;
                        let text = match extractor.extract(&frame_ref) {
                            Ok(t) if !t.is_empty() => {
                                Some(crate::privacy::sanitize_title_with_level(&t, pii_level))
                            }
                            _ => None,
                        };
                        check_processing_permit(&blocking_permit, CaptureAccess::Ocr)?;
                        let regions = match extractor.extract_regions(&frame_ref) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!("OCR region extraction failure: {e}");
                                Vec::new()
                            }
                        };
                        check_processing_permit(&blocking_permit, CaptureAccess::Ocr)?;
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
                let scaled =
                    crate::ocr_geometry::scale_ocr_regions_to_logical(&raw_regions, scale_factor);
                let sanitized = sanitize_ocr_regions(scaled, pii_level);

                // Reuse the WebP bytes as the native-OCR input only when a
                // native provider is wired (otherwise drop them so the buffer
                // is not retained).
                #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
                let ocr_input: Option<Vec<u8>> = if ocr_active { Some(webp_bytes) } else { None };
                #[cfg(any(feature = "ocr", not(feature = "native-vision")))]
                let ocr_input: Option<Vec<u8>> = None;

                // ML crop pixels: populated whenever an OCR attempt is active,
                // decoupled from whether that attempt produced regions (#7477).
                let raw_rgba_opt = if ocr_active {
                    Some(frame_ref.to_rgba8().into_vec())
                } else {
                    None
                };

                check_processing_permit(&blocking_permit, CaptureAccess::Retain)?;
                Ok::<_, CoreError>((
                    enc,
                    ocr_input,
                    text_val,
                    sanitized,
                    raw_rgba_opt,
                    // #8042: raw, pre-logical-scale OCR regions (physical
                    // source pixels, unsanitized text) so the caller can
                    // detect + mask PII pixels aligned with the stored
                    // physical-resolution frame. Empty under the native path
                    // (its OCR runs in the async stage below).
                    raw_regions,
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
            let (ocr_text, ocr_regions_final, physical_regions) = match (
                ocr_provider.as_ref(),
                ocr_input.as_ref(),
            ) {
                (Some(provider), Some(webp)) => {
                    check_processing_permit(&permit, CaptureAccess::Ocr)?;
                    let result = provider.extract_elements(webp, "webp").await;
                    check_processing_permit(&permit, CaptureAccess::Ocr)?;
                    match result {
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
                            // `raw_regions` (physical, unsanitized) drives pixel
                            // redaction below; `sanitized` (logical, masked text)
                            // is what the frame exposes downstream.
                            (text, sanitized, raw_regions)
                        }
                        Err(e) => {
                            tracing::warn!(err.code = %e.code(), "native OCR extraction failed: {e}");
                            (None, Vec::new(), Vec::new())
                        }
                    }
                }
                _ => (None, Vec::new(), Vec::new()),
            };
            #[cfg(any(feature = "ocr", not(feature = "native-vision")))]
            let (ocr_text, ocr_regions_final, physical_regions) =
                (blocking_text, blocking_regions, blocking_raw_regions);

            ocr_regions = ocr_regions_final;
            raw_rgba = raw_rgba_val;

            // #8042: pixel-level PII redaction of the STORED frame. `encoded`
            // (base64 WebP) and `raw_rgba` currently hold the *original* pixels; if
            // OCR located PII, destructively mask those pixel regions and re-encode
            // before the frame is persisted. `physical_regions` carry raw OCR text
            // in the frame's PHYSICAL source-pixel space (pre
            // `scale_ocr_regions_to_logical`), matching the physical-resolution
            // frame exactly — no HiDPI scale round-trip. Gated by the configured
            // PiiFilterLevel (Off = operator opt-out — consistent with the OCR-text
            // path); consuming OCR regions implicitly gates it on ocr_processing
            // consent (no regions ⇒ no masking). Detection + re-encode both run in
            // spawn_blocking (off the async reactor) and only when regions exist.
            let mut encoded = encoded;
            if pii_level != PiiFilterLevel::Off && !physical_regions.is_empty() {
                let frame_for_redact = Arc::clone(&current_frame);
                let redaction_permit = permit.clone();
                // Fail CLOSED: if PII was detected but the redacted frame cannot be
                // produced, propagate the error so the caller drops the frame rather
                // than persisting unmasked PII pixels.
                let redacted = tokio::task::spawn_blocking(move || {
                    check_processing_permit(&redaction_permit, CaptureAccess::Retain)?;
                    let result = crate::frame_pii::redact_frame_if_pii(
                        &frame_for_redact,
                        &physical_regions,
                        pii_level,
                        WebPQuality::High,
                    )?;
                    check_processing_permit(&redaction_permit, CaptureAccess::Retain)?;
                    Ok::<_, CoreError>(result)
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("pii redaction task panicked: {e}"),
                })??;
                if let Some((redacted_webp, redacted_rgba)) = redacted {
                    encoded = {
                        use base64::Engine as _;
                        base64::engine::general_purpose::STANDARD.encode(&redacted_webp)
                    };
                    // ML crop input must mirror the stored frame (no PII pixels).
                    raw_rgba = Some(redacted_rgba);
                }
            }

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
            // Legacy baselines have no permit. Guarded requests neither read nor
            // replace them; medium importance emits a fresh Full until a separately
            // admitted permit-bound delta cache exists.
            let prev_snapshot = if permit.is_some() {
                None
            } else {
                self.prev_frame
                    .lock()
                    .map_err(|e| CoreError::Internal {
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: format!("prev_frame lock poisoned: {e}"),
                    })?
                    .clone()
            };

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
                let encoding_permit = permit.clone();
                let encoded = tokio::task::spawn_blocking(move || {
                    check_processing_permit(&encoding_permit, CaptureAccess::Retain)?;
                    let encoded = encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)?;
                    check_processing_permit(&encoding_permit, CaptureAccess::Retain)?;
                    Ok::<_, CoreError>(encoded)
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
            let tw = config
                .as_ref()
                .map_or(self.thumbnail_width, |c| c.vision.thumbnail_width);
            let th = config
                .as_ref()
                .map_or(self.thumbnail_height, |c| c.vision.thumbnail_height);
            let frame_ref = Arc::clone(&current_frame);
            let encoding_permit = permit.clone();
            let encoded = tokio::task::spawn_blocking(move || {
                check_processing_permit(&encoding_permit, CaptureAccess::Retain)?;
                let thumb = if encoding_permit.is_some() {
                    crate::capture::resize_uncached(&frame_ref, tw, th)?
                } else {
                    thumbnail::resize_to_fit(&frame_ref, tw, th)?
                };
                let (width, height) = (thumb.width(), thumb.height());
                let data = encoder::encode_webp_base64(&thumb, WebPQuality::Low)?;
                check_processing_permit(&encoding_permit, CaptureAccess::Retain)?;
                Ok::<_, CoreError>((data, width, height))
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

        check_processing_permit(&permit, CaptureAccess::Retain)?;
        if ocr_attempted {
            check_processing_permit(&permit, CaptureAccess::Ocr)?;
        }
        // No unguarded pixel cache may retain an authorized request's image.
        if permit.is_none() {
            *self.prev_frame.lock().map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("prev_frame lock poisoned: {e}"),
            })? = Some(current_frame);
        }

        Ok(ProcessedFrame {
            metadata,
            image_payload,
            ocr_regions,
            raw_rgba,
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[cfg(feature = "ocr")]
mod ocr_lifetime_tests {
    use super::*;
    use maekon_core::config::AppConfig;
    use maekon_core::ports::vision::CaptureGuard;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RevokeDuringOcr {
        calls: AtomicUsize,
        limit: usize,
    }

    impl CaptureGuard for RevokeDuringOcr {
        fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
            if matches!(access, CaptureAccess::Ocr)
                && self.calls.fetch_add(1, Ordering::SeqCst) + 1 >= self.limit
            {
                return Err(capture_denied("synthetic OCR grant revoked"));
            }
            Ok(())
        }

        fn config_snapshot(&self) -> Option<Arc<AppConfig>> {
            Some(Arc::new(AppConfig::default_config()))
        }
    }

    #[tokio::test]
    async fn guarded_tesseract_never_overwrites_legacy_input_after_delivery_or_revocation() {
        for limit in [1, 3, 4, usize::MAX] {
            let mut processor = EdgeFrameProcessor::new(8, 4, None);
            let shared = Arc::new(crate::ocr::OcrExtractor::new_with_lang(None, "eng"));
            let image = |w, h| {
                DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                    w,
                    h,
                    image::Rgba([255, 255, 255, 255]),
                ))
            };
            assert_eq!(shared.extract(&image(64, 32)).unwrap(), "");
            assert_eq!(shared.cached_image_dimensions(), Some((64, 32)));
            processor.ocr_extractor = Some(shared.clone());
            let permit = CapturePermit::new(Arc::new(RevokeDuringOcr {
                calls: AtomicUsize::new(0),
                limit,
            }));
            let request = CaptureRequest {
                importance: 0.9,
                ocr_processing_permitted: true,
                trigger_type: "synthetic".into(),
                app_name: "synthetic".into(),
                window_title: "synthetic".into(),
                monitor_id: None,
                app_bundle_id: None,
                window_bounds: None,
                screen_scale_factor: None,
            };
            let result = processor
                .process_frame_inner(image(16, 8), &request, None, Some(permit))
                .await;
            if limit == usize::MAX {
                assert_eq!(result.unwrap().raw_rgba.unwrap().len(), 16 * 8 * 4);
            } else {
                assert!(matches!(result.unwrap_err(), CoreError::PermissionDenied {
                    code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
                    message,
                } if message == "capture denied: synthetic OCR grant revoked"));
            }
            assert_eq!(
                shared.cached_image_dimensions(),
                Some((64, 32)),
                "OCR check {limit}"
            );
            assert!(processor.prev_frame.lock().unwrap().is_none());
        }
    }
}
