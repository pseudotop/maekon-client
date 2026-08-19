// OOS-TBD: ADR-013 file split — baselined past the 900-line giant
// threshold while growing for #9646; split per ADR-003 when next touched.
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
pub(crate) use window::set_tracking_panel_visible;

use maekon_core::config::OverlayMode;
use maekon_core::models::coaching::{CoachingMessage, DismissAction};
use maekon_core::ports::foreground_window::ForegroundFullscreenProbe;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PassiveOverlayWindowPolicy {
    Hidden,
    FullScreenClickThrough,
}

pub(crate) fn passive_overlay_window_policy(
    coaching_visible: bool,
    effective_capture_permitted: bool,
    cua_safe_mode: bool,
) -> PassiveOverlayWindowPolicy {
    if coaching_visible || (effective_capture_permitted && !cua_safe_mode) {
        PassiveOverlayWindowPolicy::FullScreenClickThrough
    } else {
        PassiveOverlayWindowPolicy::Hidden
    }
}

pub(crate) fn passive_tracking_surface_policy(
    target_os: &str,
    effective_capture_permitted: bool,
    indicator_visible: bool,
    capture_paused: bool,
) -> PassiveTrackingSurfacePolicy {
    // #8094: the passive recording border may render ONLY when capture is
    // EFFECTIVELY permitted — i.e. `capture_permitted_now` (consent.screen_capture
    // AND capture_enabled AND active_hours AND NOT tracking-muted AND NOT paused).
    // A fresh no-consent profile has `effective_capture_permitted == false`, so the
    // border stays hidden even though `indicator.show_border` defaults true. Before
    // this term was added, a no-consent Windows profile rendered a recording border
    // while nothing was captured (QC CRT-PRV-UX-PRIVACY-VIS-001). `capture_paused`
    // is already subsumed by the effective gate but kept as an explicit belt-and-
    // suspenders term.
    if target_os == "windows" && effective_capture_permitted && indicator_visible && !capture_paused
    {
        PassiveTrackingSurfacePolicy::ThinBorderWindows
    } else {
        PassiveTrackingSurfacePolicy::Hidden
    }
}

pub(crate) fn current_passive_tracking_surface_policy(
    effective_capture_permitted: bool,
    indicator_visible: bool,
    capture_paused: bool,
) -> PassiveTrackingSurfacePolicy {
    passive_tracking_surface_policy(
        std::env::consts::OS,
        effective_capture_permitted,
        indicator_visible,
        capture_paused,
    )
}

/// #8094: the macOS native recording border may show ONLY when capture is
/// effectively permitted, the indicator is visible, capture is not paused, and
/// CUA safe mode is off. Pure so it is unit-testable on every platform (the
/// `#[cfg(target_os = "macos")]` call sites that consume it are not).
// All non-test callers (commands/capture_status, setup/platform,
// reconcile_capture_indicator below) are `#[cfg(target_os = "macos")]`-gated,
// so on other platforms the lib build sees no user — mirror the
// `native_border::mod.rs` pattern to keep the fn cross-platform-testable
// without tripping the deny(dead_code) CI build on Linux/Windows.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn native_recording_border_visible(
    effective_capture_permitted: bool,
    indicator_visible: bool,
    capture_paused: bool,
    cua_safe_mode: bool,
) -> bool {
    effective_capture_permitted && indicator_visible && !capture_paused && !cua_safe_mode
}

/// Reads the EFFECTIVE capture-permitted gate (the same `capture_permitted_now`
/// composite the monitor loop uses to decide whether to capture) from
/// `AppState`. This is the single source of truth the recording border mirrors:
/// the border must never render unless capture is actually happening (#8094).
///
/// Fail-closed: consent flows through `ConsentGate` (all-false unless consent is
/// currently `Valid`, and all-false when no `ConsentManager` is wired), so a
/// fresh no-consent profile yields `false`.
pub(crate) fn effective_capture_permitted(
    state: &crate::runtime_state::AppState,
    capture_paused: bool,
) -> bool {
    let consent = maekon_core::ports::consent_manager::ConsentGate::from_ref(
        state.capture.consent_manager.as_ref(),
    )
    .permissions_snapshot();
    crate::scheduler::capture_permitted_now(&state.config, &consent, capture_paused)
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
    // #8094: gate the border on the live effective capture state, not just the
    // raw `indicator_visible`/`capture_paused` flags.
    let effective = effective_capture_permitted(&state, capture_paused);
    match current_passive_tracking_surface_policy(effective, indicator_visible, capture_paused) {
        PassiveTrackingSurfacePolicy::ThinBorderWindows => overlay.show_passive_tracking_surface(),
        PassiveTrackingSurfacePolicy::Hidden => overlay.hide_passive_tracking_surface(),
    }
}

