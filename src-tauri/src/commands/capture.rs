use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use oneshim_core::models::frame::ImagePayload;
use oneshim_core::ports::vision::CaptureRequest;
use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

// ── A2: Scene Analysis DTOs ──────────────────────────────────────────

#[derive(Serialize)]
pub struct SceneAnalysisResponse {
    pub app_name: String,
    pub window_title: String,
    pub timestamp: String,
    pub accessibility: Option<AccessibilitySnapshot>,
    pub ocr_regions: Vec<OcrRegionDto>,
    pub gui_elements: Vec<GuiElementDto>,
    pub work_type: Option<String>,
}

#[derive(Serialize)]
pub struct AccessibilitySnapshot {
    pub focused_element: Option<FocusedElementDto>,
    pub element_count: usize,
}

#[derive(Serialize)]
pub struct GuiElementDto {
    pub role: String,
    pub label: Option<String>,
    pub bounds: Option<(i32, i32, u32, u32)>,
    /// Classification confidence for the inferred element type (0.0-1.0).
    pub type_confidence: f32,
}

#[derive(Serialize)]
pub struct FocusedElementDto {
    pub role: String,
    pub label: Option<String>,
    pub extracted_text: Option<String>,
}

#[derive(Serialize)]
pub struct OcrRegionDto {
    pub text: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub confidence: f32,
}

#[derive(Serialize)]
pub struct ManualCaptureResponse {
    pub success: bool,
    pub frame_id: Option<String>,
    pub timestamp: String,
    pub resolution: Option<(u32, u32)>,
    pub ocr_text: Option<String>,
}

#[command]
pub async fn trigger_manual_capture(
    state: tauri::State<'_, AppState>,
) -> Result<ManualCaptureResponse, IpcError> {
    let frame_processor = state
        .capture
        .frame_processor
        .as_ref()
        .ok_or_else(|| IpcError::new("service.unavailable", "Capture not available"))?;

    // Get current window context for CaptureRequest
    let (app_name, window_title) = if let Some(ref monitor) = state.capture.activity_monitor {
        match monitor.collect_context().await {
            Ok(ctx) => match ctx.active_window {
                Some(ref w) => (w.app_name.clone(), w.title.clone()),
                None => ("unknown".to_string(), String::new()),
            },
            Err(_) => ("unknown".to_string(), String::new()),
        }
    } else {
        ("unknown".to_string(), String::new())
    };

    let request = CaptureRequest {
        trigger_type: "manual".to_string(),
        importance: 1.0,
        app_name,
        window_title,
        window_bounds: None,
    };

    let frame = frame_processor
        .capture_and_process(&request)
        .await
        .map_err(IpcError::from)?;

    // Extract image data + OCR text via pattern matching (ImagePayload is an enum).
    // EdgeFrameProcessor encodes with base64::STANDARD — decode with the same engine.
    let (image_bytes, ocr_text) = match &frame.image_payload {
        Some(ImagePayload::Full { data, ocr_text, .. }) => {
            let bytes = BASE64.decode(data).ok();
            (bytes, ocr_text.clone())
        }
        _ => (None, None),
    };

    // Persist frame image if storage available — capture file path for metadata
    let file_path: Option<String> =
        if let (Some(ref fs), Some(ref bytes)) = (&state.capture.frame_storage, &image_bytes) {
            fs.save_frame(frame.metadata.timestamp, bytes)
                .await
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };

    // Persist metadata to SQLite — synchronous method, use block_in_place.
    // Pass file_path so the metadata row links to the saved image file.
    let storage = state.storage.clone();
    let metadata_ref = frame.metadata.clone();
    let ocr_ref = ocr_text.clone();
    let fp_ref = file_path.clone();
    let frame_id = tokio::task::block_in_place(|| {
        storage
            .save_frame_metadata(&metadata_ref, fp_ref.as_deref(), ocr_ref.as_deref())
            .ok()
            .map(|row_id| row_id.to_string())
    });

    // Emit capture feedback flash to overlay
    let ts = frame.metadata.timestamp.to_rfc3339();
    if let Some(ref overlay) = state.magic_overlay {
        overlay.emit_capture_feedback(&ts);
    }

    Ok(ManualCaptureResponse {
        success: true,
        frame_id,
        timestamp: ts,
        resolution: Some(frame.metadata.resolution),
        ocr_text,
    })
}

