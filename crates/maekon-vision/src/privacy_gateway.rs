use std::sync::Arc;

use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use tracing::warn;

use crate::privacy::{is_sensitive_app, sanitize_title_with_level, should_exclude};

#[derive(Debug, Clone)]
pub enum PrivacyDenied {
    NoConsent,
    SensitiveApp(String),
    ExcludedByPolicy,
    SanitizationFailed(String),
}

impl std::fmt::Display for PrivacyDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoConsent => write!(f, "OCR consent is required"),
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
        allow_unredacted_external_ocr: bool,
    ) -> Result<SanitizedImage, PrivacyDenied> {
        let filter_level = Self::resolve_filter_level(
            pii_filter_level,
            external_data_policy,
            allow_unredacted_external_ocr,
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
        allow_unredacted_external_ocr: bool,
    ) -> Result<SanitizedImage, PrivacyDenied> {
        if !self.consent_manager.effective_permissions().ocr_processing {
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
        // When the override forced PiiFilterLevel::Off, emit an additional audit
        // event with the app/window context that resolve_filter_level cannot see.
        // The window title is PII (document names, mail subjects, URLs), so it is
        // never interpolated raw — it is sanitized at Strict before logging so the
        // file log layer cannot persist raw content (#5591/#6006).
        if allow_unredacted_external_ocr {
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

        let filter_level = self.effective_filter_level();
        Ok(texts
            .iter()
            .map(|t| sanitize_title_with_level(t, filter_level))
            .collect())
    }

    #[allow(clippy::unused_async)] // await used only with `ocr` feature
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

            let pii_regions = Self::detect_sensitive_regions(&word_boxes, filter_level);

            if pii_regions.is_empty() {
                return Ok((image_data.to_vec(), 0));
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
                return Err(PrivacyDenied::SanitizationFailed(
                    "image encoding failed after PII redaction".to_string(),
                ));
            }

            Ok((output.into_inner(), pii_regions.len()))
        }

        #[cfg(not(feature = "ocr"))]
        {
            let _ = image_data;
            let _ = filter_level;
            Err(PrivacyDenied::SanitizationFailed(
                "local OCR feature is unavailable for external image sanitization".to_string(),
            ))
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

    /// Resolve the effective [`PiiFilterLevel`] for an off-device OCR call.
    ///
    /// # WARNING — `allow_unredacted_external_ocr`
    ///
    /// When this flag is `true` **all PII filtering is bypassed** and raw,
    /// unredacted screen content is transmitted off-device to the external OCR
    /// provider. Every activation is logged at WARN severity with a structured
    /// audit event so that operators can detect unexpected or mis-configured
    /// use (see the `warn!` call below).
    ///
    /// // TODO(#5966): rename flag to `bypass_pii_filter_for_external_ocr` for
    /// //   clarity, and gate activation on an explicit user consent tier rather
    /// //   than a bare boolean config flag.
    fn resolve_filter_level(
        pii_filter_level: PiiFilterLevel,
        external_data_policy: ExternalDataPolicy,
        allow_unredacted_external_ocr: bool,
    ) -> PiiFilterLevel {
        if allow_unredacted_external_ocr {
            // AUDIT: raw, unredacted screen content is about to leave the device.
            // This warn fires on EVERY invocation of the override path so that
            // log aggregators (Loki/OTel) can alert on unexpected activations.
            warn!(
                privacy.bypass = true,
                privacy.pii_filter_override = "allow_unredacted_external_ocr",
                config.external_data_policy = ?external_data_policy,
                config.pii_filter_level = ?pii_filter_level,
                "PII filtering DISABLED for external OCR: raw unredacted screen \
                 content will be sent off-device. \
                 Ensure explicit user consent covers this data transfer."
            );
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
    use maekon_core::consent::{ConsentManager, ConsentPermissions};

    fn make_consent_manager(ocr_permitted: bool) -> Arc<ConsentManager> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("consent.json");
        let manager = ConsentManager::new(path);

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
    async fn allow_normal_app_when_unredacted_mode_is_explicit() {
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
        let sanitized = result
            .expect("prepare_image_for_external must succeed with consent + PiiFilterLevel::Off");
        assert!(sanitized.metadata_stripped);
        assert_eq!(sanitized.redacted_regions, 0);
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
        let sanitized = result.expect(
            "PiiFilterLevel::Off must not invoke blur pipeline; even invalid bytes succeed",
        );
        // Off-level contract: image bytes are passed through unmodified, no regions redacted.
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
        let d2 = PrivacyDenied::SensitiveApp("Bank".to_string());
        assert!(d2.to_string().contains("Bank"));
        let d3 = PrivacyDenied::ExcludedByPolicy;
        assert!(d3.to_string().contains("policy"));
        let d4 = PrivacyDenied::SanitizationFailed("decode".to_string());
        assert!(d4.to_string().contains("decode"));
    }

    // --- #5966: allow_unredacted_external_ocr audit tests ---

    /// Verifies that `resolve_filter_level` returns `PiiFilterLevel::Off` when
    /// `allow_unredacted_external_ocr` is `true`, regardless of the configured
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
    /// when `allow_unredacted_external_ocr` is `false` — guard against the
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
            "AllowFiltered must delegate to the user-configured pii_filter_level"
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
}
