use std::sync::Arc;

use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use tracing::warn;

use crate::privacy::{is_sensitive_app, sanitize_title_with_level, should_exclude_by_policy};

#[derive(Debug, Clone)]
pub enum PrivacyDenied {
    NoConsent,
    UnredactedExternalOcrConsentRequired,
    SensitiveApp(String),
    ExcludedByPolicy,
    SanitizationFailed(String),
}

impl std::fmt::Display for PrivacyDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsent => write!(f, "OCR consent is required"),
            Self::UnredactedExternalOcrConsentRequired => {
                write!(f, "Unredacted external OCR consent is required")
            }
            Self::SensitiveApp(app) => write!(f, "Blocked sensitive app: {}", app),
            Self::ExcludedByPolicy => write!(f, "Excluded by policy"),
            Self::SanitizationFailed(reason) => write!(f, "Image sanitization failed: {reason}"),
        }
    }
}

#[derive(Debug)]
pub struct SanitizedImage {
    pub image_data: Vec<u8>,
    pub metadata_stripped: bool,
    pub redacted_regions: usize,
}

#[cfg(any(feature = "ocr", feature = "native-vision"))]
#[derive(Debug, Clone, Copy)]
struct SensitiveRegion {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Minimal (text, bounding box) pair used to feed sensitive-region detection.
///
/// Decoupled from `crate::ocr::OcrWordBox` (only compiled under
/// `feature = "ocr"`, leptess) and from
/// `maekon_core::ports::ocr_provider::OcrResult` (the wire type returned by
/// [`maekon_core::ports::ocr_provider::OcrProvider`]) so that
/// [`PrivacyGateway::detect_sensitive_regions`] and
/// [`PrivacyGateway::merge_sensitive_regions`] compile and run identically
/// for both the leptess word-box path and the native-OCR (VIS-Q2, #7602)
/// element path, without either path depending on the other's module.
#[cfg(any(feature = "ocr", feature = "native-vision"))]
#[derive(Debug, Clone)]
struct TextBox {
    text: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Converts a native `OcrProvider` element (`OcrResult`) into the shared
/// [`TextBox`] shape consumed by [`PrivacyGateway::detect_sensitive_regions`].
///
/// Mirrors `processor.rs`'s `ocr_result_to_region` conversion for the capture
/// pipeline (#7477); this is the privacy-gateway analogue for the VIS-Q2
/// native-OCR fallback (#7602).
#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
fn ocr_result_to_text_box(result: &maekon_core::ports::ocr_provider::OcrResult) -> TextBox {
    TextBox {
        text: result.text.clone(),
        x: result.x,
        y: result.y,
        w: result.width as i32,
        h: result.height as i32,
    }
}

/// Destructively redacts a rectangular region of an RGBA image by overwriting
/// every pixel inside it with a solid, fully-opaque black fill.
///
/// This replaces an earlier Gaussian-blur treatment (#7069). The redacted image
/// egresses to a third-party / external OCR endpoint, so the recipient is
/// exactly the adversary the redaction defends against. A Gaussian blur is a
/// recoverable transform — blurred SSNs / card numbers / emails / API keys can
/// be reconstructed via deconvolution or CNN deblurring — whereas a constant
/// opaque fill discards the underlying pixels entirely. The resulting region is
/// constant-valued (zero variance), so nothing recoverable leaves the device.
///
/// `x`/`y`/`w`/`h` are clamped to the image bounds. Returns the number of pixels
/// overwritten.
#[cfg_attr(not(any(feature = "ocr", feature = "native-vision")), allow(dead_code))]
fn redact_region_opaque(img: &mut image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> u32 {
    // Solid, fully-opaque black. Every redacted pixel becomes this exact value.
    const FILL: image::Rgba<u8> = image::Rgba([0, 0, 0, 255]);

    let (img_w, img_h) = img.dimensions();
    let mut filled = 0u32;
    for dy in 0..h {
        let py = y + dy;
        if py >= img_h {
            break;
        }
        for dx in 0..w {
            let px = x + dx;
            if px >= img_w {
                break;
            }
            img.put_pixel(px, py, FILL);
            filled += 1;
        }
    }
    filled
}

// PrivacyGateway

pub struct PrivacyGateway {
    consent_manager: Arc<dyn ConsentManagerPort>,
    pii_filter_level: PiiFilterLevel,
    external_data_policy: ExternalDataPolicy,
    privacy_config: PrivacyConfig,
}

impl PrivacyGateway {
    pub fn new(
        consent_manager: Arc<dyn ConsentManagerPort>,
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        privacy_config: PrivacyConfig,
    ) -> Self {
        Self {
            consent_manager,
            pii_filter_level,
            external_data_policy,
            privacy_config,
        }
    }

    pub async fn sanitize_image_for_external_policy(
        image_data: &[u8],
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        bypass_pii_filter_for_external_ocr: bool,
    ) -> Result<SanitizedImage, PrivacyDenied> {
        let filter_level = Self::resolve_filter_level(
            pii_filter_level,
            external_data_policy,
            bypass_pii_filter_for_external_ocr,
        );
        let (sanitized_data, redacted_regions) = if filter_level == PiiFilterLevel::Off {
            (image_data.to_vec(), 0)
        } else {
            Self::blur_pii_regions(image_data, filter_level).await?
        };

        Ok(SanitizedImage {
            image_data: sanitized_data,
            metadata_stripped: true,
            redacted_regions,
        })
    }

    pub async fn prepare_image_for_external(
        &self,
        image_data: &[u8],
        active_app: &str,
        window_title: &str,
    ) -> Result<SanitizedImage, PrivacyDenied> {
        self.prepare_image_for_external_with_override(image_data, active_app, window_title, false)
            .await
    }

    pub async fn prepare_image_for_external_with_override(
        &self,
        image_data: &[u8],
        active_app: &str,
        window_title: &str,
        bypass_pii_filter_for_external_ocr: bool,
    ) -> Result<SanitizedImage, PrivacyDenied> {
        if !self.consent_manager.effective_permissions().ocr_processing {
            return Err(PrivacyDenied::NoConsent);
        }
        if bypass_pii_filter_for_external_ocr
            && !self
                .consent_manager
                .effective_permissions()
                .unredacted_external_ocr
        {
            return Err(PrivacyDenied::UnredactedExternalOcrConsentRequired);
        }

        if is_sensitive_app(active_app) {
            return Err(PrivacyDenied::SensitiveApp(active_app.to_string()));
        }

        if should_exclude_by_policy(&self.privacy_config, active_app, window_title) {
            return Err(PrivacyDenied::ExcludedByPolicy);
        }

        let filter_level = Self::resolve_filter_level(
            self.pii_filter_level,
            self.external_data_policy,
            bypass_pii_filter_for_external_ocr,
        );
        // When the override forced PiiFilterLevel::Off, emit an additional audit
        // event with the app/window context that resolve_filter_level cannot see.
        // The window title is PII (document names, mail subjects, URLs), so it is
        // never interpolated raw — it is sanitized at Strict before logging so the
        // file log layer cannot persist raw content (#5591/#6006).
        if bypass_pii_filter_for_external_ocr {
            warn!(
                privacy.bypass = true,
                app = %active_app,
                window_title = %sanitize_title_with_level(window_title, PiiFilterLevel::Strict),
                "PII filtering DISABLED for external OCR (app context): \
                 raw screen content for app '{}' will be sent off-device.",
                active_app
            );
        }
        let (sanitized_data, redacted_regions) = if filter_level == PiiFilterLevel::Off {
            (image_data.to_vec(), 0)
        } else {
            Self::blur_pii_regions(image_data, filter_level).await?
        };

        Ok(SanitizedImage {
            image_data: sanitized_data,
            metadata_stripped: true,
            redacted_regions,
        })
    }

    pub fn prepare_text_for_external(
        &self,
        texts: &[String],
    ) -> Result<Vec<String>, PrivacyDenied> {
        self.prepare_text_for_external_with_surface(texts, "", "")
    }

    pub fn prepare_text_for_external_with_surface(
        &self,
        texts: &[String],
        active_app: &str,
        window_title: &str,
    ) -> Result<Vec<String>, PrivacyDenied> {
        if !self.consent_manager.effective_permissions().ocr_processing {
            return Err(PrivacyDenied::NoConsent);
        }

        if !active_app.is_empty() && is_sensitive_app(active_app) {
            return Err(PrivacyDenied::SensitiveApp(active_app.to_string()));
        }

        if should_exclude_by_policy(&self.privacy_config, active_app, window_title) {
            return Err(PrivacyDenied::ExcludedByPolicy);
        }

        let filter_level = self.effective_filter_level();
        Ok(texts
            .iter()
            .map(|t| sanitize_title_with_level(t, filter_level))
            .collect())
    }

    #[allow(clippy::unused_async)] // await used only with `ocr`/`native-vision` features
    async fn blur_pii_regions(
        image_data: &[u8],
        filter_level: PiiFilterLevel,
    ) -> Result<(Vec<u8>, usize), PrivacyDenied> {
        #[cfg(feature = "ocr")]
        {
            use crate::ocr::OcrExtractor;
            use tracing::debug;

            let img = match image::load_from_memory(image_data) {
                Ok(img) => img,
                Err(e) => {
                    warn!("PII: image decoding failure: {e}");
                    return Err(PrivacyDenied::SanitizationFailed(
                        "image decoding failed before external OCR".to_string(),
                    ));
                }
            };

            let extractor = OcrExtractor::new(None);
            let word_boxes = match extractor.extract_words_with_boxes(&img).await {
                Ok(boxes) => boxes,
                Err(e) => {
                    debug!("PII: OCR failure: {e}, blocking external image");
                    return Err(PrivacyDenied::SanitizationFailed(
                        "local OCR failed before external OCR".to_string(),
                    ));
                }
            };

            if word_boxes.is_empty() {
                return Err(PrivacyDenied::SanitizationFailed(
                    "local OCR found no verifiable text before external OCR".to_string(),
                ));
            }

            let text_boxes: Vec<TextBox> = word_boxes
                .iter()
                .map(|wb| TextBox {
                    text: wb.text.clone(),
                    x: wb.x,
                    y: wb.y,
                    w: wb.w,
                    h: wb.h,
                })
                .collect();
            let pii_regions = Self::detect_sensitive_regions(&text_boxes, filter_level);

            if pii_regions.is_empty() {
                return Ok((image_data.to_vec(), 0));
            }

            debug!(
                "PII blur: detected and merged {} region(s) from {} word box(es)",
                pii_regions.len(),
                word_boxes.len()
            );

            let mut result_img = img.to_rgba8();
            Self::apply_region_redaction(&mut result_img, &pii_regions);
            let encoded = Self::encode_redacted_png(result_img)?;

            Ok((encoded, pii_regions.len()))
        }

        // VIS-Q2 (#7602 audit ②): shipped macOS/Windows builds do not compile
        // leptess (`ocr` is not in `src-tauri`'s feature list at all), but DO
        // default-compile `native-vision`. Before this fix, this arm
        // unconditionally returned `SanitizationFailed` regardless of image
        // validity or platform — a configured external/BYOK OCR provider hard-
        // errored on EVERY filtered request, a dead capability on every
        // non-leptess build. Fall back to the platform-native OCR engine
        // (macOS Vision.framework / Windows Media.Ocr) for region detection,
        // mirroring the capture pipeline's own native-OCR fallback
        // (`processor.rs`, #7477). Still fails closed when no native engine
        // exists either (e.g. Linux — see `blur_pii_regions_native`).
        #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
        {
            return Self::blur_pii_regions_native(
                image_data,
                filter_level,
                crate::native_ocr::create_native_ocr(),
            )
            .await;
        }

        #[cfg(not(any(feature = "ocr", feature = "native-vision")))]
        {
            let _ = image_data;
            let _ = filter_level;
            Err(PrivacyDenied::SanitizationFailed(
                "local OCR feature is unavailable for external image sanitization".to_string(),
            ))
        }
    }

    /// VIS-Q2 fallback (#7602): region-detection + redaction driven by an
    /// injected `OcrProvider`.
    ///
    /// Factored out of `blur_pii_regions`'s native-vision arm purely so unit
    /// tests can inject a deterministic fake in place of the real platform
    /// Vision.framework / Media.Ocr engine — mirrors
    /// `EdgeFrameProcessor::with_ocr_provider` in `processor.rs` (#7477),
    /// which solves the identical "test a native-OS-API path without the OS
    /// API" problem for the capture pipeline.
    ///
    /// `ocr_provider = None` means no OCR source exists on this platform at
    /// all (e.g. Linux, where `native_ocr::create_native_ocr()` always
    /// returns `None`, and leptess is not compiled into shipped builds
    /// either) — this fails closed exactly like the pre-#7602 unconditional
    /// behaviour: no verifiable OCR means no external egress, ever.
    #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
    async fn blur_pii_regions_native(
        image_data: &[u8],
        filter_level: PiiFilterLevel,
        ocr_provider: Option<std::sync::Arc<dyn maekon_core::ports::ocr_provider::OcrProvider>>,
    ) -> Result<(Vec<u8>, usize), PrivacyDenied> {
        use tracing::debug;

        let Some(provider) = ocr_provider else {
            return Err(PrivacyDenied::SanitizationFailed(
                "no OCR engine (leptess or native) is available on this platform for external image sanitization"
                    .to_string(),
            ));
        };

        let img = match image::load_from_memory(image_data) {
            Ok(img) => img,
            Err(e) => {
                warn!("PII: image decoding failure: {e}");
                return Err(PrivacyDenied::SanitizationFailed(
                    "image decoding failed before external OCR".to_string(),
                ));
            }
        };

        // Re-encode to PNG for the OCR call: `image_data`'s original encoding
        // is not known to this function (callers pass arbitrary capture
        // bytes), and the Windows native provider branches its decode path on
        // the format string (WIC has no WebP codec — see
        // `native_ocr::windows`). PNG is lossless and universally decodable
        // by both native providers, so always re-encoding to it and always
        // passing `"png"` is simple and unconditionally correct.
        let mut ocr_input = Vec::new();
        if let Err(e) = img.write_to(
            &mut std::io::Cursor::new(&mut ocr_input),
            image::ImageFormat::Png,
        ) {
            warn!("PII: image re-encode for native OCR failed: {e}");
            return Err(PrivacyDenied::SanitizationFailed(
                "image re-encode failed before native OCR".to_string(),
            ));
        }

        let results = match provider.extract_elements(&ocr_input, "png").await {
            Ok(r) => r,
            Err(e) => {
                debug!("PII: native OCR failure: {e}, blocking external image");
                return Err(PrivacyDenied::SanitizationFailed(
                    "native OCR failed before external OCR".to_string(),
                ));
            }
        };

        if results.is_empty() {
            return Err(PrivacyDenied::SanitizationFailed(
                "native OCR found no verifiable text before external OCR".to_string(),
            ));
        }

        let text_boxes: Vec<TextBox> = results.iter().map(ocr_result_to_text_box).collect();
        let pii_regions = Self::detect_sensitive_regions(&text_boxes, filter_level);

        if pii_regions.is_empty() {
            return Ok((image_data.to_vec(), 0));
        }

        debug!(
            "PII blur (native OCR): detected and merged {} region(s) from {} element(s)",
            pii_regions.len(),
            results.len()
        );

        let mut result_img = img.to_rgba8();
        Self::apply_region_redaction(&mut result_img, &pii_regions);
        let encoded = Self::encode_redacted_png(result_img)?;

        Ok((encoded, pii_regions.len()))
    }

    /// Applies [`redact_region_opaque`] (with the shared 4px margin) to every
    /// detected sensitive region. Shared by the leptess path and the
    /// VIS-Q2 native-OCR fallback path (#7602) so the masking treatment is
    /// byte-for-byte identical regardless of which OCR source produced the
    /// regions.
    #[cfg(any(feature = "ocr", feature = "native-vision"))]
    fn apply_region_redaction(result_img: &mut image::RgbaImage, pii_regions: &[SensitiveRegion]) {
        let (img_w, img_h) = result_img.dimensions();

        for region in pii_regions {
            let margin = 4i32;
            let x = (region.x - margin).max(0) as u32;
            let y = (region.y - margin).max(0) as u32;
            let w = ((region.w + margin * 2) as u32).min(img_w.saturating_sub(x));
            let h = ((region.h + margin * 2) as u32).min(img_h.saturating_sub(y));

            if w == 0 || h == 0 {
                continue;
            }

            // #7069: redact destructively with a solid opaque fill instead of a
            // Gaussian blur. The image is sent to a third-party OCR endpoint, so
            // the recipient is exactly the threat actor this control defends
            // against. Blur/pixelation is recoverable (deconvolution / CNN
            // deblurring of structured text); overwriting the pixels with a
            // constant opaque value discards the PII irreversibly.
            redact_region_opaque(result_img, x, y, w, h);
        }
    }

    /// Encodes a (possibly redacted) RGBA image back to PNG bytes. Shared by
    /// the leptess path and the VIS-Q2 native-OCR fallback path (#7602).
    #[cfg(any(feature = "ocr", feature = "native-vision"))]
    fn encode_redacted_png(result_img: image::RgbaImage) -> Result<Vec<u8>, PrivacyDenied> {
        let mut output = std::io::Cursor::new(Vec::new());
        if let Err(e) = image::DynamicImage::ImageRgba8(result_img)
            .write_to(&mut output, image::ImageFormat::Png)
        {
            warn!("PII: image encoding failure: {e}");
            return Err(PrivacyDenied::SanitizationFailed(
                "image encoding failed after PII redaction".to_string(),
            ));
        }
        Ok(output.into_inner())
    }

    #[cfg(any(feature = "ocr", feature = "native-vision"))]
    fn detect_sensitive_regions(
        word_boxes: &[TextBox],
        filter_level: PiiFilterLevel,
    ) -> Vec<SensitiveRegion> {
        use std::collections::HashSet;

        if word_boxes.is_empty() {
            return Vec::new();
        }

        let mut indexed: Vec<(usize, &TextBox)> = word_boxes.iter().enumerate().collect();
        indexed.sort_by_key(|(_, wb)| (wb.y, wb.x));

        let mut sensitive_indices = HashSet::new();

        for (idx, wb) in &indexed {
            if crate::privacy::is_sensitive_segment_with_level(&wb.text, filter_level) {
                sensitive_indices.insert(*idx);
            }
        }

        let line_threshold = 14i32;
        for window_size in 2..=5 {
            if indexed.len() < window_size {
                break;
            }

            for window in indexed.windows(window_size) {
                let y_min = window.iter().map(|(_, wb)| wb.y).min().unwrap_or(0);
                let y_max = window.iter().map(|(_, wb)| wb.y).max().unwrap_or(0);
                if (y_max - y_min).abs() > line_threshold {
                    continue;
                }

                let compact = window
                    .iter()
                    .map(|(_, wb)| wb.text.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                let spaced = window
                    .iter()
                    .map(|(_, wb)| wb.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");

                if crate::privacy::is_sensitive_segment_with_level(&compact, filter_level)
                    || crate::privacy::is_sensitive_segment_with_level(&spaced, filter_level)
                {
                    for (idx, _) in window {
                        sensitive_indices.insert(*idx);
                    }
                }
            }
        }

        if sensitive_indices.is_empty() {
            return Vec::new();
        }

        let raw_regions: Vec<SensitiveRegion> = word_boxes
            .iter()
            .enumerate()
            .filter(|(idx, _)| sensitive_indices.contains(idx))
            .map(|(_, wb)| SensitiveRegion {
                x: wb.x,
                y: wb.y,
                w: wb.w.max(1),
                h: wb.h.max(1),
            })
            .collect();

        Self::merge_sensitive_regions(raw_regions)
    }

    #[cfg(any(feature = "ocr", feature = "native-vision"))]
    fn merge_sensitive_regions(mut regions: Vec<SensitiveRegion>) -> Vec<SensitiveRegion> {
        if regions.is_empty() {
            return regions;
        }

        regions.sort_by_key(|r| (r.y, r.x));
        let mut merged: Vec<SensitiveRegion> = Vec::new();
        let gap = 10i32;

        for region in regions {
            let mut merged_this_round = false;

            for candidate in &mut merged {
                let candidate_right = candidate.x + candidate.w;
                let candidate_bottom = candidate.y + candidate.h;
                let region_right = region.x + region.w;
                let region_bottom = region.y + region.h;

                let overlap_or_near_x =
                    region.x <= candidate_right + gap && region_right + gap >= candidate.x;
                let overlap_or_near_y =
                    region.y <= candidate_bottom + gap && region_bottom + gap >= candidate.y;

                if overlap_or_near_x && overlap_or_near_y {
                    let left = candidate.x.min(region.x);
                    let top = candidate.y.min(region.y);
                    let right = candidate_right.max(region_right);
                    let bottom = candidate_bottom.max(region_bottom);

                    candidate.x = left;
                    candidate.y = top;
                    candidate.w = (right - left).max(1);
                    candidate.h = (bottom - top).max(1);
                    merged_this_round = true;
                    break;
                }
            }

            if !merged_this_round {
                merged.push(region);
            }
        }

        merged
    }

    fn effective_filter_level(&self) -> PiiFilterLevel {
        Self::resolve_filter_level(self.pii_filter_level, self.external_data_policy, false)
    }

    /// Resolve the effective [`PiiFilterLevel`] for an off-device OCR call.
    ///
    /// # WARNING — `bypass_pii_filter_for_external_ocr`
    ///
    /// When this flag is `true` **all PII filtering is bypassed** and raw,
    /// unredacted screen content is transmitted off-device to the external OCR
    /// provider. Every activation is logged at WARN severity with a structured
    /// audit event so that operators can detect unexpected or mis-configured
    /// use (see the `warn!` call below).
    ///
    /// #5966: this parameter only carries the config bypass flag. It is never the
    /// sole authority for the bypass: callers must first clear the explicit
    /// `unredacted_external_ocr` consent tier in
    /// [`Self::prepare_image_for_external_with_override`] (dual consent — generic
    /// `ocr_processing` plus the dedicated raw-off-device-OCR tier) before the flag
    /// is allowed to reach this function. The consent tier is the gating
    /// authority; this flag is the operator opt-in.
    fn resolve_filter_level(
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        bypass_pii_filter_for_external_ocr: bool,
    ) -> PiiFilterLevel {
        if bypass_pii_filter_for_external_ocr {
            // AUDIT: raw, unredacted screen content is about to leave the device.
            // This warn fires on EVERY invocation of the override path so that
            // log aggregators (Loki/OTel) can alert on unexpected activations.
            warn!(
                privacy.bypass = true,
                privacy.pii_filter_override = "bypass_pii_filter_for_external_ocr",
                config.external_data_policy = ?external_data_policy,
                config.pii_filter_level = ?pii_filter_level,
                "PII filtering DISABLED for external OCR: raw unredacted screen \
                 content will be sent off-device. \
                 Ensure explicit user consent covers this data transfer."
            );
            return PiiFilterLevel::Off;
        }

        // #6442 F10: delegate to the shared SSOT floor. AllowFiltered floors the
        // configured level to Basic so OCR images are masked identically to window
        // titles. Previously this arm returned the configured level verbatim — so
        // AllowFiltered + PiiFilterLevel::Off egressed images COMPLETELY UNREDACTED while
        // the window-title path floored to Basic.
        external_data_policy.effective_egress_pii_level(pii_filter_level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};

    fn make_consent_manager_with_permissions(
        permissions: ConsentPermissions,
    ) -> Arc<ConsentManager> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

        manager.grant_consent(permissions, 30).unwrap();

        std::mem::forget(dir);
        Arc::new(manager)
    }

    fn make_consent_manager(ocr_permitted: bool) -> Arc<ConsentManager> {
        if !ocr_permitted {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("consent.json");
            let manager = ConsentManager::new(path);
            std::mem::forget(dir);
            return Arc::new(manager);
        }

        make_consent_manager_with_permissions(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            ..Default::default()
        })
    }

    fn make_unredacted_external_ocr_consent_manager() -> Arc<ConsentManager> {
        make_consent_manager_with_permissions(ConsentPermissions {
            ocr_processing: true,
            screen_capture: true,
            unredacted_external_ocr: true,
            ..Default::default()
        })
    }

    fn make_gateway(ocr_permitted: bool, policy: ExternalDataPolicy) -> PrivacyGateway {
        let consent = make_consent_manager(ocr_permitted);
        PrivacyGateway::new(
            consent,
            PiiFilterLevel::Standard,
            policy,
            PrivacyConfig::default(),
        )
    }

    #[tokio::test]
    async fn deny_without_consent() {
        let gw = make_gateway(false, ExternalDataPolicy::PiiFilterStrict);
        let err = gw
            .prepare_image_for_external(b"img", "VSCode", "main.rs")
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivacyDenied::NoConsent),
            "no-consent must produce NoConsent, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn deny_sensitive_app() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStrict);
        let err = gw
            .prepare_image_for_external(b"img", "1Password", "Vault")
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivacyDenied::SensitiveApp(_)),
            "sensitive app must produce SensitiveApp, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn allow_normal_app_when_unredacted_override_is_explicit() {
        let consent = make_unredacted_external_ocr_consent_manager();
        let gw = PrivacyGateway::new(
            consent,
            PiiFilterLevel::Off,
            ExternalDataPolicy::AllowFiltered,
            PrivacyConfig::default(),
        );
        let result = gw
            .prepare_image_for_external_with_override(b"img", "VSCode", "main.rs", true)
            .await;
        let sanitized = result.expect("explicit unredacted override must succeed with consent");
        assert!(sanitized.metadata_stripped);
        assert_eq!(sanitized.redacted_regions, 0);
    }

    #[tokio::test]
    async fn deny_unredacted_override_with_generic_ocr_consent_only() {
        let consent = make_consent_manager(true);
        let gw = PrivacyGateway::new(
            consent,
            PiiFilterLevel::Off,
            ExternalDataPolicy::AllowFiltered,
            PrivacyConfig::default(),
        );
        let err = gw
            .prepare_image_for_external_with_override(b"img", "VSCode", "main.rs", true)
            .await
            .unwrap_err();

        assert!(
            matches!(err, PrivacyDenied::UnredactedExternalOcrConsentRequired),
            "raw off-device OCR must require a dedicated consent tier, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn prepare_image_fails_closed_when_filtering_cannot_sanitize_image() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStrict);
        let err = gw
            .prepare_image_for_external(b"not-an-image", "VSCode", "main.rs")
            .await
            .unwrap_err();
        assert!(
            matches!(err, PrivacyDenied::SanitizationFailed(_)),
            "strict filtering with invalid image bytes must produce SanitizationFailed, got: {err:?}"
        );
    }

    #[test]
    fn text_filter_no_consent() {
        let gw = make_gateway(false, ExternalDataPolicy::PiiFilterStrict);
        let err = gw
            .prepare_text_for_external(&["hello".to_string()])
            .unwrap_err();
        assert!(
            matches!(err, PrivacyDenied::NoConsent),
            "no-consent text filter must produce NoConsent, got: {err:?}"
        );
    }

    #[test]
    fn text_filter_with_consent() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStandard);
        let texts = vec!["user@example.com".to_string(), "hello world".to_string()];
        let result = gw.prepare_text_for_external(&texts);
        let filtered =
            result.expect("prepare_text_for_external must succeed when OCR consent is granted");
        assert_eq!(
            filtered.len(),
            2,
            "output must contain one entry per input string"
        );
        assert!(
            filtered[0].contains("[EMAIL]") || filtered[0] == "user@example.com",
            "PiiFilterStandard must either mask the email as [EMAIL] or pass it through unchanged"
        );
    }

