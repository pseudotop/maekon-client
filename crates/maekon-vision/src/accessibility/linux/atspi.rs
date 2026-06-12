//! AT-SPI2 focus event listener.
//!
//! Provides an event-driven alternative to polling: subscribes to
//! `StateChangedEvent` over D-Bus and caches the D-Bus coordinates
//! (bus name + object path) of the last focused accessible object.

#[cfg(feature = "linux-atspi")]
use std::sync::Arc;

#[cfg(feature = "linux-atspi")]
use tokio::sync::RwLock;
#[cfg(feature = "linux-atspi")]
use tracing::{debug, info, warn};

#[cfg(feature = "linux-atspi")]
use crate::error::VisionError;

// ── Public types ──────────────────────────────────────────────────────────────

/// D-Bus coordinates of a focused accessible object.
///
/// Stores the bus name (destination) and object path so the caller can
/// build an `AccessibleProxy` for the focused element without walking
/// the entire tree.
#[cfg(feature = "linux-atspi")]
#[derive(Debug, Clone)]
pub struct FocusedObjectInfo {
    /// D-Bus bus name (e.g. ":1.42" or "org.gnome.Terminal").
    pub bus_name: String,
    /// D-Bus object path (e.g. "/org/a11y/atspi/accessible/123").
    pub object_path: String,
}

/// Handle to a running focus event listener.
///
/// Dropping this handle cancels the background task via two mechanisms:
/// 1. `_shutdown_tx` drop triggers `shutdown_rx.changed()` inside the task.
/// 2. `_task.abort()` in the `Drop` impl forces cancellation even if the
///    AT-SPI stream blocks before reaching the `select!` shutdown arm
///    (F-RR-C26-02: sibling of Cycle 25 GrpcSseAdapter JoinHandle fix).
///
/// `JoinHandle` is not `Clone`, so this struct intentionally does not
/// derive `Clone`. Callers that need shared access should wrap in `Arc`.
#[cfg(feature = "linux-atspi")]
pub struct FocusEventListenerHandle {
    /// Cached last focused object coordinates, updated by the listener task.
    pub(super) last_focused: Arc<RwLock<Option<FocusedObjectInfo>>>,
    /// Sending side of the shutdown channel. The listener task holds
    /// the receiver and stops when it fires.
    pub(super) _shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    /// JoinHandle for the background task. Held to prevent the task from
    /// being orphaned if the AT-SPI stream blocks before the shutdown arm.
    pub(super) _task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "linux-atspi")]
impl Drop for FocusEventListenerHandle {
    fn drop(&mut self) {
        // Abort unconditionally: _shutdown_tx drop fires the watch signal
        // first, but if the task is blocked in the event stream before it
        // can observe that signal, abort() guarantees cancellation.
        self._task.abort();
    }
}

#[cfg(feature = "linux-atspi")]
impl FocusEventListenerHandle {
    /// Read the last focused object info without blocking.
    ///
    /// Returns `None` if no focus event has been received yet or if the
    /// listener has not started.
    pub async fn last_focused(&self) -> Option<FocusedObjectInfo> {
        self.last_focused.read().await.clone()
    }

    /// Check whether a focus event has been received at least once.
    pub async fn has_focus(&self) -> bool {
        self.last_focused.read().await.is_some()
    }
}

// ── Internal listener ─────────────────────────────────────────────────────────

/// Background focus event listener that subscribes to AT-SPI
/// `StateChangedEvent` notifications over D-Bus.
#[cfg(feature = "linux-atspi")]
pub(super) struct FocusEventListener;

#[cfg(feature = "linux-atspi")]
impl FocusEventListener {
    /// Spawn the listener task and return a handle.
    ///
    /// The listener connects to AT-SPI, registers for `ObjectEvents`,
    /// and filters the event stream for `StateChangedEvent` with
    /// state == Focused and enabled == true. Each matching event
    /// updates the shared `last_focused` cache.
    ///
    /// The task runs until the returned handle (and all its clones) are
    /// dropped, which triggers the shutdown watch channel.
    pub(super) async fn spawn() -> Result<FocusEventListenerHandle, VisionError> {
        use ::atspi::connection::AccessibilityConnection;
        use ::atspi::events::ObjectEvents;

        let conn = AccessibilityConnection::new().await.map_err(|e| {
            VisionError::Internal(format!(
                "AT-SPI2 focus listener: D-Bus connection failed: {e}"
            ))
        })?;

        conn.register_event::<ObjectEvents>().await.map_err(|e| {
            VisionError::Internal(format!(
                "AT-SPI2 focus listener: event registration failed: {e}"
            ))
        })?;

        let last_focused: Arc<RwLock<Option<FocusedObjectInfo>>> = Arc::new(RwLock::new(None));
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        let cache = Arc::clone(&last_focused);

        // F-RR-C26-02: store the JoinHandle so Drop can abort the task
        // if the AT-SPI stream blocks before reaching the shutdown select arm.
        let task = tokio::spawn(async move {
            use ::atspi::events::object::StateChangedEvent;
            use atspi_common::State;
            use futures::StreamExt;

            let stream = conn.event_stream();
            tokio::pin!(stream);

            info!("AT-SPI2 focus event listener started");

            loop {
                tokio::select! {
                    biased;

                    // Shutdown signal — stop the loop
                    _ = shutdown_rx.changed() => {
                        info!("AT-SPI2 focus event listener shutting down");
                        break;
                    }

                    // Next event from the AT-SPI stream
                    event_opt = stream.next() => {
                        match event_opt {
                            Some(Ok(event)) => {
                                // Try to convert to StateChangedEvent
                                if let Ok(state_change) = StateChangedEvent::try_from(event) {
                                    if state_change.state == State::Focused
                                        && state_change.enabled
                                    {
                                        let item = &state_change.item;
                                        let info = FocusedObjectInfo {
                                            bus_name: item.name_as_str().unwrap_or("").to_string(),
                                            object_path: item.path_as_str().to_string(),
                                        };
                                        debug!(
                                            bus = %info.bus_name,
                                            path = %info.object_path,
                                            "AT-SPI2 focus changed"
                                        );
                                        *cache.write().await = Some(info);
                                    }
                                }
                            }
                            Some(Err(e)) => {
                                warn!("AT-SPI2 event stream error: {e}");
                                // Continue listening -- transient errors are expected
                            }
                            None => {
                                // Stream ended unexpectedly
                                warn!("AT-SPI2 event stream ended unexpectedly");
                                break;
                            }
                        }
                    }
                }
            }

            // Deregister events on shutdown (best-effort)
            if let Err(e) = conn.deregister_event::<ObjectEvents>().await {
                debug!("AT-SPI2 focus listener: deregister failed (non-fatal): {e}");
            }
        });

        Ok(FocusEventListenerHandle {
            last_focused,
            _shutdown_tx: Arc::new(shutdown_tx),
            _task: task,
        })
    }
}
