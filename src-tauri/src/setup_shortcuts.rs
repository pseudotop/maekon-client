use tauri::App;
use tracing::info;

pub(crate) fn register_all(app: &App) {
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
                            let _ = handle.emit_to(
                                "magic-overlay",
                                "overlay:capture-state-changed",
                                &payload,
                            );
                            let _ = handle.emit_to(
                                "tracking-panel",
                                "overlay:capture-state-changed",
                                &payload,
                            );
                            let _ = crate::tray::sync_tray_state(
                                &handle,
                                new_paused,
                                indicator_visible,
                            );
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
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let result =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+O", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let state: tauri::State<'_, crate::runtime_state::AppState> =
                            handle.state();
                        if let Some(ref overlay) = state.magic_overlay {
                            overlay.set_interactive(true);
                        }
                    });
                }
            });

    match result {
        Ok(()) => info!("Global shortcut registered: CmdOrCtrl+Shift+O (overlay toggle)"),
        Err(e) => tracing::warn!("Failed to register overlay shortcut: {e}"),
    }
}

/// Register Cmd+Shift+S (macOS) / Ctrl+Shift+S (Windows/Linux) to toggle
/// the suggestions panel in the MagicOverlay. Makes the overlay interactive
/// so the user can accept/reject/defer suggestions.
fn register_suggestions_shortcut(app: &App) {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    let result =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+S", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    let handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(state) =
                            handle.try_state::<crate::runtime_state::SuggestionRuntimeState>()
                        {
                            if let Some(overlay) = state.overlay() {
                                // Only emit the toggle event — the frontend's useEffect
                                // calls toggle_suggestions_panel IPC which handles both
                                // window resize and interactivity.
                                overlay.emit_toggle_suggestions();
                            }
                        }
                    });
                }
            });

    match result {
        Ok(()) => info!("Global shortcut registered: CmdOrCtrl+Shift+S (suggestions panel)"),
        Err(e) => tracing::warn!("Failed to register suggestions shortcut: {e}"),
    }
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
                            if let Some(overlay) = state.overlay() {
                                overlay.set_interactive(true);
                            }
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

/// Register Cmd+Shift+A (macOS) / Ctrl+Shift+A (Windows/Linux) to open
/// the automation quick-access panel. Emits an event to the main window
/// so the frontend can show the automation modal.
fn register_automation_shortcut(app: &App) {
    use tauri::Emitter;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    if let Err(e) =
        app.global_shortcut()
            .on_shortcut("CmdOrCtrl+Shift+A", |app_handle, _shortcut, event| {
                if event.state == ShortcutState::Pressed {
                    info!("automation quick-access triggered via shortcut");
                    let _ = app_handle.emit("automation:quick-access", ());
                }
            })
    {
        tracing::warn!("failed to register automation shortcut: {e}");
    }
}
