use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::models::intent::{ElementBounds, FinderSource, UiElement};
use tracing::debug;
use zeroize::{Zeroize, Zeroizing};

use crate::error::VisionError;
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::ui_scene::{
    NormalizedBounds, UiScene, UiSceneElement, UI_SCENE_SCHEMA_VERSION,
};
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};
use maekon_core::ports::rectangle_detector::{DetectedRectangle, RectangleDetector};

/// The image a finder answers from, and the OCR results already read from it.
///
/// `screen_seq` moves only when the image bytes or format change, so lookups
/// on an unchanged screen reuse `results` instead of running OCR again. A
/// `WaitForText` poll asks every `retry_interval_ms`, while the stored frame
/// changes only when the monitor saves a new one.
#[derive(Default)]
struct HeldScreen {
    image: Option<(Vec<u8>, String)>,
    screen_seq: u64,
    results: Option<Zeroizing<HeldResults>>,
}

impl HeldScreen {
    /// Hold `image`. A different image, or none, moves `screen_seq` and lets
    /// go of the results read from the previous one.
    fn hold(&mut self, image: Option<(Vec<u8>, String)>) {
        if self.image == image {
            return;
        }
        self.image = image;
        self.screen_seq = self.screen_seq.wrapping_add(1);
        self.results = None;
    }
}

/// OCR results kept for one screen. They outlive the lookup that read them,
/// so `Zeroizing` overwrites their text whenever they are let go: on a new
/// image, on `clear_image`, and when the finder is dropped.
struct HeldResults(Vec<OcrResult>);

impl Zeroize for HeldResults {
    fn zeroize(&mut self) {
        for result in &mut self.0 {
            result.text.zeroize();
        }
    }
}

/// What a lookup copies out of the held screen, so that no lock is held
/// across an OCR call.
struct HeldSnapshot {
    image_data: Vec<u8>,
    image_format: String,
    screen_seq: u64,
    kept: Option<Vec<OcrResult>>,
}

pub struct OcrElementFinder {
    ocr_provider: Arc<dyn OcrProvider>,
    rectangle_detector: Option<Arc<dyn RectangleDetector>>,
    held: tokio::sync::RwLock<HeldScreen>,
    /// PII level applied to scene-element `text_masked` (review4 V10). Was a
    /// hardcoded Standard, which silently downgraded a Strict-configured user
    /// (leaving API key / IP / passport in OCR'd scene text on the localhost REST
    /// scene surface + the overlay WebView).
    pii_level: PiiFilterLevel,
}

impl OcrElementFinder {
    pub fn new(ocr_provider: Arc<dyn OcrProvider>) -> Self {
        Self {
            ocr_provider,
            rectangle_detector: None,
            held: tokio::sync::RwLock::new(HeldScreen::default()),
            pii_level: PiiFilterLevel::Standard,
        }
    }

    /// Set the configured PII level used to mask scene-element text (review4 V10).
    pub fn with_pii_level(mut self, level: PiiFilterLevel) -> Self {
        self.pii_level = level;
        self
    }

    pub fn with_rectangle_detector(mut self, detector: Arc<dyn RectangleDetector>) -> Self {
        self.rectangle_detector = Some(detector);
        self
    }

    /// Hold the image later lookups answer from. Holding the same bytes and
    /// format again keeps the OCR results already read from them.
    pub async fn set_image(&self, image_data: Vec<u8>, format: String) {
        self.held.write().await.hold(Some((image_data, format)));
    }

    /// Drop the held image and wipe the OCR results read from it, so no lookup
    /// answers from a screen that could not be read again.
    pub async fn clear_image(&self) {
        self.held.write().await.hold(None);
    }

    async fn snapshot(&self) -> Option<HeldSnapshot> {
        let held = self.held.read().await;
        let (image_data, image_format) = held.image.clone()?;
        Some(HeldSnapshot {
            image_data,
            image_format,
            screen_seq: held.screen_seq,
            kept: held.results.as_ref().map(|results| results.0.clone()),
        })
    }

