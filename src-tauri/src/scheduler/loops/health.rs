use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{debug, info};

/// Adapter-side health flags — each is written by the real adapter that owns
/// the corresponding subsystem, storing `true` on an observed success and
/// `false` on an observed failure (#8050). They start optimistic (`true`) at
/// the composition root, so a subsystem reads healthy until its adapter records
/// a real failure. Writers:
/// - `server_ok`: the heartbeat loop (`send_heartbeat` result) and the sync
///   loop (successful batch `flush`) in `loops/network.rs`.
/// - `llm_ok`: the `AuditingSession` send decorator (`auditing_session.rs`),
///   the outermost wrapper over every chat provider.
/// - `cli_ok`: the automation controller command result
///   (`maekon-automation` `controller/preset.rs`).
///
/// This loop only READS these flags and mirrors them into the UI-facing
/// [`ConnectionFlags`]; it never writes them.
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

/// One health tick's DECISION half (#9637 seam): mirror the adapter-owned
/// flags into the UI-facing connection flags and report the new snapshot iff
/// anything changed (`None` = steady state, no UI work). Pure with respect to
/// Tauri — the spawn loop below owns the side effects (window emits, tray
/// sync, logging) — so the transition semantics are unit-testable without an
/// AppHandle, which is what previously made this file's logic untestable.
fn evaluate_health_tick(
    adapter_flags: &AdapterHealthFlags,
    connection_flags: &ConnectionFlags,
) -> Option<(bool, bool, bool)> {
    let srv = adapter_flags.server_ok.load(Ordering::Relaxed);
    let llm = adapter_flags.llm_ok.load(Ordering::Relaxed);
    let cli = adapter_flags.cli_ok.load(Ordering::Relaxed);

    let prev_srv = connection_flags.server.swap(srv, Ordering::Relaxed);
    let prev_llm = connection_flags.llm.swap(llm, Ordering::Relaxed);
    let prev_cli = connection_flags.cli.swap(cli, Ordering::Relaxed);

    (prev_srv != srv || prev_llm != llm || prev_cli != cli).then_some((srv, llm, cli))
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
        let mut ticker = super::intervals::coalescing_interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Some((srv, llm, cli)) = evaluate_health_tick(&adapter_flags, &connection_flags) {
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
                    // #7947: self RSS/CPU budget sampling now runs as its own
                    // always-on loop (`spawn_resource_health_loop`), decoupled
                    // from the health-probe wiring so it also runs in minimal /
                    // OSS configs where this health loop is not spawned.
                }
                _ = shutdown_rx.changed() => {
                    info!("health check loop shutdown");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(server: bool, llm: bool, cli: bool) -> (AdapterHealthFlags, ConnectionFlags) {
        (
            AdapterHealthFlags {
                server_ok: Arc::new(AtomicBool::new(server)),
                llm_ok: Arc::new(AtomicBool::new(llm)),
                cli_ok: Arc::new(AtomicBool::new(cli)),
            },
            ConnectionFlags {
                server: Arc::new(AtomicBool::new(server)),
                llm: Arc::new(AtomicBool::new(llm)),
                cli: Arc::new(AtomicBool::new(cli)),
            },
        )
    }

    #[test]
    fn steady_state_reports_no_change() {
        let (adapter, connection) = flags(true, true, true);
        assert_eq!(evaluate_health_tick(&adapter, &connection), None);
        // Repeated ticks stay quiet — no per-tick UI churn.
        assert_eq!(evaluate_health_tick(&adapter, &connection), None);
    }

    #[test]
    fn adapter_failure_is_mirrored_and_reported_once() {
        let (adapter, connection) = flags(true, true, true);
        adapter.server_ok.store(false, Ordering::Relaxed);

        assert_eq!(
            evaluate_health_tick(&adapter, &connection),
            Some((false, true, true)),
            "the first tick after a failure must report the transition"
        );
        assert!(
            !connection.server.load(Ordering::Relaxed),
            "the UI-facing flag must mirror the adapter flag"
        );
        assert_eq!(
            evaluate_health_tick(&adapter, &connection),
            None,
            "an unchanged failure state must not re-notify every tick"
        );
    }

    #[test]
    fn recovery_and_multi_flag_transitions_report_the_full_snapshot() {
        let (adapter, connection) = flags(false, false, true);
        adapter.server_ok.store(true, Ordering::Relaxed);
        adapter.cli_ok.store(false, Ordering::Relaxed);

        // One tick, two flags moved (one recovery, one failure) → single
        // notification carrying the complete new snapshot.
        assert_eq!(
            evaluate_health_tick(&adapter, &connection),
            Some((true, false, false))
        );
        assert!(connection.server.load(Ordering::Relaxed));
        assert!(!connection.cli.load(Ordering::Relaxed));
    }
}
