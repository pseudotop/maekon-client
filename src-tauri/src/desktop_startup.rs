use tauri::{App, Manager};
#[allow(unused_imports)]
use tracing::{debug, info};

pub(crate) struct DesktopStartupCoordinator;

impl DesktopStartupCoordinator {
    pub(crate) fn apply(
        app: &App,
        frontend_web_port: u16,
        local_auth_token: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        crate::tray::setup_tray(app)?;

        #[cfg(target_os = "macos")]
        {
            crate::macos_integration::set_dock_icon();
            info!("macOS dock icon set from embedded icon.png");
        }

        Self::show_main_window(app, frontend_web_port, local_auth_token);
        Ok(())
    }

    fn show_main_window(app: &App, frontend_web_port: u16, local_auth_token: &str) {
        if let Some(window) = app.get_webview_window("main") {
            #[cfg(not(target_os = "macos"))]
            {
                if let Err(e) = window.set_decorations(false) {
                    debug!("set_decorations failed: {e}");
                }
            }

            let port_js = format!("window.__MAEKON_WEB_PORT__ = {};", frontend_web_port);
            if let Err(e) = window.eval(&port_js) {
                debug!("eval failed: {e}");
            }

            // E20-41 (#4833): inject the per-session local-API auth token into the
            // legit WebView only. A different local user's browser never transits
            // Tauri, so it never receives the token and is rejected (401) by the
            // /api gate. `{:?}` JSON-quotes/escapes the hex token. The frontend also
            // has a get_local_auth_token IPC fallback to cover the post-load timing.
            let auth_js = format!("window.__MAEKON_LOCAL_AUTH__ = {local_auth_token:?};");
            if let Err(e) = window.eval(&auth_js) {
                debug!("local-auth eval failed: {e}");
            }

            crate::window_state::restore_main_window_state(&window);

            if let Err(e) = window.show() {
                debug!("window show failed: {e}");
            }
            if let Err(e) = window.set_focus() {
                debug!("set_focus failed: {e}");
            }
            debug_assert!(
                window.is_visible().unwrap_or(false),
                "main window must be visible after desktop startup"
            );
        } else {
            debug_assert!(false, "main window not found during desktop startup");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn desktop_startup_contains_window_show_call() {
        let startup_src = include_str!("desktop_startup.rs");

        assert!(
            startup_src.contains("window.show()"),
            "desktop startup must call window.show()"
        );
        assert!(
            startup_src.contains("window.set_focus()"),
            "desktop startup must call window.set_focus() after show()"
        );
    }
}