    /// Run OCR on a held image and keep the results for its screen number,
    /// unless the image changed meanwhile or the provider is external.
    async fn read_screen(
        &self,
        image_data: &[u8],
        image_format: &str,
        screen_seq: u64,
    ) -> Result<Vec<OcrResult>, CoreError> {
        let results = self
            .ocr_provider
            .extract_elements(image_data, image_format)
            .await?;
        if self.keeps_results() {
            let mut held = self.held.write().await;
            if held.screen_seq == screen_seq && held.results.is_none() {
                held.results = Some(Zeroizing::new(HeldResults(results.clone())));
            }
        }
        Ok(results)
    }

    /// External providers are called every time: each call passes their
    /// consent, policy and redaction gate, and kept results would outlive a
    /// revocation.
    fn keeps_results(&self) -> bool {
        !self.ocr_provider.is_external()
    }

    // A subscription may answer after a replacement frame arrives. A live
    // lookup must not hand those coordinates to the overlay or input driver.
    async fn check_external_snapshot(&self, expected: u64) -> Result<(), CoreError> {
        if self.ocr_provider.is_external() && self.held.read().await.screen_seq != expected {
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "Screen changed while waiting for the text provider.".to_string(),
            });
        }
        Ok(())
    }

    fn ocr_to_elements(
        results: &[OcrResult],
        text_query: Option<&str>,
        _role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Vec<UiElement> {
        results
            .iter()
            .filter(|r| {
                if let Some(bounds) = region {
                    // i64-widen extents to avoid i32 overflow on large OCR coords
                    // (review4 V11).
                    let in_region = i64::from(r.x) >= i64::from(bounds.x)
                        && i64::from(r.y) >= i64::from(bounds.y)
                        && (i64::from(r.x) + i64::from(r.width))
                            <= (i64::from(bounds.x) + i64::from(bounds.width))
                        && (i64::from(r.y) + i64::from(r.height))
                            <= (i64::from(bounds.y) + i64::from(bounds.height));
                    if !in_region {
                        return false;
                    }
                }
                true
            })
            .filter(|r| {
                if let Some(query) = text_query {
                    let query_lower = query.to_lowercase();
                    let text_lower = r.text.to_lowercase();
                    text_lower.contains(&query_lower)
                } else {
                    true
                }
            })
            .map(|r| {
                let text_confidence = if let Some(query) = text_query {
                    text_similarity(&r.text, query)
                } else {
                    1.0
                };
                let combined_confidence = r.confidence * text_confidence;

                UiElement {
                    text: r.text.clone(),
                    bounds: ElementBounds {
                        x: r.x,
                        y: r.y,
                        width: r.width,
                        height: r.height,
                    },
                    role: None,
                    confidence: combined_confidence,
                    source: FinderSource::Ocr,
                }
            })
            .collect()
    }

    pub async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, VisionError> {
        let HeldSnapshot {
            image_data,
            image_format,
            screen_seq,
            kept,
        } = self.snapshot().await.ok_or_else(|| {
            VisionError::Internal("OCR finder: captured image is missing".to_string())
        })?;
        let size = image_size(&image_data)?;
        let image = Arc::new(image_data);
        let text = async {
            match kept {
                Some(results) => Ok(results),
                None => self.read_screen(&image, &image_format, screen_seq).await,
            }
        };
        let scene = self
            .scene_with_text(image.clone(), size, text, app_name, screen_id)
            .await?;
        self.check_external_snapshot(screen_seq).await?;
        Ok(scene)
    }

    async fn analyze_scene_from_image_data(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, VisionError> {
        let size = image_size(&image_data)?;
        let image = Arc::new(image_data);
        let text = self.ocr_provider.extract_elements(&image, &image_format);
        self.scene_with_text(image.clone(), size, text, app_name, screen_id)
            .await
    }

    // Start geometry on the same immutable frame as the text request. The combined
    // scene preserves the configured provider's authorization and error semantics.
    async fn scene_with_text(
        &self,
        image: Arc<Vec<u8>>,
        size: (u32, u32),
        text: impl std::future::Future<Output = Result<Vec<OcrResult>, CoreError>>,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, VisionError> {
        let geometry = self.detect_regions(image, size);
        let (regions, text) =
            tokio::try_join!(geometry, async { text.await.map_err(VisionError::from) })?;
        // A rectangle backend failure previously left a successful OCR scene usable.
        let regions = regions.unwrap_or_else(|err| {
            debug!(code = err.code(), "optional region detector failed");
            Vec::new()
        });
        Ok(self.scene_from_parts(size, &regions, &text, app_name, screen_id))
    }

    /// Analyze visible geometry without calling the configured text provider.
    /// The returned scene has no OCR labels and never starts a provider request.
    pub async fn analyze_geometry_from_image(
        &self,
        image_data: Vec<u8>,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, VisionError> {
        if self.rectangle_detector.is_none() {
            return Err(VisionError::Internal(
                "No region detector configured".to_string(),
            ));
        }
        let size = image::ImageReader::new(std::io::Cursor::new(&image_data))
            .with_guessed_format()
            .map_err(|err| VisionError::Internal(err.to_string()))?
            .into_dimensions()
            .map_err(|err| VisionError::Internal(err.to_string()))?;
        let regions = self.detect_regions(Arc::new(image_data), size).await??;
        Ok(self.scene_from_parts(size, &regions, &[], app_name, screen_id))
    }

    async fn detect_regions(
        &self,
        image: Arc<Vec<u8>>,
        (width, height): (u32, u32),
    ) -> Result<Result<Vec<DetectedRectangle>, CoreError>, VisionError> {
        let Some(detector) = self.rectangle_detector.clone() else {
            return Ok(Ok(Vec::new()));
        };
        tokio::task::spawn_blocking(move || {
            detector.detect_rectangles(&image, width, height, 0.02, 100)
        })
        .await
        .map_err(|err| VisionError::Internal(format!("region detector task failed: {err}")))
    }

    fn scene_from_parts(
        &self,
        (screen_width, screen_height): (u32, u32),
        regions: &[DetectedRectangle],
        text: &[OcrResult],
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> UiScene {
        let mut elements = Self::ocr_to_scene_elements(
            text,
            screen_width,
            screen_height,
            app_name,
            screen_id,
            self.pii_level,
        );
        let geometry = merge_rectangles(regions, &elements, screen_width, screen_height);
        attach_text_to_regions(&mut elements, &geometry);
        elements.extend(geometry);

        UiScene {
            schema_version: UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: maekon_core::generate_id("scene"),
            app_name: app_name.map(str::to_string),
            screen_id: screen_id.map(str::to_string),
            captured_at: Utc::now(),
            screen_width,
            screen_height,
            elements,
        }
    }

    fn ocr_to_scene_elements(
        results: &[OcrResult],
        screen_width: u32,
        screen_height: u32,
        app_name: Option<&str>,
        screen_id: Option<&str>,
        pii_level: PiiFilterLevel,
    ) -> Vec<UiSceneElement> {
        let width = screen_width.max(1) as f32;
        let height = screen_height.max(1) as f32;
        let app_label = app_name.unwrap_or("unknown");
        let screen_label = screen_id.unwrap_or("main");

        results
            .iter()
            .enumerate()
            .map(|(index, r)| {
                let text_trimmed = r.text.trim();
                let label = if text_trimmed.is_empty() {
                    "text".to_string()
                } else {
                    text_trimmed.to_string()
                };
                let text_masked = crate::privacy::sanitize_title_with_level(&label, pii_level);

                let bbox_abs = ElementBounds {
                    x: r.x.max(0),
                    y: r.y.max(0),
                    width: r.width.max(1),
                    height: r.height.max(1),
                };
                let bbox_norm = NormalizedBounds::new(
                    bbox_abs.x as f32 / width,
                    bbox_abs.y as f32 / height,
                    bbox_abs.width as f32 / width,
                    bbox_abs.height as f32 / height,
                );

                UiSceneElement {
                    element_id: format!("el_{app_label}_{screen_label}_{index}"),
                    bbox_abs,
                    bbox_norm,
                    label,
                    role: None,
                    intent: None,
                    state: None,
                    confidence: r.confidence,
                    text_masked: Some(text_masked),
                    parent_id: None,
                }
            })
            .collect()
    }
}

#[async_trait]
impl ElementFinder for OcrElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        let HeldSnapshot {
            image_data,
            image_format,
            screen_seq,
            kept,
        } = self.snapshot().await.ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "OCR finder: captured image is missing".to_string(),
        })?;

        debug!(
            provider = self.ocr_provider.provider_name(),
            text_len = ?text.map(str::len),
            role = ?role,
            reused_ocr = kept.is_some(),
            "OCR element search started"
        );

        let ocr_results = match kept {
            Some(results) => results,
            None => {
                self.read_screen(&image_data, &image_format, screen_seq)
                    .await?
            }
        };

        let mut elements = Self::ocr_to_elements(&ocr_results, text, role, region);

        elements.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        self.check_external_snapshot(screen_seq).await?;
        debug!(count = elements.len(), "OCR element lookup completed");

        Ok(elements)
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        OcrElementFinder::analyze_scene(self, app_name, screen_id)
            .await
            .map_err(Into::into)
    }

    async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        self.analyze_scene_from_image_data(image_data, image_format, app_name, screen_id)
            .await
            .map_err(Into::into)
    }

    fn name(&self) -> &str {
        "ocr"
    }
}

