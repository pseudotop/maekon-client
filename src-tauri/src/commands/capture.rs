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
    // effective_permissions() only returns permissions when the status is Valid —
    // Expired/UpdateRequired return all-false, so a stale consent record is also
    // handled fail-closed (Task 3).
    state
        .capture
        .consent_manager
        .as_ref()
        .map(|manager| manager.effective_permissions())
        .unwrap_or_default()
}

/// Own-field gate (#4802): gates the OCR text of a manual capture on the
/// ocr_processing consent.
///
/// If `ocr_permitted` (= `permissions.ocr_processing`) is true, the extracted OCR
/// text is returned as-is; if false, it is discarded as None. This is a consent
/// field separate from image capture (screen_capture), so when only screen_capture
/// is granted, no OCR text leaks.
fn gate_manual_ocr_text(ocr_text: Option<String>, ocr_permitted: bool) -> Option<String> {
    if ocr_permitted {
        ocr_text
    } else {
        None
    }
}

/// Own-field gate (#4802): gates the OCR region text of scene analysis on the
/// ocr_processing consent.
///
/// `analyze_current_scene` returns the frame's `ocr_regions` (per-region extracted
/// text) as-is, and this text also flows into gui_elements labels and the work_type
/// classification sample. Just as `trigger_manual_capture` gates the single OCR text
/// via `gate_manual_ocr_text`, when ocr_processing is not granted all extracted OCR
/// regions are discarded (empty Vec) so that no OCR text leaks when only
/// screen_capture is granted.
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
    // Own-field gate (#4802): extracting/storing/returning OCR text requires the
    // ocr_processing consent. A manual capture allows the image itself via the
    // screen_capture consent, but OCR text needs a separate consent, so it is
    // discarded as None when ocr_processing is not granted (ConsentPermissions).
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

    let (active_app_name, active_window_title, active_app_bundle_id, active_window_bounds) =
        if let Some(ref monitor) = state.capture.activity_monitor {
            match monitor.collect_context().await {
                Ok(ctx) => match ctx.active_window {
                    Some(window) => (
                        window.app_name,
                        window.title,
                        window.app_bundle_id,
                        window.bounds,
                    ),
                    None => ("unknown".to_string(), String::new(), None, None),
                },
                Err(_) => ("unknown".to_string(), String::new(), None, None),
            }
        } else {
            ("unknown".to_string(), String::new(), None, None)
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

    // #6260: AX-tree extraction reads on-screen accessibility content (window /
    // menu / element titles + descriptions), so it must honor the SAME capture
    // privacy gate as its content-capture siblings (analyze_current_scene,
    // trigger_manual_capture): capture_enabled + screen_capture consent +
    // not-paused + schedule/active-hours/power gate. Without it, AX content
    // leaked even when the user had disabled capture, paused it, or withheld
    // screen_capture consent. Return the structured Ok-wrapped failure shape
    // (matching the extractor-unavailable / permission-denied branches) rather
    // than `?`-propagating.
    let permissions = manual_capture_permissions(&state);
    if let Err(gate_err) = manual_capture_privacy_gate(
        &state.config,
        &permissions,
        state.capture_paused.load(Ordering::Relaxed),
        active_window_bounds.as_ref(),
    ) {
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
            error_message: Some(gate_err.message().to_string()),
            timestamp,
        });
    }

    let pii_level = state.config.privacy.pii_filter_level;
    // Reuse the gate-fetched permissions: full_text_extraction is a SEPARATE
    // own-field consent that decides PII escalation inside the extractor (it does
    // not gate access — the composite gate above does).
    let has_consent = permissions.full_text_extraction;

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
                // Own-field gate (#4802): if the ocr_processing consent is absent,
                // discard the OCR region text. Same as the sibling paths
                // trigger_manual_capture / handle_frame_capture. `permissions` was
                // already fetched above (~:696).
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

    /// Own-field gate (#4802): when only screen_capture is granted
    /// (ocr_processing=false), the manual-capture OCR text must be discarded (None).
    #[test]
    fn manual_ocr_not_collected_with_only_screen_capture() {
        // allowed_permissions() is screen_capture:true and ocr_processing defaults to false.
        let perms = allowed_permissions();
        assert!(perms.screen_capture, "composite gate passes");
        assert!(!perms.ocr_processing, "ocr_processing defaults to false");
        let gated =
            gate_manual_ocr_text(Some("user@example.com".to_string()), perms.ocr_processing);
        assert!(
            gated.is_none(),
            "without ocr_processing, manual-capture OCR text is None (no leak)"
        );
    }

    /// Own-field gate (#4802): when ocr_processing is granted, the manual-capture OCR text must be preserved.
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
            "with ocr_processing granted, manual-capture OCR text is preserved"
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

    /// Own-field gate (#4802): when only screen_capture is granted
    /// (ocr_processing=false), the OCR region text of analyze_current_scene must be
    /// discarded (empty Vec).
    #[test]
    fn scene_ocr_not_collected_with_only_screen_capture() {
        let perms = allowed_permissions();
        assert!(perms.screen_capture, "composite gate passes");
        assert!(!perms.ocr_processing, "ocr_processing defaults to false");
        let gated = gate_scene_ocr_regions(sample_ocr_regions(), perms.ocr_processing);
        assert!(
            gated.is_empty(),
            "without ocr_processing, scene OCR regions are an empty Vec (no leak)"
        );
    }

    /// Own-field gate (#4802): when ocr_processing is granted, the OCR regions of analyze_current_scene are preserved.
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
            "with ocr_processing granted, scene OCR regions are preserved"
        );
        assert_eq!(gated[0].text, "user@example.com");
    }
}
