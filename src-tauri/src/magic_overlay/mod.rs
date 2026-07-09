//! MagicOverlay — transparent Tauri WebView overlay for coaching, detection,
//! and suggestion panel display.
//!
//! ADR-013: 500L threshold applied — original 1061L file converted to module
//! folder with submodules:
//!   types.rs     — payload structs + OverlayState
//!   window.rs    — monitor helpers, create_tracking_panel
//!   detection.rs — build_detection_payload + visibility filtering

mod detection;
#[cfg(test)]
mod tests;
mod types;
mod window;

pub use types::{
    OverlayCoachingPayload, OverlayFocusPayload, OverlayFullscreenPolicyPayload,
    OverlayGoalPayload, OverlayModePayload, OverlayPointerContextPayload, OverlayUpgradePayload,
};
pub use window::create_tracking_panel;

use maekon_core::config::OverlayMode;
use maekon_core::models::coaching::{CoachingMessage, DismissAction};
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use types::OverlayState;
use window::{
    is_internal_overlay_window, overlay_monitor_layout, show_overlay_window,
    sync_tracking_border_windows, target_overlay_monitor, OVERLAY_LABEL,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PassiveTrackingSurfacePolicy {
    Hidden,
    ThinBorderWindows,
}

pub(crate) fn passive_tracking_surface_policy(
    target_os: &str,
    indicator_visible: bool,
    capture_paused: bool,
) -> PassiveTrackingSurfacePolicy {
    if target_os == "windows" && indicator_visible && !capture_paused {
        PassiveTrackingSurfacePolicy::ThinBorderWindows
    } else {
        PassiveTrackingSurfacePolicy::Hidden
    }
}

pub(crate) fn current_passive_tracking_surface_policy(
    indicator_visible: bool,
    capture_paused: bool,
) -> PassiveTrackingSurfacePolicy {
    passive_tracking_surface_policy(std::env::consts::OS, indicator_visible, capture_paused)
}

pub(crate) fn sync_passive_tracking_surface<R: Runtime>(
    app_handle: &tauri::AppHandle<R>,
    capture_paused: bool,
    indicator_visible: bool,
) {
    let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() else {
        return;
    };
    let Some(ref overlay) = state.magic_overlay else {
        return;
    };
    match current_passive_tracking_surface_policy(indicator_visible, capture_paused) {
        PassiveTrackingSurfacePolicy::ThinBorderWindows => overlay.show_passive_tracking_surface(),
        PassiveTrackingSurfacePolicy::Hidden => overlay.hide_passive_tracking_surface(),
    }
}

// ── #7076: least-privilege event scoping ────────────────────────────────
//
// Overlay "screen-content" events carry on-screen accessibility text and
// detected GUI element labels derived from the user's screen. They are rendered
// ONLY by the transparent overlay webview, so they must be delivered to that
// window alone via `emit_to(OVERLAY_LABEL, ...)`. A global `emit` would also
// reach the `main` and `tracking-panel` webviews — both hold the
// `core:event:allow-listen` capability but have no functional need for this
// content — contradicting the per-window capability separation the app enforces.
const OVERLAY_SCREEN_CONTENT_EVENTS: &[&str] = &[
    "overlay:update-focus",
    "overlay:clear-focus",
    "overlay:detection-update",
    "overlay:detection-clear",
    "overlay:heatmap-update",
];

/// Returns the single webview label a screen-content event must be scoped to, or
/// `None` for control events that may broadcast app-wide.
fn screen_content_event_target(event: &str) -> Option<&'static str> {
    if OVERLAY_SCREEN_CONTENT_EVENTS.contains(&event) {
        Some(OVERLAY_LABEL)
    } else {
        None
    }
}