pub struct ChainedElementFinder {
    finders: Vec<Box<dyn ElementFinder>>,
}

impl ChainedElementFinder {
    pub fn new(finders: Vec<Box<dyn ElementFinder>>) -> Self {
        Self { finders }
    }
}

#[async_trait]
impl ElementFinder for ChainedElementFinder {
    async fn find_element(
        &self,
        text: Option<&str>,
        role: Option<&str>,
        region: Option<&ElementBounds>,
    ) -> Result<Vec<UiElement>, CoreError> {
        for finder in &self.finders {
            debug!(finder = finder.name(), "lookup attempt");
            match finder.find_element(text, role, region).await {
                Ok(elements) if !elements.is_empty() => {
                    debug!(
                        finder = finder.name(),
                        count = elements.len(),
                        "Chain finder succeeded"
                    );
                    return Ok(elements);
                }
                Ok(_) => {
                    debug!(finder = finder.name(), "none, next lookup attempt");
                }
                Err(e) => {
                    debug!(finder = finder.name(), error = %e, "lookup failure, next lookup attempt");
                }
            }
        }
        Err(CoreError::ElementNotFound {
            code: maekon_core::error_codes::UiCode::ElementMissing,
            name: format!(
                "No element found in all finders (text={:?}, role={:?})",
                text, role
            ),
        })
    }