// ── A2: Scene Analysis Command ───────────────────────────────────────

#[command]
pub async fn analyze_current_scene(
    state: tauri::State<'_, AppState>,
) -> Result<SceneAnalysisResponse, IpcError> {
    // 1. Get current window context
    let monitor =
        state.capture.activity_monitor.as_ref().ok_or_else(|| {
            IpcError::new("service.unavailable", "Activity monitor not available")
        })?;

    let ctx = monitor.collect_context().await.map_err(IpcError::from)?;
    let (app_name, window_title) = match ctx.active_window {
        Some(ref w) => (w.app_name.clone(), w.title.clone()),
        None => {
            return Ok(SceneAnalysisResponse {
                app_name: "unknown".to_string(),
                window_title: String::new(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                accessibility: None,
                ocr_regions: Vec::new(),
                gui_elements: Vec::new(),
                work_type: None,
            });
        }
    };

    // 2. Accessibility extraction (optional)
    let accessibility = if let Some(ref extractor) = state.capture.accessibility_extractor {
        let pii_level = state.config.privacy.pii_filter_level;
        let has_consent = state
            .capture
            .consent_manager
            .as_ref()
            .map(|cm| cm.is_permitted(|p| p.full_text_extraction))
            .unwrap_or(false);
        match extractor
            .extract_focused_element(pii_level, has_consent)
            .await
        {
            Ok(Some(elem)) => Some(AccessibilitySnapshot {
                focused_element: Some(FocusedElementDto {
                    role: elem.role.clone(),
                    label: elem.label.clone(),
                    extracted_text: elem.extracted_text.clone(),
                }),
                element_count: 1,
            }),
            Ok(None) => Some(AccessibilitySnapshot {
                focused_element: None,
                element_count: 0,
            }),
            Err(_) => None,
        }
    } else {
        None
    };

    // 3. Capture frame for OCR regions
    let ocr_regions = if let Some(ref fp) = state.capture.frame_processor {
        let request = CaptureRequest {
            trigger_type: "scene_analysis".to_string(),
            importance: 0.8,
            app_name: app_name.clone(),
            window_title: window_title.clone(),
            window_bounds: None,
        };
        match fp.capture_and_process(&request).await {
            Ok(frame) => frame
                .ocr_regions
                .into_iter()
                .map(|r| OcrRegionDto {
                    text: r.text,
                    x: r.bbox.x,
                    y: r.bbox.y,
                    width: r.bbox.width,
                    height: r.bbox.height,
                    confidence: r.confidence,
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // GUI elements from OCR regions via GuiElementDetector
    let gui_elements: Vec<GuiElementDto> = if !ocr_regions.is_empty() {
        let resolution = (1920_u32, 1080_u32);
        let pii_level = state.config.privacy.pii_filter_level;
        let detector = oneshim_vision::gui_detector::GuiElementDetector::new(resolution, pii_level);

        ocr_regions
            .iter()
            .map(|r| {
                let bbox = oneshim_core::models::frame::BoundingBox {
                    x: r.x,
                    y: r.y,
                    width: r.width,
                    height: r.height,
                };
                let (element_type, type_confidence) =
                    detector.infer_element_type_scored(&r.text, &bbox);
                GuiElementDto {
                    role: format!("{element_type:?}"),
                    label: Some(r.text.clone()),
                    bounds: Some((r.x as i32, r.y as i32, r.width, r.height)),
                    type_confidence,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    // Work type classification
    let work_type = state.capture.work_classifier.as_ref().map(|clf| {
        let focused_role = accessibility
            .as_ref()
            .and_then(|a| a.focused_element.as_ref())
            .map(|f| f.role.as_str());
        let ocr_sample = ocr_regions.first().map(|r| r.text.as_str());
        format!(
            "{:?}",
            clf.classify(&app_name, &window_title, focused_role, ocr_sample)
        )
    });

    Ok(SceneAnalysisResponse {
        app_name,
        window_title,
        timestamp: chrono::Utc::now().to_rfc3339(),
        accessibility,
        ocr_regions,
        gui_elements,
        work_type,
    })
}
