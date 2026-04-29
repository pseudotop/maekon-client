use oneshim_core::config::OverlayMode;
use oneshim_core::models::coaching::{CoachingMessage, DismissAction, GoalProgressView};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

const OVERLAY_LABEL: &str = "magic-overlay";
const OVERLAY_URL: &str = "overlay.html";

/// Serializable payload emitted to the overlay WebView via Tauri events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayCoachingPayload {
    pub message_id: String,
    pub profile: String,
    pub trigger_type: String,
    pub text: String,
    pub auto_dismiss_secs: u64,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayUpgradePayload {
    pub message_id: String,
    pub personalized_text: String,
}

#[allow(dead_code)] // retained for future IPC command usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayFocusPayload {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub border_color: String,
    pub opacity: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayGoalPayload {
    pub goals: Vec<GoalProgressView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverlayModePayload {
    pub mode: OverlayMode,
}

#[allow(dead_code)] // used by detection overlay IPC commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionElementPayload {
    pub element_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub label: String,
    pub role: Option<String>,
    pub confidence: f64,
    pub source: String,
}

#[allow(dead_code)] // used by detection overlay IPC commands
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionScenePayload {
    pub scene_id: String,
    pub app_name: Option<String>,
    pub screen_width: u32,
    pub screen_height: u32,
    pub element_count: usize,
    pub elements: Vec<DetectionElementPayload>,
}

#[derive(Debug)]
struct OverlayState {
    mode: OverlayMode,
    visible: bool,
    current_message_id: Option<String>,
    detection_active: bool,
    suggestions_panel_open: bool,
    automation_confirm_active: bool,
}

/// Handle for managing the MagicOverlay Tauri WebView window.
///
/// Created during app setup. The overlay window is created and shown at
/// startup so persistent components (TrackingBorder, CaptureFlash) render
/// immediately. The window is transparent and click-through by default.
///
/// # Note: CoachingOverlayPort consideration
///
/// This struct is **not** behind a port trait. It depends on `tauri::AppHandle`
/// which is only available in the binary crate (`src-tauri`), making it
/// unsuitable for the `oneshim-core` port layer.
///
/// A `CoachingOverlayPort` trait could be introduced for unit testing the
/// scheduler coaching loop, but the notification fallback already provides
/// sufficient test coverage for the coaching output path.
#[derive(Clone)]
pub struct MagicOverlayHandle {
    app_handle: AppHandle,
    state: Arc<RwLock<OverlayState>>,
}

impl MagicOverlayHandle {
    pub fn new(app_handle: AppHandle, initial_mode: OverlayMode) -> Self {
        Self {
            app_handle,
            state: Arc::new(RwLock::new(OverlayState {
                mode: initial_mode,
                visible: false,
                current_message_id: None,
                detection_active: false,
                suggestions_panel_open: false,
                automation_confirm_active: false,
            })),
        }
    }

    /// Create the overlay window if it does not yet exist.
    ///
    /// Gracefully degrades on Linux/Wayland (overlay not supported).
    /// macOS requires `macos-private-api` feature flag for transparent windows.
    /// Windows requires `shadow: false` to avoid rendering artifacts.
    pub fn ensure_window(&self) -> Result<(), String> {
        // Wayland graceful degradation: skip overlay creation entirely
        #[cfg(target_os = "linux")]
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            warn!("Wayland detected — MagicOverlay disabled, using notification fallback");
            return Err("Wayland does not support transparent overlay windows".to_string());
        }

        // Check if window already exists
        if self.app_handle.get_webview_window(OVERLAY_LABEL).is_some() {
            return Ok(());
        }

        let monitor = self
            .app_handle
            .primary_monitor()
            .map_err(|e| format!("Failed to get primary monitor: {e}"))?
            .ok_or_else(|| "No primary monitor found".to_string())?;

        let size = monitor.size();

        let builder = WebviewWindowBuilder::new(
            &self.app_handle,
            OVERLAY_LABEL,
            WebviewUrl::App(OVERLAY_URL.into()),
        )
        .title("Maekon Overlay")
        .inner_size(size.width as f64, size.height as f64)
        .position(0.0, 0.0)
        .transparent(true)
        .always_on_top(true)
        .decorations(false)
        .resizable(false)
        .visible(false)
        .skip_taskbar(true)
        // Windows: shadow must be false for transparent windows to render correctly
        .shadow(false);

        let window = builder
            .build()
            .map_err(|e| format!("Failed to create overlay window: {e}"))?;

        // Default: click-through. User presses Cmd+Shift+O to make interactive.
        if let Err(e) = window.set_ignore_cursor_events(true) {
            debug!("set_ignore_cursor_events failed: {e}");
        }