    async fn analyze_scene(
        &self,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        let mut last_err: Option<CoreError> = None;

        for finder in &self.finders {
            debug!(finder = finder.name(), "scene min attempt");
            match finder.analyze_scene(app_name, screen_id).await {
                Ok(scene) => return Ok(scene),
                Err(err) => {
                    debug!(
                        finder = finder.name(),
                        error = %err,
                        "Scene analysis failed, trying next finder"
                    );
                    last_err = Some(err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| CoreError::ElementNotFound {
            code: maekon_core::error_codes::UiCode::ElementMissing,
            name: "No finder supports scene analysis".to_string(),
        }))
    }

    async fn analyze_scene_from_image(
        &self,
        image_data: Vec<u8>,
        image_format: String,
        app_name: Option<&str>,
        screen_id: Option<&str>,
    ) -> Result<UiScene, CoreError> {
        let mut last_err: Option<CoreError> = None;

        for finder in &self.finders {
            debug!(finder = finder.name(), "image scene min attempt");
            match finder
                .analyze_scene_from_image(
                    image_data.clone(),
                    image_format.clone(),
                    app_name,
                    screen_id,
                )
                .await
            {
                Ok(scene) => return Ok(scene),
                Err(err) => {
                    debug!(
                        finder = finder.name(),
                        error = %err,
                        "image scene analysis failed, trying next finder"
                    );
                    last_err = Some(err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| CoreError::ElementNotFound {
            code: maekon_core::error_codes::UiCode::ElementMissing,
            name: "No finder supports image scene analysis".to_string(),
        }))
    }

    fn name(&self) -> &str {
        "chained"
    }
}

fn image_size(image_data: &[u8]) -> Result<(u32, u32), VisionError> {
    image::load_from_memory(image_data)
        .map(|img| (img.width().max(1), img.height().max(1)))
        .map_err(|e| VisionError::Ocr(format!("Failed to parse image size: {e}")))
}

fn merge_rectangles(
    rects: &[DetectedRectangle],
    ocr_elements: &[UiSceneElement],
    screen_width: u32,
    screen_height: u32,
) -> Vec<UiSceneElement> {
    let w = screen_width.max(1) as f32;
    let h = screen_height.max(1) as f32;
    let mut new_elements = Vec::new();

    for rect in rects {
        let duplicates_text = ocr_elements.iter().any(|el| {
            let iou = compute_iou(&rect.bounds, &el.bbox_abs);
            iou >= 0.80 || (iou >= 0.20 && !contains_box(&rect.bounds, &el.bbox_abs))
        });
        if !duplicates_text {
            new_elements.push(UiSceneElement {
                element_id: maekon_core::generate_id("rect"),
                bbox_abs: rect.bounds.clone(),
                bbox_norm: NormalizedBounds::new(
                    rect.bounds.x as f32 / w,
                    rect.bounds.y as f32 / h,
                    rect.bounds.width as f32 / w,
                    rect.bounds.height as f32 / h,
                ),
                label: String::new(),
                role: Some("region".to_string()),
                intent: None,
                state: None,
                confidence: rect.confidence,
                text_masked: None,
                parent_id: None,
            });
        }
    }

    new_elements
}

fn contains_box(outer: &ElementBounds, inner: &ElementBounds) -> bool {
    inner.width > 0
        && inner.height > 0
        && inner.x >= outer.x
        && inner.y >= outer.y
        && i64::from(inner.x) + i64::from(inner.width)
            <= i64::from(outer.x) + i64::from(outer.width)
        && i64::from(inner.y) + i64::from(inner.height)
            <= i64::from(outer.y) + i64::from(outer.height)
}

// Set only a parent reference. Raw text and its redacted copy stay on the OCR
// observation; region proposals themselves remain unlabeled and non-actionable.
fn attach_text_to_regions(text: &mut [UiSceneElement], regions: &[UiSceneElement]) {
    for word in text {
        let smallest = regions
            .iter()
            .filter(|region| contains_box(&region.bbox_abs, &word.bbox_abs))
            // A containing box is at least as wide and tall as its child.
            // The overlap check below handles crossing boxes.
            .min_by_key(|region| (region.bbox_abs.width, region.bbox_abs.height));
        if let Some(parent) = smallest {
            // There is no z-order in a bitmap. Two intersecting, non-nested
            // candidate owners are ambiguous even if one box is smaller.
            if regions.iter().all(|region| {
                compute_iou(&region.bbox_abs, &word.bbox_abs) == 0.0
                    || contains_box(&region.bbox_abs, &parent.bbox_abs)
            }) {
                word.parent_id = Some(parent.element_id.clone());
            }
        }
    }
}

fn compute_iou(a: &ElementBounds, b: &ElementBounds) -> f32 {
    // Widen to i64 before computing right/bottom extents (review4 V11): x is i32 and
    // width is u32, so `x + width as i32` overflows i32 on adversarially-large OCR
    // coordinates (panic in debug/tests, two's-complement wrap in release → garbage
    // IoU). Mirrors the i64-widening already used in the OCR parser's vertex path.
    let x1 = i64::from(a.x).max(i64::from(b.x));
    let y1 = i64::from(a.y).max(i64::from(b.y));
    let x2 = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
    let y2 = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));

    if x2 <= x1 || y2 <= y1 {
        return 0.0;
    }

    let intersection = (x2 - x1) as f32 * (y2 - y1) as f32;
    let area_a = a.width as f32 * a.height as f32;
    let area_b = b.width as f32 * b.height as f32;
    let union = area_a + area_b - intersection;

    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn text_similarity(text: &str, query: &str) -> f64 {
    let text_lower = text.to_lowercase();
    let query_lower = query.to_lowercase();

    if text_lower == query_lower {
        1.0
    } else if text_lower.contains(&query_lower) {
        query_lower.len() as f64 / text_lower.len() as f64
    } else {
        0.0
    }
}

#[cfg(test)]
#[path = "element_finder_tests.rs"]
mod tests;
