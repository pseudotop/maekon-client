use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::gui::GuiBenchmarkPrivacyStatus;
use maekon_core::ports::ocr_provider::OcrResult;
use serde::Serialize;

use crate::privacy::{detect_pii_markers_with_level, sanitize_title_with_level};

const MAX_TEXT_SAMPLES: usize = 8;

#[derive(Debug, Clone, Copy)]
pub struct WindowsNativeOcrSampleInput<'a> {
    pub sample_label: &'a str,
    pub image_width: u32,
    pub image_height: u32,
    pub image_format: &'a str,
    pub display_scale: f64,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WindowsNativeOcrSampleSummary {
    pub ok: bool,
    pub sample_label: String,
    pub image_width: u32,
    pub image_height: u32,
    pub image_format: String,
    pub display_scale: f64,
    pub language_assumption: &'static str,
    pub latency_ms: u64,
    pub extraction_count: usize,
    pub bounds_present_count: usize,
    pub bounds_sane_count: usize,
    pub logical_bounds_samples: Vec<WindowsNativeOcrBoundsSummary>,
    pub sanitized_text_samples: Vec<String>,
    pub pii_marker_count: usize,
    pub privacy_status: GuiBenchmarkPrivacyStatus,
    pub failure: Option<WindowsNativeOcrFailureSummary>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct WindowsNativeOcrBoundsSummary {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WindowsNativeOcrFailureSummary {
    pub class: WindowsNativeOcrFailureClass,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WindowsNativeOcrFailureClass {
    WindowsOcrCapabilityMissing,
    ImageDecodeOrFormatFailure,
    RecognitionFailure,
    RuntimeJoinFailure,
    IntegrationDefect,
}

pub fn summarize_windows_native_ocr_sample(
    input: WindowsNativeOcrSampleInput<'_>,
    result: Result<&[OcrResult], &CoreError>,
    pii_filter_level: PiiFilterLevel,
) -> WindowsNativeOcrSampleSummary {
    match result {
        Ok(results) => summarize_successful_sample(&input, results, pii_filter_level),
        Err(error) => summarize_failed_sample(&input, error, pii_filter_level),
    }
}

pub fn classify_windows_native_ocr_failure(error: &CoreError) -> WindowsNativeOcrFailureClass {
    let message = error.to_string().to_ascii_lowercase();

    if message.contains("ocrengine creation failed")
        || message.contains("trycreatefromuserprofilelanguages")
        || message.contains("language pack")
        || message.contains("language unavailable")
    {
        return WindowsNativeOcrFailureClass::WindowsOcrCapabilityMissing;
    }

    if message.contains("bitmapdecoder")
        || message.contains("getsoftwarebitmap")
        || message.contains("stream creation")
        || message.contains("datawriter")
        || message.contains("writebytes")
        || message.contains("storeasync")
        || message.contains("flushasync")
        || message.contains("detachstream")
        || message.contains("seek failed")
    {
        return WindowsNativeOcrFailureClass::ImageDecodeOrFormatFailure;
    }

    if message.contains("recognizeasync")
        || message.contains("lines failed")
        || message.contains("words failed")
        || message.contains("text failed")
        || message.contains("boundingrect")
    {
        return WindowsNativeOcrFailureClass::RecognitionFailure;
    }

    if message.contains("task join") || message.contains("join error") {
        return WindowsNativeOcrFailureClass::RuntimeJoinFailure;
    }

    WindowsNativeOcrFailureClass::IntegrationDefect
}

fn summarize_successful_sample(
    input: &WindowsNativeOcrSampleInput<'_>,
    results: &[OcrResult],
    pii_filter_level: PiiFilterLevel,
) -> WindowsNativeOcrSampleSummary {
    let scale = normalized_display_scale(input.display_scale);
    let bounds_present_count = results
        .iter()
        .filter(|result| result.width > 0 && result.height > 0)
        .count();
    let bounds_sane_count = results
        .iter()
        .filter(|result| ocr_bounds_sane(result, input.image_width, input.image_height))
        .count();
    let logical_bounds_samples = results
        .iter()
        .filter(|result| result.width > 0 && result.height > 0)
        .take(MAX_TEXT_SAMPLES)
        .map(|result| logical_bounds(result, scale))
        .collect::<Vec<_>>();
    let sanitized_text_samples = results
        .iter()
        .filter_map(|result| sanitized_sample_text(&result.text, pii_filter_level))
        .take(MAX_TEXT_SAMPLES)
        .collect::<Vec<_>>();
    let pii_marker_count = results
        .iter()
        .map(|result| detect_pii_markers_with_level(&result.text, pii_filter_level).len())
        .sum();

    WindowsNativeOcrSampleSummary {
        ok: !results.is_empty(),
        sample_label: input.sample_label.to_string(),
        image_width: input.image_width,
        image_height: input.image_height,
        image_format: input.image_format.to_ascii_lowercase(),
        display_scale: scale,
        language_assumption: "Windows user profile OCR language packs",
        latency_ms: input.latency_ms,
        extraction_count: results.len(),
        bounds_present_count,
        bounds_sane_count,
        logical_bounds_samples,
        sanitized_text_samples,
        pii_marker_count,
        privacy_status: GuiBenchmarkPrivacyStatus::Redacted,
        failure: None,
    }
}

fn summarize_failed_sample(
    input: &WindowsNativeOcrSampleInput<'_>,
    error: &CoreError,
    pii_filter_level: PiiFilterLevel,
) -> WindowsNativeOcrSampleSummary {
    let sanitized_message = sanitize_title_with_level(&error.to_string(), pii_filter_level);

    WindowsNativeOcrSampleSummary {
        ok: false,
        sample_label: input.sample_label.to_string(),
        image_width: input.image_width,
        image_height: input.image_height,
        image_format: input.image_format.to_ascii_lowercase(),
        display_scale: normalized_display_scale(input.display_scale),
        language_assumption: "Windows user profile OCR language packs",
        latency_ms: input.latency_ms,
        extraction_count: 0,
        bounds_present_count: 0,
        bounds_sane_count: 0,
        logical_bounds_samples: Vec::new(),
        sanitized_text_samples: Vec::new(),
        pii_marker_count: 0,
        privacy_status: GuiBenchmarkPrivacyStatus::Redacted,
        failure: Some(WindowsNativeOcrFailureSummary {
            class: classify_windows_native_ocr_failure(error),
            code: error.code().to_string(),
            message: sanitized_message,
        }),
    }
}

fn sanitized_sample_text(text: &str, pii_filter_level: PiiFilterLevel) -> Option<String> {
    let sanitized = sanitize_title_with_level(text.trim(), pii_filter_level);
    if sanitized.trim().is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

fn logical_bounds(result: &OcrResult, display_scale: f64) -> WindowsNativeOcrBoundsSummary {
    WindowsNativeOcrBoundsSummary {
        x: scaled_i32(result.x, display_scale),
        y: scaled_i32(result.y, display_scale),
        width: scaled_u32(result.width, display_scale),
        height: scaled_u32(result.height, display_scale),
    }
}

fn ocr_bounds_sane(result: &OcrResult, image_width: u32, image_height: u32) -> bool {
    if result.width == 0 || result.height == 0 {
        return false;
    }
    let right = result.x as i64 + result.width as i64;
    let bottom = result.y as i64 + result.height as i64;

    result.x >= 0 && result.y >= 0 && right <= image_width as i64 && bottom <= image_height as i64
}

fn normalized_display_scale(display_scale: f64) -> f64 {
    if display_scale.is_finite() && display_scale > 0.0 {
        display_scale
    } else {
        1.0
    }
}

fn scaled_i32(value: i32, display_scale: f64) -> i32 {
    (value as f64 / display_scale).round() as i32
}

fn scaled_u32(value: u32, display_scale: f64) -> u32 {
    (value as f64 / display_scale).round().max(1.0) as u32
}