        info!("MagicOverlay window created");
        Ok(())
    }

    /// Show a coaching message on the overlay.
    ///
    /// Creates the overlay window if needed, emits the event, and sets
    /// the window to visible.
    pub async fn show_coaching(&self, message: &CoachingMessage) {
        if let Err(e) = self.ensure_window() {
            debug!("overlay unavailable, skipping show_coaching: {e}");
            return;
        }

        let payload = OverlayCoachingPayload {
            message_id: message.message_id.clone(),
            profile: format!("{:?}", message.profile),
            trigger_type: oneshim_core::models::coaching::trigger_type_name(&message.trigger),
            text: message.display_text().to_string(),
            auto_dismiss_secs: 15,
            explanation: message.explanation.clone(),
        };

        if let Err(e) = self.app_handle.emit("overlay:show-coaching", &payload) {
            warn!("failed to emit overlay:show-coaching: {e}");
            return;
        }

        // Show the window
        if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
            if let Err(e) = window.show() {
                debug!("window show failed: {e}");
            }
        }

        let mut state = self.state.write().await;
        state.visible = true;
        state.current_message_id = Some(message.message_id.clone());
    }

    /// Upgrade the coaching message text with LLM-personalized content.
    ///
    /// Only emits if the current message matches and the window is still visible.
    pub async fn upgrade_message(&self, message_id: &str, personalized_text: &str) {
        let state = self.state.read().await;
        if !state.visible {
            return;
        }
        if state.current_message_id.as_deref() != Some(message_id) {
            return;
        }
        drop(state);

        let payload = OverlayUpgradePayload {
            message_id: message_id.to_string(),
            personalized_text: personalized_text.to_string(),
        };

        if let Err(e) = self.app_handle.emit("overlay:upgrade-message", &payload) {
            warn!("failed to emit overlay:upgrade-message: {e}");
        }
    }

    /// Dismiss a coaching message from the overlay.
    ///
    /// Clears the current message ID. The window stays visible for persistent
    /// components (TrackingBorder, CaptureFlash). The React layer handles
    /// hiding the coaching popup via the 'dismiss' event.
    pub async fn dismiss(&self, message_id: &str, _action: DismissAction) {
        let mut state = self.state.write().await;
        if state.current_message_id.as_deref() == Some(message_id) {
            state.current_message_id = None;
        }
        state.visible = false;
        drop(state);

        // Window stays visible — persistent components need it.
        // The React dismiss reducer clears the coaching popup from the DOM.

        if let Err(e) = self.app_handle.emit("overlay:dismiss", message_id) {
            warn!("failed to emit overlay:dismiss: {e}");
        }
    }

    /// Update focus highlight overlay element.
    #[allow(dead_code)] // retained for future IPC command usage
    pub fn update_focus_highlight(&self, highlight: OverlayFocusPayload) {
        if let Err(e) = self.app_handle.emit("overlay:update-focus", &highlight) {
            warn!("failed to emit overlay:update-focus: {e}");
        }
    }

    /// Clear the focus highlight from the overlay.
    #[allow(dead_code)] // retained for future IPC command usage
    pub fn clear_focus_highlight(&self) {
        if let Err(e) = self.app_handle.emit("overlay:clear-focus", ()) {
            debug!("emit overlay:clear-focus failed: {e}");
        }
    }

    /// Emit a full UiScene to the detection overlay. Clears any active
    /// focus highlight first (mutual exclusion).
    #[allow(dead_code)] // Called from scheduler detection loop when accessibility scene is available
    pub async fn emit_detection_scene(&self, scene: &oneshim_core::models::ui_scene::UiScene) {
        self.clear_focus_highlight();

        const DETECTION_ELEMENT_LIMIT: usize = 200;

        let mut elements: Vec<DetectionElementPayload> = scene
            .elements
            .iter()
            .map(|el| DetectionElementPayload {
                element_id: el.element_id.clone(),
                x: el.bbox_abs.x,
                y: el.bbox_abs.y,
                width: el.bbox_abs.width,
                height: el.bbox_abs.height,
                label: el.label.clone(),
                role: el.role.clone(),
                confidence: el.confidence,
                source: "composite".to_string(),
            })
            .collect();

        // Sort highest-confidence elements first so the cap retains the most
        // valuable detections rather than silently dropping them.
        elements.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let total = elements.len();
        if total > DETECTION_ELEMENT_LIMIT {
            warn!(
                scene_id = %scene.scene_id,
                total = total,
                limit = DETECTION_ELEMENT_LIMIT,
                "detection scene truncated — showing top {DETECTION_ELEMENT_LIMIT} of {total} elements by confidence",
            );
            elements.truncate(DETECTION_ELEMENT_LIMIT);
        }

        let payload = DetectionScenePayload {
            scene_id: scene.scene_id.clone(),
            app_name: scene.app_name.clone(),
            screen_width: scene.screen_width,
            screen_height: scene.screen_height,
            element_count: elements.len(),
            elements,
        };

        if let Err(e) = self.ensure_window() {
            debug!("ensure_window failed: {e}");
        }
        if let Err(e) = self.app_handle.emit("overlay:detection-update", &payload) {
            warn!("failed to emit overlay:detection-update: {e}");
        }

        let mut state = self.state.write().await;
        state.detection_active = true;
        info!(
            scene_id = %scene.scene_id,
            element_count = payload.element_count,
            "detection overlay updated"
        );
    }

    /// Clear the detection overlay.
    #[allow(dead_code)] // Called from scheduler detection loop on scene teardown
    pub async fn clear_detection_scene(&self) {
        if let Err(e) = self.app_handle.emit("overlay:detection-clear", ()) {
            debug!("emit overlay:detection-clear failed: {e}");
        }
        let mut state = self.state.write().await;
        state.detection_active = false;
        debug!("detection overlay cleared");
    }

    /// Update goal progress data on the overlay.
    pub fn update_goal_progress(&self, goals: Vec<GoalProgressView>) {
        let payload = OverlayGoalPayload { goals };
        if let Err(e) = self.app_handle.emit("overlay:update-goals", &payload) {
            warn!("failed to emit overlay:update-goals: {e}");
        }
    }

    /// Set the overlay display mode (Minimal/Rich/Adaptive).
    pub async fn set_mode(&self, mode: OverlayMode) {
        let mut state = self.state.write().await;
        state.mode = mode;
        drop(state);

        let payload = OverlayModePayload { mode };
        if let Err(e) = self.app_handle.emit("overlay:set-mode", &payload) {
            warn!("failed to emit overlay:set-mode: {e}");
        }
    }

    /// Get the current overlay display mode.
    pub async fn get_mode(&self) -> OverlayMode {
        self.state.read().await.mode
    }

    /// Check if the overlay window is currently visible.
    pub async fn is_visible(&self) -> bool {
        self.state.read().await.visible
    }

    /// Cycle through overlay modes: Minimal -> Rich -> Adaptive -> Minimal.
    pub async fn toggle_mode(&self) {
        let new_mode = {
            let state = self.state.read().await;
            match state.mode {
                OverlayMode::Minimal => OverlayMode::Rich,
                OverlayMode::Rich => OverlayMode::Adaptive,
                OverlayMode::Adaptive => OverlayMode::Minimal,
            }
        };
        self.set_mode(new_mode).await;
    }

    /// Apply the correct window layout based on active overlay mode priority.
    ///
    /// Priority (highest wins):
    ///   1. Automation Confirm — full-screen interactive (modal backdrop)
    ///   2. Detection — full-screen interactive (inspection mode)
    ///   3. Suggestions Panel — compact right-edge strip interactive
    ///   4. Default — full-screen click-through
    ///
    /// Called after any mode flag changes to ensure the window always matches
    /// the highest-priority active mode.
    fn apply_window_layout(&self, state: &OverlayState) {
        if let Err(e) = self.ensure_window() {
            debug!("ensure_window failed: {e}");
            return;
        }

        let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) else {
            return;
        };

        if state.automation_confirm_active || state.detection_active {
            // Full-screen interactive
            if let Ok(Some(monitor)) = self.app_handle.primary_monitor() {
                let size = monitor.size();
                let _ = window.set_position(tauri::LogicalPosition::new(0.0, 0.0));
                let _ = window.set_size(tauri::PhysicalSize::new(size.width, size.height));
            }
            let _ = window.show();
            let _ = window.set_ignore_cursor_events(false);
            debug!(
                "Overlay layout: full-screen interactive (automation={}, detection={})",
                state.automation_confirm_active, state.detection_active
            );
        } else if state.suggestions_panel_open {
            // Compact right-edge strip
            const PANEL_STRIP_WIDTH: f64 = 380.0;
            if let Ok(Some(monitor)) = self.app_handle.primary_monitor() {
                let scale = monitor.scale_factor();
                let logical_w = monitor.size().width as f64 / scale;
                let logical_h = monitor.size().height as f64 / scale;
                let x = logical_w - PANEL_STRIP_WIDTH;
                let _ = window.set_size(tauri::LogicalSize::new(PANEL_STRIP_WIDTH, logical_h));
                let _ = window.set_position(tauri::LogicalPosition::new(x, 0.0));
            }
            let _ = window.show();
            let _ = window.set_ignore_cursor_events(false);
            debug!("Overlay layout: compact panel strip");
        } else {
            // Full-screen click-through (default)
            if let Ok(Some(monitor)) = self.app_handle.primary_monitor() {
                let size = monitor.size();
                let _ = window.set_position(tauri::LogicalPosition::new(0.0, 0.0));
                let _ = window.set_size(tauri::PhysicalSize::new(size.width, size.height));
            }
            let _ = window.set_ignore_cursor_events(true);
            debug!("Overlay layout: full-screen click-through");
        }
    }

    /// Toggle overlay interactivity for detection or coaching.
    ///
    /// Delegates to `apply_window_layout` to respect mode priority. When
    /// `interactive = true` for detection, the overlay goes full-screen
    /// interactive only if no higher-priority mode overrides it.
    pub fn set_interactive(&self, interactive: bool) {
        // For backwards compat: set_interactive is used by detection toggle
        // and coaching dismiss. Detection state is tracked separately via
        // detection_active flag, so this is a best-effort fallback.
        // Callers that manage specific modes should use the dedicated methods.
        if let Err(e) = self.ensure_window() {
            debug!("ensure_window failed: {e}");
            return;
        }
        if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
            if interactive {
                let _ = window.show();
                let _ = window.set_ignore_cursor_events(false);
            } else if let Ok(state) = self.state.try_read() {
                self.apply_window_layout(&state);
                return;
            } else {
                // Fallback: couldn't acquire lock, just set click-through
                let _ = window.set_ignore_cursor_events(true);
            }
        }
        debug!("Overlay set_interactive={interactive}");
    }

    /// Enter or leave compact panel mode for the suggestions panel.
    /// Recalculates window layout respecting mode priority.
    pub fn set_panel_mode(&self, open: bool) {
        if let Ok(mut state) = self.state.try_write() {
            state.suggestions_panel_open = open;
            self.apply_window_layout(&state);
        } else {
            debug!("set_panel_mode: lock contention, skipping");
        }
    }

    /// Enter or leave automation confirmation mode.
    /// Full-screen interactive — highest priority overlay mode.
    pub fn set_automation_confirm_mode(&self, active: bool) {
        if let Ok(mut state) = self.state.try_write() {
            state.automation_confirm_active = active;
            self.apply_window_layout(&state);
        } else {
            debug!("set_automation_confirm_mode: lock contention, skipping");
        }
    }

    /// Emit focus mode state change to overlay frontend.
    pub fn emit_focus_mode(&self, active: bool, auto: bool) {
        let _ = self.app_handle.emit(
            "overlay:focus-mode",
            serde_json::json!({ "active": active, "auto": auto }),
        );
    }

    /// Notify overlay that the suggestion queue changed (item added/removed/feedback).
    pub fn emit_suggestions_changed(&self, count: usize) {
        let _ = self.app_handle.emit(
            "overlay:suggestions-changed",
            serde_json::json!({ "count": count }),
        );
    }

    /// Toggle the suggestions panel from keyboard shortcut (Cmd+Shift+S).
    pub fn emit_toggle_suggestions(&self) {
        if let Err(e) = self.app_handle.emit("overlay:toggle-suggestions", ()) {
            debug!("emit overlay:toggle-suggestions failed: {e}");
        }
    }

    /// Emit a brief capture feedback flash to the overlay.
    pub fn emit_capture_feedback(&self, timestamp: &str) {
        let _ = self.app_handle.emit(
            "overlay:capture-feedback",
            serde_json::json!({ "timestamp": timestamp }),
        );
    }

    /// Emit heatmap grid data to the overlay for HeatmapGhost rendering.
    /// `grid` is a flat 50×50 array of normalized [0.0, 1.0] values (row-major).
    pub fn emit_heatmap(&self, grid: Vec<f32>) {
        #[derive(Serialize)]
        struct HeatmapPayload {
            grid: Vec<f32>,
            cols: usize,
            rows: usize,
        }

        let payload = HeatmapPayload {
            grid,
            cols: 50,
            rows: 50,
        };

        if let Err(e) = self.app_handle.emit("overlay:heatmap-update", &payload) {
            debug!("failed to emit overlay:heatmap-update: {e}");
        }
    }
}