/// Emit an overlay event with least-privilege scoping (#7076).
///
/// Screen-content events are delivered only to the transparent overlay webview
/// via [`Emitter::emit_to`]; all other (control) events keep the app-wide
/// [`Emitter::emit`] broadcast. Shared by [`MagicOverlayHandle`] and the
/// `MagicOverlayDriver` so the scoping policy has a single source of truth.
pub(crate) fn emit_overlay_event<S: serde::Serialize + Clone>(
    app_handle: &AppHandle,
    event: &str,
    payload: S,
) -> tauri::Result<()> {
    match screen_content_event_target(event) {
        Some(label) => app_handle.emit_to(label, event, payload),
        None => app_handle.emit(event, payload),
    }
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
/// unsuitable for the `maekon-core` port layer.
#[derive(Clone)]
pub struct MagicOverlayHandle {
    app_handle: AppHandle,
    state: Arc<RwLock<OverlayState>>,
    last_fullscreen_policy: Arc<Mutex<Option<OverlayFullscreenPolicyPayload>>>,
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
            last_fullscreen_policy: Arc::new(Mutex::new(None)),
        }
    }

    /// Create the overlay window if it does not yet exist.
    ///
    /// Gracefully degrades on Linux/Wayland (overlay not supported).
    /// macOS requires `macos-private-api` feature flag for transparent windows.
    /// Windows requires `shadow: false` to avoid rendering artifacts.
    pub fn ensure_window(&self) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            warn!("Wayland detected — MagicOverlay disabled, using notification fallback");
            return Err("Wayland does not support transparent overlay windows".to_string());
        }

        if self.app_handle.get_webview_window(OVERLAY_LABEL).is_some() {
            return Ok(());
        }

        let layout = overlay_monitor_layout(&target_overlay_monitor(&self.app_handle)?);

        let builder = tauri::WebviewWindowBuilder::new(
            &self.app_handle,
            OVERLAY_LABEL,
            tauri::WebviewUrl::App(window::OVERLAY_URL.into()),
        )
        .title("Maekon Overlay")
        .inner_size(layout.logical_width, layout.logical_height)
        .position(layout.origin_x, layout.origin_y)
        .transparent(true)
        .always_on_top(true)
        .decorations(false)
        .focused(false)
        .focusable(false)
        .resizable(false)
        .visible(false)
        .skip_taskbar(true)
        .shadow(false);

        let window = builder
            .build()
            .map_err(|e| format!("Failed to create overlay window: {e}"))?;

        if let Err(e) = window.set_ignore_cursor_events(true) {
            debug!("set_ignore_cursor_events failed: {e}");
        }

        info!("MagicOverlay window created");
        Ok(())
    }

    pub fn show_passive_tracking_surface(&self) {
        if let Err(e) = sync_tracking_border_windows(&self.app_handle, true) {
            debug!("tracking border windows unavailable: {e}");
        }
    }

    pub fn hide_passive_tracking_surface(&self) {
        if let Err(e) = sync_tracking_border_windows(&self.app_handle, false) {
            debug!("tracking border windows hide failed: {e}");
        }
    }

    /// Show a coaching message on the overlay.
    pub async fn show_coaching(&self, message: &CoachingMessage) {
        if let Err(e) = self.ensure_window() {
            debug!("overlay unavailable, skipping show_coaching: {e}");
            return;
        }

        let payload = OverlayCoachingPayload {
            message_id: message.message_id.clone(),
            profile: format!("{:?}", message.profile),
            trigger_type: maekon_core::models::coaching::trigger_type_name(&message.trigger),
            text: message.display_text().to_string(),
            auto_dismiss_secs: 15,
            explanation: message.explanation.clone(),
        };

        if let Err(e) = self.app_handle.emit("overlay:show-coaching", &payload) {
            warn!("failed to emit overlay:show-coaching: {e}");
            return;
        }

        if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
            if let Err(e) = show_overlay_window(&window) {
                debug!("window show failed: {e}");
            }
        }

        let mut state = self.state.write().await;
        state.visible = true;
        state.current_message_id = Some(message.message_id.clone());
    }

    /// Upgrade the coaching message text with LLM-personalized content.
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
    pub async fn dismiss(&self, message_id: &str, _action: DismissAction) {
        let mut state = self.state.write().await;
        if state.current_message_id.as_deref() == Some(message_id) {
            state.current_message_id = None;
        }
        state.visible = false;
        drop(state);

        if let Err(e) = self.app_handle.emit("overlay:dismiss", message_id) {
            warn!("failed to emit overlay:dismiss: {e}");
        }
    }

    // #7719: unlike `clear_focus_highlight`/`emit_detection_scene`/
    // `clear_detection_scene` (all live — called from setup_shortcuts.rs,
    // commands/detection.rs, scheduler/loops/detection_helper.rs), this
    // method has no caller: focus-highlight updates now route through the
    // `OverlayDriver` trait (`scheduler/loops/detection_helper::
    // update_focus_highlight` + `MagicOverlayDriver`), not this
    // webview-emit path.
    #[allow(dead_code)]
    pub fn update_focus_highlight(&self, highlight: OverlayFocusPayload) {
        // #7076: screen-content event — scoped to the overlay webview only.
        if let Err(e) = emit_overlay_event(&self.app_handle, "overlay:update-focus", &highlight) {
            warn!("failed to emit overlay:update-focus: {e}");
        }
    }

    pub fn clear_focus_highlight(&self) {
        // #7076: paired with the scoped focus update — scoped to the overlay webview.
        if let Err(e) = emit_overlay_event(&self.app_handle, "overlay:clear-focus", ()) {
            debug!("emit overlay:clear-focus failed: {e}");
        }
    }

    /// Emit a full UiScene to the detection overlay.
    pub async fn emit_detection_scene(&self, scene: &maekon_core::models::ui_scene::UiScene) {
        self.clear_focus_highlight();

        let Some(payload) = detection::build_detection_payload(scene) else {
            warn!(
                scene_id = %scene.scene_id,
                element_count = scene.elements.len(),
                "detection scene has no visible elements — keeping overlay click-through",
            );
            self.clear_detection_scene().await;
            return;
        };

        if let Err(e) = self.ensure_window() {
            debug!("ensure_window failed: {e}");
        }
        // #7076: screen-content event (detected GUI element labels) — scoped to
        // the overlay webview only.
        if let Err(e) = emit_overlay_event(&self.app_handle, "overlay:detection-update", &payload) {
            warn!("failed to emit overlay:detection-update: {e}");
            self.clear_detection_scene().await;
            return;
        }

        let mut state = self.state.write().await;
        state.detection_active = true;
        self.apply_window_layout(&state);
        info!(
            scene_id = %scene.scene_id,
            element_count = payload.element_count,
            "detection overlay updated"
        );
    }

    pub async fn clear_detection_scene(&self) {
        // #7076: paired with the scoped detection update — scoped to the overlay webview.
        if let Err(e) = emit_overlay_event(&self.app_handle, "overlay:detection-clear", ()) {
            debug!("emit overlay:detection-clear failed: {e}");
        }
        let window_exists = self.app_handle.get_webview_window(OVERLAY_LABEL).is_some();
        let mut state = self.state.write().await;
        state.detection_active = false;
        if window_exists {
            self.apply_window_layout(&state);
        }
        debug!("detection overlay cleared");
    }

    pub fn update_goal_progress(
        &self,
        goals: Vec<maekon_core::models::coaching::GoalProgressView>,
    ) {
        let payload = OverlayGoalPayload { goals };
        if let Err(e) = self.app_handle.emit("overlay:update-goals", &payload) {
            warn!("failed to emit overlay:update-goals: {e}");
        }
    }

    // #7686 removed the IPC commands that called these overlay-mode accessors.
    // Kept for the mode-switching surface that magic_overlay is expected to
    // regain (multi-mode overlay UX is still an active roadmap item) — not
    // reachable from any current caller, so `-D warnings` flags them.
    #[allow(dead_code)]
    pub async fn set_mode(&self, mode: OverlayMode) {
        let mut state = self.state.write().await;
        state.mode = mode;
        drop(state);

        let payload = OverlayModePayload { mode };
        if let Err(e) = self.app_handle.emit("overlay:set-mode", &payload) {
            warn!("failed to emit overlay:set-mode: {e}");
        }
    }

    #[allow(dead_code)]
    pub async fn get_mode(&self) -> OverlayMode {
        self.state.read().await.mode
    }

    #[allow(dead_code)]
    pub async fn is_visible(&self) -> bool {
        self.state.read().await.visible
    }

    #[allow(dead_code)]
    pub fn fullscreen_policy_state(&self) -> Option<OverlayFullscreenPolicyPayload> {
        self.last_fullscreen_policy.lock().clone()
    }

    #[allow(dead_code)]
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
    fn apply_window_layout(&self, state: &OverlayState) {
        if let Err(e) = self.ensure_window() {
            debug!("ensure_window failed: {e}");
            return;
        }

        let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) else {
            return;
        };

        if state.automation_confirm_active || state.detection_active {
            if let Ok(monitor) = target_overlay_monitor(&self.app_handle) {
                let layout = overlay_monitor_layout(&monitor);
                let _ = window.set_position(tauri::LogicalPosition::new(
                    layout.origin_x,
                    layout.origin_y,
                ));
                let _ = window.set_size(tauri::LogicalSize::new(
                    layout.logical_width,
                    layout.logical_height,
                ));
            }
            let _ = window.set_focusable(false);
            let _ = show_overlay_window(&window);
            let _ = window.set_ignore_cursor_events(false);
            debug!(
                "Overlay layout: full-screen interactive (automation={}, detection={})",
                state.automation_confirm_active, state.detection_active
            );
        } else if state.suggestions_panel_open {
            const PANEL_STRIP_WIDTH: f64 = 380.0;
            if let Ok(monitor) = target_overlay_monitor(&self.app_handle) {
                let layout = overlay_monitor_layout(&monitor);
                let x = layout.origin_x + layout.logical_width - PANEL_STRIP_WIDTH;
                let _ = window.set_size(tauri::LogicalSize::new(
                    PANEL_STRIP_WIDTH,
                    layout.logical_height,
                ));
                let _ = window.set_position(tauri::LogicalPosition::new(x, layout.origin_y));
            }
            let _ = window.set_focusable(false);
            let _ = show_overlay_window(&window);
            let _ = window.set_ignore_cursor_events(false);
            debug!("Overlay layout: compact panel strip");
        } else {
            if let Ok(monitor) = target_overlay_monitor(&self.app_handle) {
                let layout = overlay_monitor_layout(&monitor);
                let _ = window.set_position(tauri::LogicalPosition::new(
                    layout.origin_x,
                    layout.origin_y,
                ));
                let _ = window.set_size(tauri::LogicalSize::new(
                    layout.logical_width,
                    layout.logical_height,
                ));
            }
            let _ = window.set_ignore_cursor_events(true);
            debug!("Overlay layout: full-screen click-through");
        }
    }

    fn evaluate_fullscreen_policy(&self) -> OverlayFullscreenPolicyPayload {
        let fullscreen_detected = self
            .app_handle
            .webview_windows()
            .into_values()
            .any(|window| {
                !is_internal_overlay_window(window.label())
                    && window.is_fullscreen().unwrap_or(false)
            });

        if fullscreen_detected {
            OverlayFullscreenPolicyPayload {
                fullscreen_detected: true,
                policy: "suppress".to_string(),
                overlay_allowed: false,
                reason: "native fullscreen window detected".to_string(),
            }
        } else {
            OverlayFullscreenPolicyPayload {
                fullscreen_detected: false,
                policy: "show_on_top".to_string(),
                overlay_allowed: true,
                reason: "no fullscreen window detected".to_string(),
            }
        }
    }

    pub fn set_interactive(&self, interactive: bool) {
        if let Err(e) = self.ensure_window() {
            debug!("ensure_window failed: {e}");
            return;
        }

        if interactive {
            let decision = self.evaluate_fullscreen_policy();
            *self.last_fullscreen_policy.lock() = Some(decision.clone());
            if let Err(e) = self.app_handle.emit("overlay:fullscreen-policy", &decision) {
                debug!("emit overlay:fullscreen-policy failed: {e}");
            }
            if !decision.overlay_allowed {
                if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                    let _ = window.set_ignore_cursor_events(true);
                    let _ = window.hide();
                }
                debug!(
                    policy = %decision.policy,
                    reason = %decision.reason,
                    "overlay interactive request suppressed"
                );
                return;
            }
        }

        if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
            if interactive {
                let _ = window.set_focusable(false);
                let _ = show_overlay_window(&window);
                let _ = window.set_ignore_cursor_events(false);
            } else if let Ok(state) = self.state.try_read() {
                self.apply_window_layout(&state);
                return;
            } else {
                let _ = window.set_ignore_cursor_events(true);
            }
        }
        debug!("Overlay set_interactive={interactive}");
    }

    pub async fn set_panel_mode(&self, open: bool) {
        // #6830: awaited write lock instead of try_write() + silent skip — on
        // lock contention the old path dropped the update while the command still
        // returned Ok, so the panel layout silently did not change. Holding the
        // write guard across the sync `apply_window_layout` matches the existing
        // emit_detection_scene/clear_detection_scene pattern. Callers are async
        // Tauri commands.
        let mut state = self.state.write().await;
        state.suggestions_panel_open = open;
        self.apply_window_layout(&state);
    }

    pub async fn set_automation_confirm_mode(&self, active: bool) {
        // #6830: awaited write lock (deterministic). A dropped update here is the
        // worst case of all overlay modes — a skipped *deactivation* would leave
        // the full-screen automation-confirm modal capturing all desktop input
        // while the toggle reports success.
        let mut state = self.state.write().await;
        state.automation_confirm_active = active;
        self.apply_window_layout(&state);
    }

    pub fn emit_focus_mode(&self, active: bool, auto: bool) {
        let _ = self.app_handle.emit(
            "overlay:focus-mode",
            serde_json::json!({ "active": active, "auto": auto }),
        );
    }

    pub fn emit_suggestions_changed(&self, count: usize) {
        let _ = self.app_handle.emit(
            "overlay:suggestions-changed",
            serde_json::json!({ "count": count }),
        );
    }

    pub fn emit_toggle_suggestions(&self) {
        if let Err(e) = self.app_handle.emit("overlay:toggle-suggestions", ()) {
            debug!("emit overlay:toggle-suggestions failed: {e}");
        }
    }

    pub fn emit_capture_feedback(&self, timestamp: &str) {
        let _ = self.app_handle.emit(
            "overlay:capture-feedback",
            serde_json::json!({ "timestamp": timestamp }),
        );
    }

    pub fn emit_pointer_context(&self, payload: OverlayPointerContextPayload) {
        if payload.enabled {
            if let Err(e) = self.ensure_window() {
                debug!("ensure_window failed for pointer context: {e}");
            } else if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                let _ = window.set_ignore_cursor_events(true);
                if let Err(e) = show_overlay_window(&window) {
                    debug!("pointer context window show failed: {e}");
                }
            }
        }

        if let Err(e) = self
            .app_handle
            .emit("overlay:pointer-context-update", &payload)
        {
            debug!("failed to emit overlay:pointer-context-update: {e}");
        }
    }

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

        // #7076: screen-content-derived event — scoped to the overlay webview only.
        if let Err(e) = emit_overlay_event(&self.app_handle, "overlay:heatmap-update", &payload) {
            debug!("failed to emit overlay:heatmap-update: {e}");
        }
    }
}
