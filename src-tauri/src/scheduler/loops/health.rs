use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{debug, info};

/// Adapter-side health flags — written by adapters on success/failure.
pub(crate) struct AdapterHealthFlags {
    pub server_ok: Arc<AtomicBool>,
    pub llm_ok: Arc<AtomicBool>,
    pub cli_ok: Arc<AtomicBool>,
}

/// UI-facing connection status — read by tray, overlay, and IPC commands.
pub(crate) struct ConnectionFlags {
    pub server: Arc<AtomicBool>,
    pub llm: Arc<AtomicBool>,
    pub cli: Arc<AtomicBool>,
}

/// Spawn a periodic health check loop that reads adapter health flags,
/// updates connection status flags, and emits Tauri events on change.
///
/// Uses concrete `AppHandle` (Wry runtime), not generic `<R: Runtime>`.
pub(crate) fn spawn_health_check_loop(
    interval: Duration,
    adapter_flags: AdapterHealthFlags,
    connection_flags: ConnectionFlags,
    app_handle: AppHandle,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    let srv = adapter_flags.server_ok.load(Ordering::Relaxed);
                    let llm = adapter_flags.llm_ok.load(Ordering::Relaxed);
                    let cli = adapter_flags.cli_ok.load(Ordering::Relaxed);

                    let prev_srv = connection_flags.server.swap(srv, Ordering::Relaxed);
                    let prev_llm = connection_flags.llm.swap(llm, Ordering::Relaxed);
                    let prev_cli = connection_flags.cli.swap(cli, Ordering::Relaxed);

                    if prev_srv != srv || prev_llm != llm || prev_cli != cli {
                        let payload = serde_json::json!({
                            "server": srv, "llm": llm, "cli": cli
                        });
                        if let Err(e) = app_handle.emit_to("magic-overlay", "overlay:connection-changed", &payload) {
                            debug!("emit magic-overlay failed: {e}");
                        }
                        if let Err(e) = app_handle.emit_to("tracking-panel", "overlay:connection-changed", &payload) {
                            debug!("emit tracking-panel failed: {e}");
                        }

                        if let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() {
                            let paused = state.capture_paused.load(Ordering::Relaxed);
                            let visible = state.indicator_visible.load(Ordering::Relaxed);
                            if let Err(e) = crate::tray::sync_tray_state(&app_handle, paused, visible) {
                                debug!("sync_tray_state failed: {e}");
                            }
                        }
                        info!(server = srv, llm = llm, cli = cli, "connection status changed");
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("health check loop shutdown");
                    break;
                }
            }
        }
    })
}
