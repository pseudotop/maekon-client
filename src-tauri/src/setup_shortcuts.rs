use tauri::App;
use tracing::info;

pub(crate) fn register_all(app: &App) {
    crate::shortcut_registry::reset();

    // Register global overlay toggle shortcut (Cmd+Shift+O / Ctrl+Shift+O)
    register_overlay_shortcut(app);

    // Register capture toggle shortcut (Cmd+Shift+\ / Ctrl+Shift+\)
    register_capture_shortcut(app);

    // Register suggestions panel toggle shortcut (Cmd+Shift+S / Ctrl+Shift+S)
    register_suggestions_shortcut(app);

    // Register detection overlay toggle shortcut (Cmd+Shift+D / Ctrl+Shift+D)
    register_detection_shortcut(app);

    // Register detection overlay refresh shortcut (Cmd+Shift+R / Ctrl+Shift+R)
    register_detection_refresh_shortcut(app);

    // Register automation quick-access shortcut (Cmd+Shift+A / Ctrl+Shift+A)
    register_automation_shortcut(app);
}

/// Returns true when a forced shortcut collision is requested for the given
/// component via the `MAEKON_TEST_FORCE_SHORTCUT_COLLISION` env var (a
/// comma-separated component list, e.g. "overlay,suggestions"). Used by private
/// Windows collision tests to deterministically exercise the fallback path
/// without depending on whatever third-party utility happens to hold the chord.
fn shortcut_collision_forced(component: &str) -> bool {
    collision_list_contains(
        std::env::var("MAEKON_TEST_FORCE_SHORTCUT_COLLISION")
            .ok()
            .as_deref(),
        component,
    )
}

/// Pure parser for the collision component list — split out from the env read
/// so the matching logic is unit-testable without mutating process env state.
fn collision_list_contains(list: Option<&str>, component: &str) -> bool {
    list.map(|value| value.split(',').any(|item| item.trim() == component))
        .unwrap_or(false)
}

/// Register Cmd+Shift+\ (macOS) / Ctrl+Shift+\ (Windows/Linux) to toggle
/// capture pause state. Emits state change events to overlay and tracking panel,
/// and rebuilds the tray menu to reflect the new state.
fn register_capture_shortcut(app: &App) {
    use tauri::{Emitter, Manager};
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let result =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+\\", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(state) = handle.try_state::<crate::runtime_state::AppState>() {
                            let was_paused = state
                                .capture_paused
                                .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                            let new_paused = !was_paused;
                            let indicator_visible = state
                                .indicator_visible
                                .load(std::sync::atomic::Ordering::Relaxed);
                            let payload = serde_json::json!({
                                "paused": new_paused,
                                "indicator_visible": indicator_visible
                            });
                            if let Err(e) = handle.emit_to(
                                "magic-overlay",
                                "overlay:capture-state-changed",
                                &payload,
                            ) {
                                tracing::debug!("emit magic-overlay failed: {e}");
                            }
                            if let Err(e) = handle.emit_to(
                                "tracking-panel",
                                "overlay:capture-state-changed",
                                &payload,
                            ) {
                                tracing::debug!("emit tracking-panel failed: {e}");
                            }
                            if let Err(e) =
                                crate::tray::sync_tray_state(&handle, new_paused, indicator_visible)
                            {
                                tracing::debug!("sync_tray_state failed: {e}");
                            }
                            crate::magic_overlay::sync_passive_tracking_surface(
                                &handle,
                                new_paused,
                                indicator_visible,
                            );
                            // Immediately re-gate the VAD receiver so a pause tears
                            // down the microphone at once (the ≤2 s tick is the
                            // backstop for other gate terms), AND remember/auto-rearm
                            // VAD across the pause toggle. Shared helper so every
                            // capture-pause site routes through the same path.
                            crate::commands::audio::on_capture_pause_toggled(&handle, new_paused);
                            #[cfg(target_os = "macos")]
                            if let Some(border) =
                                handle.try_state::<crate::native_border::NativeBorderState>()
                            {
                                border.0.set_paused(new_paused);
                            }
                        }
                    });
                }
            });

    match result {
        Ok(()) => info!("Global shortcut registered: CmdOrCtrl+Shift+\\ (capture toggle)"),
        Err(e) => tracing::warn!("Failed to register capture shortcut: {e}"),
    }
}

