use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use maekon_core::config::AppConfig;
use maekon_core::consent::ConsentPermissions;
use maekon_core::error::CoreError;
use maekon_core::models::context::WindowBounds;
use maekon_core::models::focused_element::{AccessibilityElement, ElementRect};
use maekon_core::models::frame::ImagePayload;
use maekon_core::ports::vision::CaptureRequest;
use serde::Serialize;
use std::sync::atomic::Ordering;
#[cfg(target_os = "macos")]
use std::sync::OnceLock;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

#[cfg(target_os = "macos")]
static AX_FOCUS_OBSERVER: OnceLock<
    parking_lot::Mutex<Option<maekon_vision::accessibility::FocusObserverHandle>>,
> = OnceLock::new();

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

#[derive(Serialize)]
pub struct AccessibilityTreeSnapshotResponse {
    pub ok: bool,
    pub requested_app_name: Option<String>,
    pub active_app_name: String,
    pub active_window_title: String,
    pub active_app_bundle_id: Option<String>,
    pub matches_requested_app: bool,
    pub permission_granted: bool,
    pub extractor_name: Option<String>,
    pub max_depth: u32,
    pub max_elements: usize,
    pub element_count: usize,
    pub elements: Vec<AccessibilityTreeElementDto>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub timestamp: String,
}

#[derive(Serialize)]
pub struct AxFocusObserverResponse {
    pub ok: bool,
    pub requested_app_name: Option<String>,
    pub observed_pid: Option<u32>,
    pub running: bool,
    pub focus_changed: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub timestamp: String,
}

#[derive(Serialize)]
pub struct AccessibilityTreeElementDto {
    pub role: String,
    pub label: Option<String>,
    pub bounds: Option<ElementRectDto>,
}

