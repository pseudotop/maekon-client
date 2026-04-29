use std::sync::Arc;

use oneshim_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use oneshim_core::consent::ConsentManager;

use crate::privacy::{is_sensitive_app, sanitize_title_with_level, should_exclude};

#[derive(Debug, Clone)]
pub enum PrivacyDenied {
    NoConsent,
    SensitiveApp(String),
    ExcludedByPolicy,
}

impl std::fmt::Display for PrivacyDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsent => write!(f, "OCR consent is required"),
            Self::SensitiveApp(app) => write!(f, "Blocked sensitive app: {}", app),
            Self::ExcludedByPolicy => write!(f, "Excluded by policy"),
        }
    }
}

#[derive(Debug)]
pub struct SanitizedImage {
    pub image_data: Vec<u8>,
    pub metadata_stripped: bool,
    pub redacted_regions: usize,
}

#[cfg(feature = "ocr")]
#[derive(Debug, Clone, Copy)]
struct SensitiveRegion {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

// PrivacyGateway

pub struct PrivacyGateway {
    consent_manager: Arc<ConsentManager>,
    pii_filter_level: PiiFilterLevel,
    external_data_policy: ExternalDataPolicy,
    privacy_config: PrivacyConfig,
}

impl PrivacyGateway {
    pub fn new(
        consent_manager: Arc<ConsentManager>,
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
        allow_unredacted_external_ocr: bool,
    ) -> SanitizedImage {
        let filter_level = Self::resolve_filter_level(
            pii_filter_level,
            external_data_policy,
            allow_unredacted_external_ocr,
        );
        let (sanitized_data, redacted_regions) = if filter_level == PiiFilterLevel::Off {
            (image_data.to_vec(), 0)
        } else {
            Self::blur_pii_regions(image_data, filter_level).await
        };

        SanitizedImage {
            image_data: sanitized_data,
            metadata_stripped: true,
            redacted_regions,
        }
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
        allow_unredacted_external_ocr: bool,
    ) -> Result<SanitizedImage, PrivacyDenied> {
        if !self.consent_manager.is_permitted(|p| p.ocr_processing) {
            return Err(PrivacyDenied::NoConsent);
        }

        if is_sensitive_app(active_app) {
            return Err(PrivacyDenied::SensitiveApp(active_app.to_string()));
        }

        if should_exclude(
            active_app,
            window_title,
            &self.privacy_config.excluded_apps,
            &self.privacy_config.excluded_app_patterns,
            &self.privacy_config.excluded_title_patterns,
            self.privacy_config.auto_exclude_sensitive,
        ) {
            return Err(PrivacyDenied::ExcludedByPolicy);
        }

        let filter_level = Self::resolve_filter_level(
            self.pii_filter_level,
            self.external_data_policy,
            allow_unredacted_external_ocr,
        );
        let (sanitized_data, redacted_regions) = if filter_level == PiiFilterLevel::Off {
            (image_data.to_vec(), 0)
        } else {
            Self::blur_pii_regions(image_data, filter_level).await
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
        if !self.consent_manager.is_permitted(|p| p.ocr_processing) {
            return Err(PrivacyDenied::NoConsent);
        }

        let filter_level = self.effective_filter_level();
        Ok(texts
            .iter()
            .map(|t| sanitize_title_with_level(t, filter_level))
            .collect())
    }

    #[allow(clippy::unused_async)] // await used only with `ocr` feature
    async fn blur_pii_regions(image_data: &[u8], filter_level: PiiFilterLevel) -> (Vec<u8>, usize) {
        #[cfg(feature = "ocr")]
        {
            use crate::ocr::OcrExtractor;
            use image::GenericImage;
            use image::GenericImageView;
            use tracing::{debug, warn};

            let img = match image::load_from_memory(image_data) {
                Ok(img) => img,
                Err(e) => {
                    warn!("PII: image decoding failure: {e}");
                    return (image_data.to_vec(), 0);
                }
            };

            let extractor = OcrExtractor::new(None);
            let word_boxes = match extractor.extract_words_with_boxes(&img).await {
                Ok(boxes) => boxes,
                Err(e) => {
                    debug!("PII: OCR failure: {e}, returning original image");
                    return (image_data.to_vec(), 0);
                }
            };

            if word_boxes.is_empty() {
                return (image_data.to_vec(), 0);
            }

            let pii_regions = Self::detect_sensitive_regions(&word_boxes, filter_level);

            if pii_regions.is_empty() {
                return (image_data.to_vec(), 0);
            }

            debug!(
                "PII blur: detected and merged {} region(s) from {} word box(es)",
                pii_regions.len(),
                word_boxes.len()
            );

            let mut result_img = img.to_rgba8();
            let (img_w, img_h) = result_img.dimensions();

            for region in &pii_regions {
                let margin = 4i32;
                let x = (region.x - margin).max(0) as u32;
                let y = (region.y - margin).max(0) as u32;
                let w = ((region.w + margin * 2) as u32).min(img_w.saturating_sub(x));
                let h = ((region.h + margin * 2) as u32).min(img_h.saturating_sub(y));

                if w == 0 || h == 0 {
                    continue;
                }

                let roi = image::DynamicImage::ImageRgba8(result_img.clone()).crop_imm(x, y, w, h);
                let blurred = roi.blur(8.0);
                let blurred_rgba = blurred.to_rgba8();

                for dy in 0..h.min(blurred_rgba.height()) {
                    for dx in 0..w.min(blurred_rgba.width()) {
                        let pixel = blurred_rgba.get_pixel(dx, dy);
                        if x + dx < img_w && y + dy < img_h {
                            result_img.put_pixel(x + dx, y + dy, *pixel);
                        }
                    }
                }
            }

            let mut output = std::io::Cursor::new(Vec::new());
            if let Err(e) = image::DynamicImage::ImageRgba8(result_img)
                .write_to(&mut output, image::ImageFormat::Png)
            {
                warn!("PII: image encoding failure: {e}");
                return (image_data.to_vec(), 0);
            }

            (output.into_inner(), pii_regions.len())
        }

        #[cfg(not(feature = "ocr"))]
        {
            let _ = filter_level;
            (image_data.to_vec(), 0)
        }
    }

    #[cfg(feature = "ocr")]
    fn detect_sensitive_regions(
        word_boxes: &[crate::ocr::OcrWordBox],
        filter_level: PiiFilterLevel,
    ) -> Vec<SensitiveRegion> {
        use std::collections::HashSet;

        if word_boxes.is_empty() {
            return Vec::new();
        }

        let mut indexed: Vec<(usize, &crate::ocr::OcrWordBox)> =
            word_boxes.iter().enumerate().collect();
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

    #[cfg(feature = "ocr")]
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

    fn resolve_filter_level(
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        allow_unredacted_external_ocr: bool,
    ) -> PiiFilterLevel {
        if allow_unredacted_external_ocr {
            return PiiFilterLevel::Off;
        }

        match external_data_policy {
            ExternalDataPolicy::PiiFilterStrict => PiiFilterLevel::Strict,
            ExternalDataPolicy::PiiFilterStandard => PiiFilterLevel::Standard,
            ExternalDataPolicy::AllowFiltered => pii_filter_level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::consent::ConsentPermissions;

    fn make_consent_manager(ocr_permitted: bool) -> Arc<ConsentManager> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let mut manager = ConsentManager::new(path);

        if ocr_permitted {
            let perms = ConsentPermissions {
                ocr_processing: true,
                screen_capture: true,
                ..Default::default()
            };
            manager.grant_consent(perms, 30).unwrap();
        }

        std::mem::forget(dir);
        Arc::new(manager)
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
        let result = gw
            .prepare_image_for_external(b"img", "VSCode", "main.rs")
            .await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PrivacyDenied::NoConsent));
    }

    #[tokio::test]
    async fn deny_sensitive_app() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStrict);
        let result = gw
            .prepare_image_for_external(b"img", "1Password", "Vault")
            .await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PrivacyDenied::SensitiveApp(_)
        ));
    }

    #[tokio::test]
    async fn allow_normal_app() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStrict);
        let result = gw
            .prepare_image_for_external(b"img", "VSCode", "main.rs")
            .await;
        assert!(result.is_ok());
        let sanitized = result.unwrap();
        assert!(sanitized.metadata_stripped);
        assert_eq!(sanitized.redacted_regions, 0);
    }

    #[test]
    fn text_filter_no_consent() {
        let gw = make_gateway(false, ExternalDataPolicy::PiiFilterStrict);
        let result = gw.prepare_text_for_external(&["hello".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn text_filter_with_consent() {
        let gw = make_gateway(true, ExternalDataPolicy::PiiFilterStandard);
        let texts = vec!["user@example.com".to_string(), "hello world".to_string()];
        let result = gw.prepare_text_for_external(&texts);
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered[0].contains("[EMAIL]") || filtered[0] == "user@example.com");
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
    async fn blur_pii_regions_returns_data_for_empty_image() {
        let data = b"not-an-image";
        let result = PrivacyGateway::blur_pii_regions(data, PiiFilterLevel::Standard).await;
        assert_eq!(result.0, data.to_vec());
        assert_eq!(result.1, 0);
    }

    #[tokio::test]
    async fn blur_pii_regions_off_returns_original() {
        let data = b"test-data";
        let result = PrivacyGateway::blur_pii_regions(data, PiiFilterLevel::Off).await;
        assert!(!result.0.is_empty());
        assert_eq!(result.1, 0);
    }

    #[tokio::test]
    async fn prepare_image_off_level_skips_blur() {
        let consent = make_consent_manager(true);
        let gw = PrivacyGateway::new(
            consent,
            PiiFilterLevel::Off,
            ExternalDataPolicy::AllowFiltered,
            PrivacyConfig::default(),
        );
        let result = gw
            .prepare_image_for_external(b"img", "VSCode", "main.rs")
            .await;
        assert!(result.is_ok());
        let sanitized = result.unwrap();
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
        .await;
        assert_eq!(sanitized.image_data, raw.to_vec());
        assert_eq!(sanitized.redacted_regions, 0);
        assert!(sanitized.metadata_stripped);
    }

    #[tokio::test]
    async fn sanitize_image_for_external_policy_without_opt_out_runs_pipeline() {
        let raw = b"not-an-image";
        let sanitized = PrivacyGateway::sanitize_image_for_external_policy(
            raw,
            PiiFilterLevel::Standard,
            ExternalDataPolicy::PiiFilterStandard,
            false,
        )
        .await;
        assert_eq!(sanitized.image_data, raw.to_vec());
        assert_eq!(sanitized.redacted_regions, 0);
        assert!(sanitized.metadata_stripped);
    }

    #[test]
    fn privacy_denied_display() {
        let d1 = PrivacyDenied::NoConsent;
        assert!(d1.to_string().contains("consent"));
        let d2 = PrivacyDenied::SensitiveApp("Bank".to_string());
        assert!(d2.to_string().contains("Bank"));
        let d3 = PrivacyDenied::ExcludedByPolicy;
        assert!(d3.to_string().contains("policy"));
    }
}