/// Register Cmd+Shift+O (macOS) / Ctrl+Shift+O (Windows/Linux) to toggle
/// the MagicOverlay into interactive mode so users can click coaching popup buttons.
/// The overlay reverts to click-through automatically on dismiss or auto-dismiss timeout.
fn register_overlay_shortcut(app: &App) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    const SHORTCUT_ID: &str = "overlay-toggle";
    const PURPOSE: &str = "overlay toggle";
    const PRIMARY: &str = "CmdOrCtrl+Shift+O";
    const FALLBACK: &str = "CmdOrCtrl+Alt+O";

    let forced_collision = shortcut_collision_forced("overlay");

    let result = if forced_collision {
        Err("forced shortcut collision for private test".to_string())
    } else {
        app.global_shortcut()
            .on_shortcut(PRIMARY, |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    activate_overlay_shortcut(app_handle);
                }
            })
            .map_err(|error| error.to_string())
    };

    match result {
        Ok(()) => {
            crate::shortcut_registry::record_registered(SHORTCUT_ID, PURPOSE, PRIMARY);
            info!("Global shortcut registered: {PRIMARY} (overlay toggle)");
        }
        Err(primary_error) => {
            tracing::warn!("Failed to register overlay shortcut: {primary_error}");
            let fallback_result =
                app.global_shortcut()
                    .on_shortcut(FALLBACK, |app_handle, _shortcut, event| {
                        if event.state == ShortcutState::Pressed {
                            activate_overlay_shortcut(app_handle);
                        }
                    });

            match fallback_result {
                Ok(()) => {
                    crate::shortcut_registry::record_fallback_registered(
                        SHORTCUT_ID,
                        PURPOSE,
                        PRIMARY,
                        FALLBACK,
                        primary_error,
                    );
                    info!("Global fallback shortcut registered: {FALLBACK} (overlay toggle)");
                }
                Err(fallback_error) => {
                    let fallback_error = fallback_error.to_string();
                    crate::shortcut_registry::record_fallback_failed(
                        SHORTCUT_ID,
                        PURPOSE,
                        PRIMARY,
                        FALLBACK,
                        primary_error,
                        fallback_error.clone(),
                    );
                    tracing::warn!(
                        "Failed to register overlay fallback shortcut: {fallback_error}"
                    );
                }
            }
        }
    }
}

fn activate_overlay_shortcut(app_handle: &tauri::AppHandle) {
    use tauri::Manager;

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let state: tauri::State<'_, crate::runtime_state::AppState> = handle.state();
        if let Some(ref overlay) = state.magic_overlay {
            overlay.set_interactive(true);
        }
    });
}

/// Register Cmd+Shift+S (macOS) / Ctrl+Shift+S (Windows/Linux) to toggle
/// the suggestions panel in the MagicOverlay. Makes the overlay interactive
/// so the user can accept/reject/defer suggestions.
///
/// On Windows the primary chord (Ctrl+Shift+S) is frequently intercepted by
/// screenshot/capture utilities (#4698), so this mirrors the overlay-toggle
/// fallback pattern: when the primary accelerator cannot be registered, a
/// secondary chord (Cmd/Ctrl+Alt+S) is registered instead and the collision is
/// recorded in the global shortcut status so the UI can surface a notice. The
/// tracking-panel suggestions button remains the guaranteed mouse-accessible path.
fn register_suggestions_shortcut(app: &App) {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    const SHORTCUT_ID: &str = "suggestions-toggle";
    const PURPOSE: &str = "suggestions panel";
    const PRIMARY: &str = "CmdOrCtrl+Shift+S";
    const FALLBACK: &str = "CmdOrCtrl+Alt+S";

    let forced_collision = shortcut_collision_forced("suggestions");

    let result = if forced_collision {
        Err("forced shortcut collision for private test".to_string())
    } else {
        app.global_shortcut()
            .on_shortcut(PRIMARY, |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    activate_suggestions_shortcut(app_handle);
                }
            })
            .map_err(|error| error.to_string())
    };

    match result {
        Ok(()) => {
            crate::shortcut_registry::record_registered(SHORTCUT_ID, PURPOSE, PRIMARY);
            info!("Global shortcut registered: {PRIMARY} (suggestions panel)");
        }
        Err(primary_error) => {
            tracing::warn!("Failed to register suggestions shortcut: {primary_error}");
            let fallback_result =
                app.global_shortcut()
                    .on_shortcut(FALLBACK, |app_handle, _shortcut, event| {
                        if event.state == ShortcutState::Pressed {
                            activate_suggestions_shortcut(app_handle);
                        }
                    });

            match fallback_result {
                Ok(()) => {
                    crate::shortcut_registry::record_fallback_registered(
                        SHORTCUT_ID,
                        PURPOSE,
                        PRIMARY,
                        FALLBACK,
                        primary_error,
                    );
                    info!("Global fallback shortcut registered: {FALLBACK} (suggestions panel)");
                }
                Err(fallback_error) => {
                    let fallback_error = fallback_error.to_string();
                    crate::shortcut_registry::record_fallback_failed(
                        SHORTCUT_ID,
                        PURPOSE,
                        PRIMARY,
                        FALLBACK,
                        primary_error,
                        fallback_error.clone(),
                    );
                    tracing::warn!(
                        "Failed to register suggestions fallback shortcut: {fallback_error}"
                    );
                }
            }
        }
    }
}