#[derive(Serialize)]
pub struct ElementRectDto {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl From<ElementRect> for ElementRectDto {
    fn from(rect: ElementRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

impl From<AccessibilityElement> for AccessibilityTreeElementDto {
    fn from(element: AccessibilityElement) -> Self {
        Self {
            role: element.role,
            label: non_empty_label(element.label),
            bounds: element.bounds.map(ElementRectDto::from),
        }
    }
}

fn non_empty_label(label: String) -> Option<String> {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn requested_app_matches(
    requested_app_name: Option<&str>,
    active_app_name: &str,
    active_app_bundle_id: Option<&str>,
) -> bool {
    let Some(requested) = requested_app_name else {
        return true;
    };
    let requested = requested.trim();
    if requested.is_empty() {
        return true;
    }

    active_app_name.eq_ignore_ascii_case(requested)
        || active_app_bundle_id
            .map(|bundle_id| bundle_id.eq_ignore_ascii_case(requested))
            .unwrap_or(false)
}

fn core_error_parts(error: CoreError) -> (String, String) {
    let code = error.code().to_string();
    let message = error.to_string();
    (code, message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManualCaptureGateError {
    WindowBoundsRequired,
    CaptureDisabled,
    ScreenCaptureConsentMissing,
    CapturePaused,
    PrivacyGateBlocked,
}

impl ManualCaptureGateError {
    fn message(self) -> &'static str {
        match self {
            Self::WindowBoundsRequired => {
                "Manual capture requires focused-window bounds; primary-monitor fallback is blocked"
            }
            Self::CaptureDisabled => "Manual capture blocked because screen capture is disabled",
            Self::ScreenCaptureConsentMissing => {
                "Manual capture blocked because screen capture consent is missing"
            }
            Self::CapturePaused => "Manual capture blocked because capture is paused",
            Self::PrivacyGateBlocked => {
                "Manual capture blocked by active-hours, tracking-schedule, or power privacy gate"
            }
        }
    }
}

impl From<ManualCaptureGateError> for IpcError {
    fn from(error: ManualCaptureGateError) -> Self {
        IpcError::new("permission.permission_denied", error.message())
    }
}

fn manual_capture_permissions(state: &AppState) -> ConsentPermissions {
    // effective_permissions()은 Valid 상태일 때만 권한을 반환한다 — Expired/UpdateRequired는
    // all-false를 반환하므로 스테일 동의 레코드도 fail-closed 처리된다 (Task 3).
    state
        .capture
        .consent_manager
        .as_ref()
        .map(|manager| manager.effective_permissions())
        .unwrap_or_default()
}

/// Own-field gate (#4802): 수동 캡처의 OCR 텍스트를 ocr_processing 동의 여부로 게이트한다.
///
/// `ocr_permitted` (= `permissions.ocr_processing`) 가 true 이면 추출된 OCR 텍스트를
/// 그대로 반환하고, false 이면 None 으로 폐기한다. 이미지 캡처(screen_capture)와는
/// 별도의 동의 필드이므로, screen_capture 만 부여된 상태에서는 OCR 텍스트가 새지 않는다.
fn gate_manual_ocr_text(ocr_text: Option<String>, ocr_permitted: bool) -> Option<String> {
    if ocr_permitted {
        ocr_text
    } else {
        None
    }
}

/// Own-field gate (#4802): scene 분석의 OCR region 텍스트를 ocr_processing 동의로 게이트한다.
///
/// `analyze_current_scene` 은 frame 의 `ocr_regions`(영역별 추출 텍스트)를 그대로
/// 반환하며, 이 텍스트는 gui_elements 라벨과 work_type 분류 샘플로도 흘러간다.
/// `trigger_manual_capture` 가 `gate_manual_ocr_text` 로 단일 OCR 텍스트를 게이트하는
/// 것과 동일하게, ocr_processing 미부여 시에는 추출된 모든 OCR region 을 폐기(빈 Vec)
/// 하여 screen_capture 만 부여된 상태에서 OCR 텍스트가 새지 않도록 한다.
fn gate_scene_ocr_regions(regions: Vec<OcrRegionDto>, ocr_permitted: bool) -> Vec<OcrRegionDto> {
    if ocr_permitted {
        regions
    } else {
        Vec::new()
    }
}

fn manual_capture_privacy_gate(
    config: &AppConfig,
    permissions: &ConsentPermissions,
    capture_paused: bool,
    window_bounds: Option<&WindowBounds>,
) -> Result<(), ManualCaptureGateError> {
    if window_bounds.is_none() {
        return Err(ManualCaptureGateError::WindowBoundsRequired);
    }
    if !config.vision.capture_enabled {
        return Err(ManualCaptureGateError::CaptureDisabled);
    }
    if !permissions.screen_capture {
        return Err(ManualCaptureGateError::ScreenCaptureConsentMissing);
    }
    if capture_paused {
        return Err(ManualCaptureGateError::CapturePaused);
    }

    if crate::scheduler::capture_permitted_now(config, permissions, capture_paused) {
        Ok(())
    } else {
        Err(ManualCaptureGateError::PrivacyGateBlocked)
    }
}

#[cfg(target_os = "macos")]
fn ax_focus_observer_slot(
) -> &'static parking_lot::Mutex<Option<maekon_vision::accessibility::FocusObserverHandle>> {
    AX_FOCUS_OBSERVER.get_or_init(|| parking_lot::Mutex::new(None))
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
    let (app_name, window_title, window_bounds, app_bundle_id) =
        if let Some(ref monitor) = state.capture.activity_monitor {
            match monitor.collect_context().await {
                Ok(ctx) => match ctx.active_window {
                    Some(ref w) => (
                        w.app_name.clone(),
                        w.title.clone(),
                        w.bounds,
                        w.app_bundle_id.clone(),
                    ),
                    None => ("unknown".to_string(), String::new(), None, None),
                },
                Err(_) => ("unknown".to_string(), String::new(), None, None),
            }
        } else {
            ("unknown".to_string(), String::new(), None, None)
        };
    let permissions = manual_capture_permissions(&state);
    manual_capture_privacy_gate(
        &state.config,
        &permissions,
        state.capture_paused.load(Ordering::Relaxed),
        window_bounds.as_ref(),
    )?;

    let request = CaptureRequest {
        trigger_type: "manual".to_string(),
        importance: 1.0,
        app_name,
        window_title,
        monitor_id: None,
        app_bundle_id,
        window_bounds,
        screen_scale_factor: None,
    };

    let frame = frame_processor
        .capture_and_process(&request)
        .await
        .map_err(IpcError::from)?;

    // Extract image data + OCR text via pattern matching (ImagePayload is an enum).
    // EdgeFrameProcessor encodes with base64::STANDARD — decode with the same engine.
    // Own-field gate (#4802): OCR 텍스트 추출/저장/반환은 ocr_processing 동의가 있어야 한다.
    // 수동 캡처는 screen_capture 동의로 이미지 자체는 허용되지만, OCR 텍스트는 별도
    // 동의가 필요하므로 ocr_processing 미부여 시 None 으로 폐기한다 (ConsentPermissions).
    let (image_bytes, ocr_text) = match &frame.image_payload {
        Some(ImagePayload::Full { data, ocr_text, .. }) => {
            let bytes = BASE64.decode(data).ok();
            let ocr = gate_manual_ocr_text(ocr_text.clone(), permissions.ocr_processing);
            (bytes, ocr)
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
pub async fn extract_ax_tree(
    state: tauri::State<'_, AppState>,
    app_name: Option<String>,
    max_depth: Option<u32>,
    max_elements: Option<usize>,
) -> Result<AccessibilityTreeSnapshotResponse, IpcError> {
    let max_depth = max_depth.unwrap_or(4).min(8);
    let max_elements = max_elements.unwrap_or(300).clamp(1, 1_000);

    let (active_app_name, active_window_title, active_app_bundle_id) =
        if let Some(ref monitor) = state.capture.activity_monitor {
            match monitor.collect_context().await {
                Ok(ctx) => match ctx.active_window {
                    Some(window) => (window.app_name, window.title, window.app_bundle_id),
                    None => ("unknown".to_string(), String::new(), None),
                },
                Err(_) => ("unknown".to_string(), String::new(), None),
            }
        } else {
            ("unknown".to_string(), String::new(), None)
        };

    let matches_requested_app = requested_app_matches(
        app_name.as_deref(),
        &active_app_name,
        active_app_bundle_id.as_deref(),
    );

    let timestamp = chrono::Utc::now().to_rfc3339();
    let Some(ref extractor) = state.capture.accessibility_extractor else {
        return Ok(AccessibilityTreeSnapshotResponse {
            ok: false,
            requested_app_name: app_name,
            active_app_name,
            active_window_title,
            active_app_bundle_id,
            matches_requested_app,
            permission_granted: false,
            extractor_name: None,
            max_depth,
            max_elements,
            element_count: 0,
            elements: Vec::new(),
            error_code: Some("service.unavailable".to_string()),
            error_message: Some("Accessibility extractor not available".to_string()),
            timestamp,
        });
    };

    let extractor_name = Some(extractor.name().to_string());
    let permission_granted = extractor.has_permission();
    if !permission_granted {
        return Ok(AccessibilityTreeSnapshotResponse {
            ok: false,
            requested_app_name: app_name,
            active_app_name,
            active_window_title,
            active_app_bundle_id,
            matches_requested_app,
            permission_granted,
            extractor_name,
            max_depth,
            max_elements,
            element_count: 0,
            elements: Vec::new(),
            error_code: Some("permission.permission_denied".to_string()),
            error_message: Some(
                "Accessibility permission is not granted for AX tree extraction".to_string(),
            ),
            timestamp,
        });
    }

    let pii_level = state.config.privacy.pii_filter_level;
    let has_consent = state
        .capture
        .consent_manager
        .as_ref()
        .map(|cm| cm.effective_permissions().full_text_extraction)
        .unwrap_or(false);

    let extraction_result = if let Some(ref requested_app_name) = app_name {
        if requested_app_name.trim().is_empty() {
            extractor
                .extract_window_elements(max_depth, max_elements, pii_level, has_consent)
                .await
        } else {
            extractor
                .extract_application_elements(
                    requested_app_name,
                    max_depth,
                    max_elements,
                    pii_level,
                    has_consent,
                )
                .await
        }
    } else {
        extractor
            .extract_window_elements(max_depth, max_elements, pii_level, has_consent)
            .await
    };

    match extraction_result {
        Ok(elements) => {
            let elements: Vec<AccessibilityTreeElementDto> = elements
                .into_iter()
                .map(AccessibilityTreeElementDto::from)
                .collect();
            Ok(AccessibilityTreeSnapshotResponse {
                ok: true,
                requested_app_name: app_name,
                active_app_name,
                active_window_title,
                active_app_bundle_id,
                matches_requested_app,
                permission_granted,
                extractor_name,
                max_depth,
                max_elements,
                element_count: elements.len(),
                elements,
                error_code: None,
                error_message: None,
                timestamp,
            })
        }
        Err(error) => {
            let (error_code, error_message) = core_error_parts(error);
            Ok(AccessibilityTreeSnapshotResponse {
                ok: false,
                requested_app_name: app_name,
                active_app_name,
                active_window_title,
                active_app_bundle_id,
                matches_requested_app,
                permission_granted,
                extractor_name,
                max_depth,
                max_elements,
                element_count: 0,
                elements: Vec::new(),
                error_code: Some(error_code),
                error_message: Some(error_message),
                timestamp,
            })
        }
    }
}

#[command]
pub async fn start_ax_focus_observer(
    app_name: Option<String>,
) -> Result<AxFocusObserverResponse, IpcError> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    #[cfg(target_os = "macos")]
    {
        use maekon_core::ports::accessibility::AccessibilityExtractor;

        let extractor = maekon_vision::accessibility::MacOsNativeAccessibility::new();
        if !extractor.has_permission() {
            return Ok(AxFocusObserverResponse {
                ok: false,
                requested_app_name: app_name,
                observed_pid: None,
                running: false,
                focus_changed: false,
                error_code: Some("permission.permission_denied".to_string()),
                error_message: Some(
                    "Accessibility permission is not granted for AX focus observer".to_string(),
                ),
                timestamp,
            });
        }

        let pid = std::process::id() as i32;
        let Some(handle) = maekon_vision::accessibility::FocusObserverHandle::start(pid) else {
            return Ok(AxFocusObserverResponse {
                ok: false,
                requested_app_name: app_name,
                observed_pid: Some(pid as u32),
                running: false,
                focus_changed: false,
                error_code: Some("accessibility.observer_unavailable".to_string()),
                error_message: Some("AX focus observer could not be started".to_string()),
                timestamp,
            });
        };

        let observed_pid = handle.observed_pid() as u32;
        let mut slot = ax_focus_observer_slot().lock();
        if let Some(mut old_handle) = slot.take() {
            old_handle.stop();
        }
        *slot = Some(handle);

        Ok(AxFocusObserverResponse {
            ok: true,
            requested_app_name: app_name,
            observed_pid: Some(observed_pid),
            running: true,
            focus_changed: false,
            error_code: None,
            error_message: None,
            timestamp,
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(AxFocusObserverResponse {
            ok: false,
            requested_app_name: app_name,
            observed_pid: None,
            running: false,
            focus_changed: false,
            error_code: Some("platform.unsupported".to_string()),
            error_message: Some("AX focus observer debug IPC is macOS-only".to_string()),
            timestamp,
        })
    }
}

#[command]
pub async fn poll_ax_focus_observer() -> Result<AxFocusObserverResponse, IpcError> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    #[cfg(target_os = "macos")]
    {
        let slot = ax_focus_observer_slot().lock();
        if let Some(handle) = slot.as_ref() {
            let observed_pid = handle.observed_pid() as u32;
            let focus_changed = handle.has_focus_changed();
            return Ok(AxFocusObserverResponse {
                ok: true,
                requested_app_name: None,
                observed_pid: Some(observed_pid),
                running: true,
                focus_changed,
                error_code: None,
                error_message: None,
                timestamp,
            });
        }
    }

    Ok(AxFocusObserverResponse {
        ok: false,
        requested_app_name: None,
        observed_pid: None,
        running: false,
        focus_changed: false,
        error_code: Some("accessibility.observer_not_running".to_string()),
        error_message: Some("AX focus observer is not running".to_string()),
        timestamp,
    })
}

#[command]
pub async fn stop_ax_focus_observer() -> Result<AxFocusObserverResponse, IpcError> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    #[cfg(target_os = "macos")]
    {
        let mut slot = ax_focus_observer_slot().lock();
        if let Some(mut handle) = slot.take() {
            let observed_pid = handle.observed_pid() as u32;
            let focus_changed = handle.has_focus_changed();
            handle.stop();
            return Ok(AxFocusObserverResponse {
                ok: true,
                requested_app_name: None,
                observed_pid: Some(observed_pid),
                running: false,
                focus_changed,
                error_code: None,
                error_message: None,
                timestamp,
            });
        }
    }

    Ok(AxFocusObserverResponse {
        ok: false,
        requested_app_name: None,
        observed_pid: None,
        running: false,
        focus_changed: false,
        error_code: Some("accessibility.observer_not_running".to_string()),
        error_message: Some("AX focus observer is not running".to_string()),
        timestamp,
    })
}

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
    let (app_name, window_title, window_bounds, app_bundle_id) = match ctx.active_window {
        Some(ref w) => (
            w.app_name.clone(),
            w.title.clone(),
            w.bounds,
            w.app_bundle_id.clone(),
        ),
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
    let permissions = manual_capture_permissions(&state);
    manual_capture_privacy_gate(
        &state.config,
        &permissions,
        state.capture_paused.load(Ordering::Relaxed),
        window_bounds.as_ref(),
    )?;

    // 2. Accessibility extraction (optional)
    let accessibility = if let Some(ref extractor) = state.capture.accessibility_extractor {
        let pii_level = state.config.privacy.pii_filter_level;
        let has_consent = state
            .capture
            .consent_manager
            .as_ref()
            .map(|cm| cm.effective_permissions().full_text_extraction)
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
            monitor_id: None,
            app_bundle_id,
            window_bounds,
            screen_scale_factor: None,
        };
        match fp.capture_and_process(&request).await {
            Ok(frame) => {
                let regions: Vec<OcrRegionDto> = frame
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
                    .collect();
                // Own-field gate (#4802): ocr_processing 동의가 없으면 OCR region 텍스트를
                // 폐기한다. trigger_manual_capture / handle_frame_capture 형제 경로와 동일.
                // permissions 는 위(~:696)에서 이미 fetch 됨.
                gate_scene_ocr_regions(regions, permissions.ocr_processing)
            }
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // GUI elements from OCR regions via GuiElementDetector
    let gui_elements: Vec<GuiElementDto> = if !ocr_regions.is_empty() {
        let resolution = (1920_u32, 1080_u32);
        let pii_level = state.config.privacy.pii_filter_level;
        let detector = maekon_vision::gui_detector::GuiElementDetector::new(resolution, pii_level);

        ocr_regions
            .iter()
            .map(|r| {
                let bbox = maekon_core::models::frame::BoundingBox {
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

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::AppConfig;
    use maekon_core::consent::ConsentPermissions;
    use maekon_core::models::context::WindowBounds;

    fn allowed_permissions() -> ConsentPermissions {
        ConsentPermissions {
            screen_capture: true,
            ..ConsentPermissions::default()
        }
    }

    fn bounds() -> WindowBounds {
        WindowBounds {
            x: 10,
            y: 20,
            width: 640,
            height: 480,
        }
    }

    #[test]
    fn manual_capture_gate_blocks_when_capture_disabled() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = false;

        let result =
            manual_capture_privacy_gate(&config, &allowed_permissions(), false, Some(&bounds()));

        assert_eq!(result, Err(ManualCaptureGateError::CaptureDisabled));
    }

    #[test]
    fn manual_capture_gate_blocks_without_screen_capture_consent() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;

        let result = manual_capture_privacy_gate(
            &config,
            &ConsentPermissions::default(),
            false,
            Some(&bounds()),
        );

        assert_eq!(
            result,
            Err(ManualCaptureGateError::ScreenCaptureConsentMissing)
        );
    }

    #[test]
    fn manual_capture_gate_blocks_when_capture_is_paused() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;

        let result =
            manual_capture_privacy_gate(&config, &allowed_permissions(), true, Some(&bounds()));

        assert_eq!(result, Err(ManualCaptureGateError::CapturePaused));
    }

    #[test]
    fn manual_capture_gate_requires_window_bounds() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;

        let result = manual_capture_privacy_gate(&config, &allowed_permissions(), false, None);

        assert_eq!(result, Err(ManualCaptureGateError::WindowBoundsRequired));
    }

    #[test]
    fn manual_capture_gate_allows_when_scheduled_capture_gate_allows() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;

        let result =
            manual_capture_privacy_gate(&config, &allowed_permissions(), false, Some(&bounds()));

        assert_eq!(result, Ok(()));
    }

    /// Own-field gate (#4802): screen_capture 만 부여(ocr_processing=false)된 상태에서
    /// 수동 캡처의 OCR 텍스트는 폐기되어야 한다 (None).
    #[test]
    fn manual_ocr_not_collected_with_only_screen_capture() {
        // allowed_permissions() 는 screen_capture:true 이고 ocr_processing 은 기본 false.
        let perms = allowed_permissions();
        assert!(perms.screen_capture, "복합 게이트는 통과");
        assert!(!perms.ocr_processing, "ocr_processing 은 기본 false");
        let gated =
            gate_manual_ocr_text(Some("user@example.com".to_string()), perms.ocr_processing);
        assert!(
            gated.is_none(),
            "ocr_processing 미부여 시 수동 캡처 OCR 텍스트는 None (누출 없음)"
        );
    }

    /// Own-field gate (#4802): ocr_processing 부여 시 수동 캡처 OCR 텍스트가 보존되어야 한다.
    #[test]
    fn manual_ocr_collected_when_own_field_granted() {
        let perms = ConsentPermissions {
            screen_capture: true,
            ocr_processing: true,
            ..ConsentPermissions::default()
        };
        let gated = gate_manual_ocr_text(Some("agenda 2026".to_string()), perms.ocr_processing);
        assert_eq!(
            gated.as_deref(),
            Some("agenda 2026"),
            "ocr_processing 부여 시 수동 캡처 OCR 텍스트 보존"
        );
    }

    fn sample_ocr_regions() -> Vec<OcrRegionDto> {
        vec![OcrRegionDto {
            text: "user@example.com".to_string(),
            x: 1,
            y: 2,
            width: 100,
            height: 20,
            confidence: 0.9,
        }]
    }

    /// Own-field gate (#4802): screen_capture 만 부여(ocr_processing=false)된 상태에서
    /// analyze_current_scene 의 OCR region 텍스트는 폐기되어야 한다 (빈 Vec).
    #[test]
    fn scene_ocr_not_collected_with_only_screen_capture() {
        let perms = allowed_permissions();
        assert!(perms.screen_capture, "복합 게이트는 통과");
        assert!(!perms.ocr_processing, "ocr_processing 은 기본 false");
        let gated = gate_scene_ocr_regions(sample_ocr_regions(), perms.ocr_processing);
        assert!(
            gated.is_empty(),
            "ocr_processing 미부여 시 scene OCR region 은 빈 Vec (누출 없음)"
        );
    }

    /// Own-field gate (#4802): ocr_processing 부여 시 analyze_current_scene 의 OCR region 보존.
    #[test]
    fn scene_ocr_collected_when_own_field_granted() {
        let perms = ConsentPermissions {
            screen_capture: true,
            ocr_processing: true,
            ..ConsentPermissions::default()
        };
        let gated = gate_scene_ocr_regions(sample_ocr_regions(), perms.ocr_processing);
        assert_eq!(
            gated.len(),
            1,
            "ocr_processing 부여 시 scene OCR region 보존"
        );
        assert_eq!(gated[0].text, "user@example.com");
    }
}