/// Create the tracking panel window — a small, transparent, always-on-top
/// indicator bar centered horizontally near the top of the primary monitor.
///
/// Starts hidden; shown/hidden via the `toggle-indicator` tray menu item
/// or IPC commands. The panel renders the capture-active border indicator.
///
/// Gracefully degrades on Linux/Wayland (panel not supported).
pub fn create_tracking_panel(app_handle: &AppHandle) -> Result<(), String> {
    if app_handle.get_webview_window("tracking-panel").is_some() {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        warn!("Wayland — tracking panel disabled");
        return Err("Wayland not supported".to_string());
    }

    let monitor = app_handle
        .primary_monitor()
        .map_err(|e| format!("monitor: {e}"))?
        .ok_or("No monitor")?;

    let scale = monitor.scale_factor();
    let logical_width = monitor.size().width as f64 / scale;
    let logical_height = monitor.size().height as f64 / scale;
    let panel_width = 260.0;
    let panel_height = 36.0;
    let x = (logical_width / 2.0) - (panel_width / 2.0);

    // Dock-aware Y: use NSScreen::visibleFrame() to avoid Dock overlap.
    // macOS coords: bottom-left origin. Tauri coords: top-left origin.
    // visibleFrame().origin.y = distance from screen bottom to Dock top (in points).
    #[cfg(target_os = "macos")]
    let y = {
        use objc2::MainThreadMarker;
        MainThreadMarker::new()
            .and_then(|mtm| {
                let screen = objc2_app_kit::NSScreen::mainScreen(mtm)?;
                let vf = screen.visibleFrame();
                // Convert macOS bottom-up to Tauri top-down:
                // tauri_y = logical_height - (macos_y / scale) - panel_height - margin
                Some(logical_height - vf.origin.y / scale - panel_height - 8.0)
            })
            .unwrap_or(logical_height - panel_height - 80.0)
    };
    #[cfg(not(target_os = "macos"))]
    let y = logical_height - panel_height - 80.0;

    WebviewWindowBuilder::new(
        app_handle,
        "tracking-panel",
        WebviewUrl::App("tracking-panel.html".into()),
    )
    .title("Maekon Tracking")
    .inner_size(panel_width, panel_height)
    .position(x, y)
    .transparent(true)
    .always_on_top(true)
    .decorations(false)
    .resizable(true)
    .min_inner_size(260.0, 36.0)
    .max_inner_size(320.0, 310.0)
    .visible(false)
    .skip_taskbar(true)
    .shadow(false)
    .build()
    .map_err(|e| format!("panel build: {e}"))?;

    info!("Tracking panel window created");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_state_default_mode() {
        let state = OverlayState {
            mode: OverlayMode::Minimal,
            visible: false,
            current_message_id: None,
            detection_active: false,
            suggestions_panel_open: false,
            automation_confirm_active: false,
        };
        assert_eq!(state.mode, OverlayMode::Minimal);
        assert!(!state.visible);
        assert!(state.current_message_id.is_none());
        assert!(!state.detection_active);
        assert!(!state.suggestions_panel_open);
        assert!(!state.automation_confirm_active);
    }

    #[test]
    fn overlay_coaching_payload_serde_roundtrip() {
        let payload = OverlayCoachingPayload {
            message_id: "msg-001".to_string(),
            profile: "FocusGuard".to_string(),
            trigger_type: "RegimeDrift".to_string(),
            text: "Take a break from coding.".to_string(),
            auto_dismiss_secs: 15,
            explanation: "Frequent app switching detected in 'Coding'. FocusGuard profile flagged possible attention drift.".to_string(),
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: OverlayCoachingPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.message_id, "msg-001");
        assert_eq!(restored.auto_dismiss_secs, 15);
    }

    #[test]
    fn overlay_upgrade_payload_serde_roundtrip() {
        let payload = OverlayUpgradePayload {
            message_id: "msg-002".to_string(),
            personalized_text: "Great focus session! Time for a well-earned break.".to_string(),
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: OverlayUpgradePayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.message_id, "msg-002");
        assert!(restored.personalized_text.contains("well-earned"));
    }

    #[test]
    fn overlay_focus_payload_serde_roundtrip() {
        let payload = OverlayFocusPayload {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
            border_color: "#3b82f6".to_string(),
            opacity: 0.8,
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: OverlayFocusPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.x, 100);
        assert_eq!(restored.width, 800);
        assert!((restored.opacity - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn overlay_goal_payload_serde_roundtrip() {
        let payload = OverlayGoalPayload {
            goals: vec![GoalProgressView {
                regime_label: "Deep Work".to_string(),
                current_minutes: 45,
                target_minutes: 120,
                percentage: 37,
                display_color: "#3b82f6".to_string(),
            }],
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let restored: OverlayGoalPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(restored.goals.len(), 1);
        assert_eq!(restored.goals[0].regime_label, "Deep Work");
    }
}