    #[test]
    fn text_filter_denies_excluded_title_surface() {
        let consent = make_consent_manager(true);
        let privacy_config = PrivacyConfig {
            excluded_title_patterns: vec!["*private*".to_string()],
            ..PrivacyConfig::default()
        };
        let gw = PrivacyGateway::new(
            consent,
            PiiFilterLevel::Standard,
            ExternalDataPolicy::PiiFilterStandard,
            privacy_config,
        );

        let err = gw
            .prepare_text_for_external_with_surface(
                &["password: redaction-fixture-secret".to_string()],
                "Notes",
                "Private banking recovery codes",
            )
            .unwrap_err();

        assert!(
            matches!(err, PrivacyDenied::ExcludedByPolicy),
            "excluded title surfaces must not enter external text preparation, got: {err:?}"
        );
    }

    #[test]
    fn text_filter_denies_sensitive_apps_beyond_seed_keywords() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStandard);

        let err = gw
            .prepare_text_for_external_with_surface(
                &["recovery code: 123456".to_string()],
                "Keeper",
                "Vault",
            )
            .unwrap_err();

        assert!(
            matches!(&err, PrivacyDenied::SensitiveApp(app) if app == "Keeper"),
            "non-seeded sensitive app must fail closed before text leaves the device, got: {err:?}"
        );
    }

    #[test]
    fn effective_filter_level_strict() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStrict);
        assert_eq!(gw.effective_filter_level(), PiiFilterLevel::Strict);
    }

    #[test]
    fn effective_filter_level_standard() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStandard);
        assert_eq!(gw.effective_filter_level(), PiiFilterLevel::Standard);
    }

    #[test]
    fn effective_filter_level_allow_filtered() {
        let gw = make_gateway(true, ExternalDataPolicy::AllowFiltered);
        assert_eq!(gw.effective_filter_level(), PiiFilterLevel::Standard); // user setting
    }

    #[tokio::test]
    async fn blur_pii_regions_rejects_invalid_image() {
        let data = b"not-an-image";
        let result = PrivacyGateway::blur_pii_regions(data, PiiFilterLevel::Standard).await;
        assert!(matches!(result, Err(PrivacyDenied::SanitizationFailed(_))));
    }

    // --- #7602 (VIS-Q2, audit ②): native-OCR fallback for blur_pii_regions ---
    //
    // Shipped macOS/Windows builds compile `native-vision` (default-on) but
    // NOT `ocr` (leptess is never enabled by `src-tauri`). Before this fix,
    // `blur_pii_regions`'s `not(feature = "ocr")` arm unconditionally returned
    // `SanitizationFailed` — ANY image, valid or not, with ANY native OCR
    // engine available, still hard-failed. A configured external/BYOK OCR
    // provider could never succeed on such a build: a dead capability,
    // fail-closed only by accident of always failing. These tests exercise
    // `blur_pii_regions_native` directly (mirrors `processor.rs`'s
    // `with_ocr_provider` injection seam, #7477) so the fallback is
    // deterministic without a real platform Vision.framework / Media.Ocr call.
    #[cfg(all(feature = "native-vision", not(feature = "ocr")))]
    mod native_fallback_tests {
        use super::*;
        use maekon_core::ports::ocr_provider::{FakeOcrProvider, OcrResult};

        /// A valid, decodable 200x80 solid-white PNG fixture image.
        fn test_image_bytes() -> Vec<u8> {
            use image::{DynamicImage, Rgba, RgbaImage};
            let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
                200,
                80,
                Rgba([255, 255, 255, 255]),
            ));
            let mut bytes = Vec::new();
            img.write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("PNG encode of a synthetic test fixture must not fail");
            bytes
        }

        // Fails before #7602: with no native-OCR call site at all, this exact
        // scenario (valid image, native engine available) returned
        // `SanitizationFailed` unconditionally. Passes after: the fallback
        // detects the email via the injected native provider and redacts it.
        #[tokio::test]
        async fn detects_and_masks_regions_via_native_ocr_when_leptess_absent() {
            let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
                text: "contact admin@company.com".to_string(),
                x: 10,
                y: 20,
                width: 180,
                height: 24,
                confidence: 0.95,
            }]));

            let (encoded, redacted_regions) = PrivacyGateway::blur_pii_regions_native(
                &test_image_bytes(),
                PiiFilterLevel::Standard,
                Some(provider),
            )
            .await
            .expect("native OCR fallback must succeed when a provider is wired");

            assert_eq!(
                redacted_regions, 1,
                "the sensitive email region must be detected and redacted"
            );

            // The output must still be a valid, decodable image.
            let redacted = image::load_from_memory(&encoded)
                .expect("redacted output must be a valid image")
                .to_rgba8();
            // The email element (x=10,y=20,w=180,h=24) expanded by the shared
            // 4px margin covers (50, 30) — same destructive opaque-black
            // redaction contract as the leptess path (#7069).
            assert_eq!(*redacted.get_pixel(50, 30), image::Rgba([0, 0, 0, 255]));
        }

        // Symmetric with the leptess path: OCR succeeding but finding NO
        // sensitive text must pass the image through byte-identical (0
        // regions redacted) — only the ABSENCE of any verifiable OCR output
        // fails closed, not merely "found nothing sensitive".
        #[tokio::test]
        async fn passes_through_unchanged_when_native_ocr_finds_no_sensitive_text() {
            let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
                text: "hello world".to_string(),
                x: 10,
                y: 20,
                width: 100,
                height: 24,
                confidence: 0.95,
            }]));
            let image_bytes = test_image_bytes();

            let (encoded, redacted_regions) = PrivacyGateway::blur_pii_regions_native(
                &image_bytes,
                PiiFilterLevel::Standard,
                Some(provider),
            )
            .await
            .expect("non-sensitive OCR output must not fail closed");

            assert_eq!(redacted_regions, 0);
            assert_eq!(
                encoded, image_bytes,
                "non-sensitive image must pass through byte-identical"
            );
        }

        // #7602 audit ①/②: the "no OCR source at all" case (e.g. Linux, where
        // native_ocr::create_native_ocr() always returns None, and leptess is
        // not compiled into shipped builds) must still fail closed with a
        // clear, specific error — never silently emit an unredacted image.
        // This is the same code path Linux hits via the public
        // `blur_pii_regions` entry point (which passes
        // `native_ocr::create_native_ocr()` — `None` on Linux — straight
        // through to this function).
        #[tokio::test]
        async fn fails_closed_when_no_native_engine_is_available() {
            let err = PrivacyGateway::blur_pii_regions_native(
                &test_image_bytes(),
                PiiFilterLevel::Standard,
                None,
            )
            .await
            .unwrap_err();

            assert!(
                matches!(err, PrivacyDenied::SanitizationFailed(_)),
                "no OCR engine at all must fail closed, got: {err:?}"
            );
        }

        // Mirrors the leptess "local OCR found no verifiable text" guard: an
        // OCR call that SUCCEEDS but extracts zero elements cannot prove the
        // image is PII-free, so it must fail closed rather than pass the
        // (unverified) image through untouched.
        #[tokio::test]
        async fn fails_closed_when_native_ocr_returns_no_elements() {
            let provider = Arc::new(FakeOcrProvider::new(vec![]));

            let err = PrivacyGateway::blur_pii_regions_native(
                &test_image_bytes(),
                PiiFilterLevel::Standard,
                Some(provider),
            )
            .await
            .unwrap_err();

            assert!(
                matches!(err, PrivacyDenied::SanitizationFailed(_)),
                "empty native OCR output must fail closed, got: {err:?}"
            );
        }

        // Invalid image bytes must fail closed identically to the leptess
        // path (same decode-failure guard, shared error family) — proves the
        // fallback did not weaken the pre-OCR image validation.
        #[tokio::test]
        async fn fails_closed_on_undecodable_image_bytes() {
            let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
                text: "unreachable".to_string(),
                x: 0,
                y: 0,
                width: 10,
                height: 10,
                confidence: 1.0,
            }]));

            let err = PrivacyGateway::blur_pii_regions_native(
                b"not-an-image",
                PiiFilterLevel::Standard,
                Some(provider),
            )
            .await
            .unwrap_err();

            assert!(
                matches!(err, PrivacyDenied::SanitizationFailed(_)),
                "undecodable bytes must fail closed before any OCR call, got: {err:?}"
            );
        }
    }

    #[tokio::test]
    async fn prepare_image_unredacted_override_skips_blur() {
        let consent = make_unredacted_external_ocr_consent_manager();
        let gw = PrivacyGateway::new(
            consent,
            PiiFilterLevel::Off,
            ExternalDataPolicy::AllowFiltered,
            PrivacyConfig::default(),
        );
        let result = gw
            .prepare_image_for_external_with_override(b"img", "VSCode", "main.rs", true)
            .await;
        let sanitized = result.expect(
            "explicit unredacted override must not invoke blur pipeline; even invalid bytes succeed",
        );
        // Override contract: image bytes are passed through unmodified, no regions redacted.
        assert_eq!(sanitized.image_data, b"img".to_vec());
        assert_eq!(sanitized.redacted_regions, 0);
    }

    #[tokio::test]
    async fn sanitize_image_for_external_policy_opt_out_returns_original() {
        let raw = b"raw-image";
        let sanitized = PrivacyGateway::sanitize_image_for_external_policy(
            raw,
            PiiFilterLevel::Strict,
            ExternalDataPolicy::PiiFilterStrict,
            true,
        )
        .await
        .unwrap();
        assert_eq!(sanitized.image_data, raw.to_vec());
        assert_eq!(sanitized.redacted_regions, 0);
        assert!(sanitized.metadata_stripped);
    }

    #[tokio::test]
    async fn sanitize_image_for_external_policy_without_opt_out_fails_closed_when_pipeline_fails() {
        let raw = b"not-an-image";
        let result = PrivacyGateway::sanitize_image_for_external_policy(
            raw,
            PiiFilterLevel::Standard,
            ExternalDataPolicy::PiiFilterStandard,
            false,
        )
        .await;
        assert!(matches!(result, Err(PrivacyDenied::SanitizationFailed(_))));
    }

    #[test]
    fn privacy_denied_display() {
        let d1 = PrivacyDenied::NoConsent;
        assert!(d1.to_string().contains("consent"));
        let d1b = PrivacyDenied::UnredactedExternalOcrConsentRequired;
        assert!(d1b.to_string().contains("Unredacted external OCR consent"));
        let d2 = PrivacyDenied::SensitiveApp("Bank".to_string());
        assert!(d2.to_string().contains("Bank"));
        let d3 = PrivacyDenied::ExcludedByPolicy;
        assert!(d3.to_string().contains("policy"));
        let d4 = PrivacyDenied::SanitizationFailed("decode".to_string());
        assert!(d4.to_string().contains("decode"));
    }

    // --- #5966: bypass_pii_filter_for_external_ocr audit tests ---

    /// Verifies that `resolve_filter_level` returns `PiiFilterLevel::Off` when
    /// `bypass_pii_filter_for_external_ocr` is `true`, regardless of the configured
    /// policy and filter level. The audit `warn!` is emitted on this path; this
    /// test exercises the branch so that coverage tooling and reviewers can
    /// confirm the warn macro expands without panic.
    #[test]
    fn resolve_filter_level_override_returns_off_and_emits_audit_warn() {
        for policy in [
            ExternalDataPolicy::PiiFilterStrict,
            ExternalDataPolicy::PiiFilterStandard,
            ExternalDataPolicy::AllowFiltered,
        ] {
            for level in [
                PiiFilterLevel::Strict,
                PiiFilterLevel::Standard,
                PiiFilterLevel::Basic,
                PiiFilterLevel::Off,
            ] {
                let resolved = PrivacyGateway::resolve_filter_level(level, policy, true);
                assert_eq!(
                    resolved,
                    PiiFilterLevel::Off,
                    "override must force Off regardless of policy={policy:?} level={level:?}"
                );
            }
        }
    }

    /// Verifies that `resolve_filter_level` respects the normal policy path
    /// when `bypass_pii_filter_for_external_ocr` is `false` — guard against the
    /// override branch accidentally short-circuiting normal operation.
    #[test]
    fn resolve_filter_level_no_override_respects_policy() {
        assert_eq!(
            PrivacyGateway::resolve_filter_level(
                PiiFilterLevel::Basic,
                ExternalDataPolicy::PiiFilterStrict,
                false,
            ),
            PiiFilterLevel::Strict,
            "PiiFilterStrict must return Strict regardless of user filter level"
        );
        assert_eq!(
            PrivacyGateway::resolve_filter_level(
                PiiFilterLevel::Basic,
                ExternalDataPolicy::PiiFilterStandard,
                false,
            ),
            PiiFilterLevel::Standard,
        );
        assert_eq!(
            PrivacyGateway::resolve_filter_level(
                PiiFilterLevel::Basic,
                ExternalDataPolicy::AllowFiltered,
                false,
            ),
            PiiFilterLevel::Basic,
            "AllowFiltered honours a configured level at or above the Basic floor"
        );
        // #6442 F10: AllowFiltered floors Off -> Basic so OCR images are not egressed
        // unredacted (previously this returned Off, leaving images completely unmasked
        // while the window-title path floored to Basic).
        assert_eq!(
            PrivacyGateway::resolve_filter_level(
                PiiFilterLevel::Off,
                ExternalDataPolicy::AllowFiltered,
                false,
            ),
            PiiFilterLevel::Basic,
            "AllowFiltered must floor Off to Basic (unify with the window-title egress)"
        );
    }

    /// Verifies that the static `sanitize_image_for_external_policy` path also
    /// exercises the override audit branch (warm coverage for log aggregator
    /// alert paths). The function must succeed and return the raw bytes
    /// unchanged when the override is active.
    #[tokio::test]
    async fn sanitize_image_for_external_policy_override_emits_warn_and_passes_bytes_through() {
        let raw = b"sentinel-bytes";
        let result = PrivacyGateway::sanitize_image_for_external_policy(
            raw,
            PiiFilterLevel::Strict,
            ExternalDataPolicy::PiiFilterStrict,
            true, // override — triggers audit warn!
        )
        .await
        .expect("override path must succeed (bypass means no sanitization pipeline)");
        assert_eq!(
            result.image_data, raw,
            "override path must return bytes unchanged"
        );
        assert_eq!(result.redacted_regions, 0);
    }

    // --- #7069: destructive (opaque-fill) PII redaction for off-device OCR ---

    /// Per-channel population variance summed across RGBA, over a rectangular
    /// region. Equals exactly `0.0` iff every pixel in the region is
    /// byte-identical (constant-valued); any smoothing/structure is > 0.
    fn region_variance(img: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> f64 {
        let mut sums = [0f64; 4];
        let mut sq_sums = [0f64; 4];
        let n = f64::from(w * h);
        for py in y..y + h {
            for px in x..x + w {
                let p = img.get_pixel(px, py).0;
                for c in 0..4 {
                    let v = f64::from(p[c]);
                    sums[c] += v;
                    sq_sums[c] += v * v;
                }
            }
        }
        let mut total = 0f64;
        for c in 0..4 {
            let mean = sums[c] / n;
            total += sq_sums[c] / n - mean * mean;
        }
        total
    }

    /// #7069 regression guard: PII regions destined for an external (off-device)
    /// OCR endpoint must be redacted with a *destructive* opaque fill, not a
    /// recoverable Gaussian blur.
    ///
    /// Asserts that after [`redact_region_opaque`] the region is constant-valued
    /// (variance == 0, and `min == max` per channel), and documents that a
    /// Gaussian blur of the same content is NOT constant-valued. This guard fails
    /// against blur (the previous behaviour) and passes against the opaque fill.
    #[test]
    fn redact_region_opaque_is_destructive_unlike_blur() {
        use image::{DynamicImage, Rgba, RgbaImage};

        // High-contrast gradient so any non-destructive transform leaves
        // recoverable (non-constant) structure.
        let (w, h) = (32u32, 32u32);
        let mut img = RgbaImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 8 + y * 5) % 256) as u8;
                img.put_pixel(x, y, Rgba([v, 255 - v, v / 2, 255]));
            }
        }

        let (rx, ry, rw, rh) = (4u32, 4u32, 20u32, 20u32);

        // Sanity: the region starts non-constant.
        assert!(
            region_variance(&img, rx, ry, rw, rh) > 0.0,
            "fixture region must start with recoverable (non-zero variance) content"
        );

        // The old #7069 treatment (Gaussian blur) must NOT make the region
        // constant — this is exactly why blur is insufficient redaction.
        let blurred = DynamicImage::ImageRgba8(img.clone()).blur(8.0).to_rgba8();
        assert!(
            region_variance(&blurred, rx, ry, rw, rh) > 0.0,
            "Gaussian blur must leave recoverable (non-constant) structure — \
             the anti-pattern #7069 replaces; a blur-based redaction would fail here"
        );

        // The destructive opaque fill must overwrite every region pixel and yield
        // a constant-valued (zero-variance) region.
        let filled = redact_region_opaque(&mut img, rx, ry, rw, rh);
        assert_eq!(
            filled,
            rw * rh,
            "every pixel inside the region must be overwritten"
        );
        assert_eq!(
            region_variance(&img, rx, ry, rw, rh),
            0.0,
            "opaque fill must yield a constant-valued (zero-variance) region, not a smoothed one"
        );

        // min == max per channel (integer-exact constancy) and the fill is opaque
        // black — nothing recoverable, full alpha (no transparency leak).
        let first = *img.get_pixel(rx, ry);
        for y in ry..ry + rh {
            for x in rx..rx + rw {
                assert_eq!(
                    *img.get_pixel(x, y),
                    Rgba([0, 0, 0, 255]),
                    "redacted pixels must all be opaque black (min == max)"
                );
                assert_eq!(*img.get_pixel(x, y), first);
            }
        }

        // Pixels just outside the region are untouched (redaction is bounded).
        assert_ne!(
            *img.get_pixel(rx + rw, ry),
            Rgba([0, 0, 0, 255]),
            "redaction must not bleed past the region boundary"
        );
    }
}