/// #8094: reconcile BOTH recording-indicator surfaces (Windows passive thin-
/// border + macOS native border) to the live effective capture gate.
///
/// Call after any transition that can change the effective gate WITHOUT already
/// syncing the border — notably a consent grant/revoke, which opens or closes
/// the `screen_capture` term. The pause / indicator IPC sites already call
/// `sync_passive_tracking_surface`; this is the consent-change equivalent so a
/// grant surfaces the border immediately and a revoke tears it down within the
/// same command (not only after the next pause/indicator toggle).
pub(crate) fn reconcile_capture_indicator(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let Some(state) = app.try_state::<crate::runtime_state::AppState>() else {
        return;
    };
    let paused = state.capture_paused.load(Ordering::Relaxed);
    let indicator_visible = state.indicator_visible.load(Ordering::Relaxed);
    // Windows passive border (effective gate computed inside).
    sync_passive_tracking_surface(app, paused, indicator_visible);
    // macOS native border.
    #[cfg(target_os = "macos")]
    if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>() {
        let effective = effective_capture_permitted(&state, paused);
        border.0.set_paused(paused);
        if native_recording_border_visible(
            effective,
            indicator_visible,
            paused,
            crate::app_runtime_launch::cua_safe_mode_enabled(),
        ) {
            border.0.show();
        } else {
            border.0.hide();
        }
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
    "overlay:pointer-context-update",
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
/// Created during app setup. The interactive overlay WebView is pre-created
/// hidden and shown only for an active transient surface. Persistent recording
/// indication uses the separate thin border windows, avoiding an idle
/// full-screen transparent compositor surface.
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

    /// Return whether the interactive overlay surface was visible before a
    /// short-lived diagnostic probe changed its layout.
    #[cfg(debug_assertions)]
    pub(crate) fn window_is_visible(&self) -> bool {
        self.app_handle
            .get_webview_window(OVERLAY_LABEL)
            .and_then(|window| window.is_visible().ok())
            .unwrap_or(false)
    }

    #[cfg(debug_assertions)]
    pub(crate) fn window_exists(&self) -> bool {
        self.app_handle.get_webview_window(OVERLAY_LABEL).is_some()
    }

    /// Restore the overlay surface after a diagnostic probe.
    ///
    /// Probes temporarily make the full-screen surface visible and
    /// click-through. If the surface was hidden before the probe, hide it again
    /// so its transparent WebView cannot keep compositing indefinitely. If it
    /// was already visible, re-apply the live mode so an interactive panel or
    /// confirmation dialog does not remain click-through.
    #[cfg(debug_assertions)]
    pub(crate) fn restore_window_after_probe(&self, existed: bool, was_visible: bool) {
        if !existed {
            if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                if let Err(error) = window.destroy() {
                    debug!("overlay diagnostic probe destroy failed: {error}");
                }
            }
            return;
        }

        if was_visible {
            if let Ok(state) = self.state.try_read() {
                self.apply_window_layout(&state);
            } else {
                debug!("overlay state busy while restoring diagnostic probe layout");
            }
            return;
        }

        if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
            let _ = window.set_ignore_cursor_events(true);
            if let Err(error) = window.hide() {
                debug!("overlay diagnostic probe hide failed: {error}");
            }
        }
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

        let mut state = self.state.write().await;
        state.visible = true;
        state.current_message_id = Some(message.message_id.clone());
        self.apply_window_layout(&state);
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
        let dismissed_current = state.current_message_id.as_deref() == Some(message_id);
        if dismissed_current {
            state.current_message_id = None;
            state.visible = false;
        }

        if dismissed_current && self.app_handle.get_webview_window(OVERLAY_LABEL).is_some() {
            self.apply_window_layout(&state);
        }
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
    ///   4. Passive capture/coaching — full-screen click-through
    ///   5. Idle — hidden
    fn apply_window_layout(&self, state: &OverlayState) {
        let effective_capture = self
            .app_handle
            .try_state::<crate::runtime_state::AppState>()
            .map(|app_state| {
                let paused = app_state
                    .capture_paused
                    .load(std::sync::atomic::Ordering::Relaxed);
                effective_capture_permitted(&app_state, paused)
            })
            .unwrap_or(false);
        let passive_policy = passive_overlay_window_policy(
            state.visible,
            effective_capture,
            crate::app_runtime_launch::cua_safe_mode_enabled(),
        );
        let window_required = state.automation_confirm_active
            || state.detection_active
            || state.suggestions_panel_open
            || passive_policy == PassiveOverlayWindowPolicy::FullScreenClickThrough;

        if !window_required {
            if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                if let Err(error) = window.destroy() {
                    debug!("idle overlay destroy failed: {error}");
                }
            }
            debug!("Overlay layout: destroyed (idle)");
            return;
        }

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
            // #9646: the strip is BOUNDED in height, not monitor-tall. The
            // visible panel card is ~190-500px depending on tab, yet the
            // window used to span the full monitor height with
            // ignore_cursor_events(false) — the transparent remainder
            // swallowed clicks destined for the apps underneath and showed up
            // as a screen-tall window in screenshots/Expose (user-reported).
            // The panel scrolls internally, so a fixed cap loses nothing;
            // clamp to the monitor for short displays.
            const PANEL_STRIP_MAX_HEIGHT: f64 = 560.0;
            if let Ok(monitor) = target_overlay_monitor(&self.app_handle) {
                let layout = overlay_monitor_layout(&monitor);
                let x = layout.origin_x + layout.logical_width - PANEL_STRIP_WIDTH;
                let height = PANEL_STRIP_MAX_HEIGHT.min(layout.logical_height);
                let _ = window.set_size(tauri::LogicalSize::new(PANEL_STRIP_WIDTH, height));
                let _ = window.set_position(tauri::LogicalPosition::new(x, layout.origin_y));
            }
            let _ = window.set_focusable(false);
            let _ = show_overlay_window(&window);
            let _ = window.set_ignore_cursor_events(false);
            debug!("Overlay layout: compact panel strip (bounded height)");
        } else {
            // Reset passive geometry before either showing or hiding. A later
            // pointer/coaching event may show the pre-created window directly;
            // it must not inherit the previous bounded suggestions-strip bounds (#9646).
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
            let _ = show_overlay_window(&window);
            debug!("Overlay layout: full-screen click-through (passive surface active)");
        }
    }

    /// Evaluate the fullscreen-suppression policy (CRT-PRV-OVL-005).
    ///
    /// Considers BOTH a Maekon-owned webview reporting fullscreen (via
    /// `webview_windows()`) AND — added in #8849 — the foreground EXTERNAL
    /// application (fullscreen game, browser video, presentation), which
    /// `webview_windows()` cannot see. The pure decision lives in
    /// [`types::decide_fullscreen_policy`] so the external scenario is
    /// unit-testable without a live desktop.
    fn evaluate_fullscreen_policy(&self) -> OverlayFullscreenPolicyPayload {
        let owned_fullscreen = self
            .app_handle
            .webview_windows()
            .into_values()
            .any(|window| {
                !is_internal_overlay_window(window.label())
                    && window.is_fullscreen().unwrap_or(false)
            });

        // #8849: probe the foreground external window through the platform port.
        // `None` (undetermined — unsupported platform / no X server / permission)
        // degrades gracefully to "external not fullscreen".
        let external_fullscreen =
            maekon_monitor::foreground_fullscreen::PlatformForegroundFullscreenProbe
                .foreground_is_fullscreen();

        types::decide_fullscreen_policy(owned_fullscreen, external_fullscreen)
    }

    /// THE single authoritative gate every INTERACTIVE overlay-open path routes
    /// through (#8858). Evaluates the fullscreen policy FIRST — before any cold
    /// `ensure_window()` — considering the foreground external app (#8849), then
    /// records and emits the decision. Callers MUST NOT create/show an
    /// interactive surface when the returned decision is not `overlay_allowed`
    /// (OVL-005). Kept as one method so a future refactor cannot reintroduce a
    /// bypass (routing is asserted by the maekon-lint overlay-gate gate).
    fn gate_interactive_open(&self) -> OverlayFullscreenPolicyPayload {
        let decision = self.evaluate_fullscreen_policy();
        *self.last_fullscreen_policy.lock() = Some(decision.clone());
        if let Err(e) = self.app_handle.emit("overlay:fullscreen-policy", &decision) {
            debug!("emit overlay:fullscreen-policy failed: {e}");
        }
        if !decision.overlay_allowed {
            debug!(
                policy = %decision.policy,
                reason = %decision.reason,
                "interactive overlay open suppressed"
            );
        }
        decision
    }

    pub fn set_interactive(&self, interactive: bool) {
        if interactive {
            // #8858: gate FIRST — evaluate the fullscreen policy (incl. external
            // foreground apps, #8849) BEFORE any cold window creation. A
            // suppressed open must NEVER create/show the interactive surface.
            if !self.gate_interactive_open().overlay_allowed {
                if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                    let _ = window.set_ignore_cursor_events(true);
                    let _ = window.hide();
                }
                return;
            }
            if let Err(e) = self.ensure_window() {
                debug!("ensure_window failed: {e}");
                return;
            }
            if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                let _ = window.set_focusable(false);
                let _ = show_overlay_window(&window);
                let _ = window.set_ignore_cursor_events(false);
            }
            debug!("Overlay set_interactive=true");
        } else {
            // Closing needs no gate — recompute the passive/idle layout, which
            // may hide or destroy the surface (apply_window_layout handles the
            // ensure/destroy decision itself; no cold ensure_window here).
            if let Ok(state) = self.state.try_read() {
                self.apply_window_layout(&state);
            } else if let Some(window) = self.app_handle.get_webview_window(OVERLAY_LABEL) {
                let _ = window.set_ignore_cursor_events(true);
            }
            debug!("Overlay set_interactive=false");
        }
    }

    /// Set the suggestions-panel mode, routing an OPEN through the single
    /// fullscreen gate (#8858). Returns the AUTHORITATIVE resolved open state:
    /// an open suppressed by the policy resolves to `false` so native + frontend
    /// state stay in agreement (OVL-005). The gate (an OS probe + event emit)
    /// runs OUTSIDE the state write lock; the guard is held only across the sync
    /// `apply_window_layout`, matching the emit_detection_scene pattern (#6830).
    pub async fn set_panel_mode(&self, open: bool) -> bool {
        let effective_open = if open {
            self.gate_interactive_open().overlay_allowed
        } else {
            false
        };
        {
            // Scope the write guard so it drops at its last use
            // (`apply_window_layout`), not across the trailing return
            // (clippy::significant_drop_tightening). Held across the sync
            // `apply_window_layout` — matching emit_detection_scene (#6830).
            let mut state = self.state.write().await;
            state.suggestions_panel_open = effective_open;
            self.apply_window_layout(&state);
        }
        effective_open
    }

    /// Toggle the suggestions panel from its AUTHORITATIVE native state (#8847).
    ///
    /// Native-first: the toggle target is derived from the native
    /// `suggestions_panel_open` flag, not from the frontend WebView (which the
    /// idle policy may have destroyed), so the shortcut works from a cold/idle
    /// overlay. The OPEN transition routes through the single fullscreen gate
    /// (#8858). Returns the resolved open state so the caller can emit an
    /// idempotent explicit state event.
    pub async fn toggle_panel_mode(&self) -> bool {
        let target_open = !self.state.read().await.suggestions_panel_open;
        self.set_panel_mode(target_open).await
    }

    /// Return the native suggestions-panel state.
    ///
    /// The overlay WebView can be created lazily after a tracking-panel open
    /// request. In that cold-start path the event emitted before WebView
    /// readiness is not buffered, so the frontend must hydrate from this
    /// authoritative state before writing its initial reducer value back.
    pub async fn suggestions_panel_open(&self) -> bool {
        self.state.read().await.suggestions_panel_open
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

    /// Emit the AUTHORITATIVE suggestions-panel open/closed state as an
    /// idempotent explicit event (#8847). The frontend's
    /// `overlay:set-suggestions-panel` listener converges its reducer to this
    /// value, so event timing cannot invert or lose the native state — unlike a
    /// relative toggle event, which is lost when the WebView was destroyed by
    /// the idle policy. Replaces the former emit-only relative toggle emitter.
    pub fn emit_suggestions_panel_state(&self, open: bool) {
        if let Err(e) = self.app_handle.emit(
            "overlay:set-suggestions-panel",
            serde_json::json!({ "open": open }),
        ) {
            debug!("emit overlay:set-suggestions-panel failed: {e}");
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

        // Pointer coordinates are screen-content-derived data. Keep them in
        // the transparent overlay webview instead of broadcasting them to the
        // main and tracking-panel webviews.
        if let Err(e) =
            emit_overlay_event(&self.app_handle, "overlay:pointer-context-update", &payload)
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