fn activate_suggestions_shortcut(app_handle: &tauri::AppHandle) {
    use tauri::Manager;

    let handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = handle.try_state::<crate::runtime_state::SuggestionRuntimeState>() {
            if let Some(overlay) = state.overlay() {
                // Only emit the toggle event — the frontend's useEffect
                // calls toggle_suggestions_panel IPC which handles both
                // window resize and interactivity.
                overlay.emit_toggle_suggestions();
            }
        }
    });
}

fn register_detection_shortcut(app: &App) {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    if let Err(e) =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+D", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let state: tauri::State<'_, crate::runtime_state::DetectionRuntimeState> =
                            handle.state();
                        let now_active = state.toggle_active();

                        if now_active {
                            tracing::info!("detection overlay toggled ON via shortcut");
                            crate::commands::detection::spawn_detection_analysis_from_state(&state);
                        } else {
                            tracing::info!("detection overlay toggled OFF via shortcut");
                            if let Some(overlay) = state.overlay() {
                                overlay.clear_detection_scene().await;
                                overlay.set_interactive(false);
                            }
                        }
                    });
                }
            })
    {
        tracing::warn!("failed to register detection shortcut: {e}");
    }
}

fn register_detection_refresh_shortcut(app: &App) {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    if let Err(e) =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+R", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let state: tauri::State<'_, crate::runtime_state::DetectionRuntimeState> =
                            handle.state();
                        if state.is_active() {
                            tracing::info!("detection overlay refresh via shortcut");
                            crate::commands::detection::spawn_detection_analysis_from_state(&state);
                        }
                    });
                }
            })
    {
        tracing::warn!("failed to register detection refresh shortcut: {e}");
    }
}

/// Register Cmd+Shift+A (macOS) / Ctrl+Shift+A (Windows/Linux) to open the
/// automation page. Brings the main window forward and emits the unified
/// `navigate` deep-link event (matching the tray menu). The earlier
/// `automation:quick-access` event was folded into `navigate` (see
/// useTauriEventBridge.ts) and had no frontend listener, so the shortcut was a
/// silent no-op; route it to `/automation` like the tray "run-preset" entry.
fn register_automation_shortcut(app: &App) {
    use tauri::Emitter;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    if let Err(e) =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+A", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    info!("automation quick-access triggered via shortcut");
                    crate::tray::focus_main_window(app_handle);
                    if let Err(e) = app_handle.emit_to("main", "navigate", "/automation") {
                        tracing::warn!("emit navigate(/automation) failed: {e}");
                    }
                }
            })
    {
        tracing::warn!("failed to register automation shortcut: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::collision_list_contains;

    #[test]
    fn collision_list_matches_named_component() {
        // single component
        assert!(collision_list_contains(Some("suggestions"), "suggestions"));
        assert!(collision_list_contains(Some("overlay"), "overlay"));
        // multi-component list (the suggestions fallback must still trigger when
        // the chord is listed alongside others)
        assert!(collision_list_contains(
            Some("overlay,suggestions"),
            "suggestions"
        ));
        // whitespace around items is tolerated
        assert!(collision_list_contains(
            Some(" overlay , suggestions "),
            "suggestions"
        ));
    }

    #[test]
    fn collision_list_ignores_absent_unset_or_partial() {
        // unset env → no forced collision
        assert!(!collision_list_contains(None, "suggestions"));
        // empty value → no match
        assert!(!collision_list_contains(Some(""), "suggestions"));
        // a different component listed must not force this one
        assert!(!collision_list_contains(Some("overlay"), "suggestions"));
        // substring must NOT match (exact component only)
        assert!(!collision_list_contains(
            Some("suggestionsX"),
            "suggestions"
        ));
    }
}
