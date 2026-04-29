pub mod autostart_migration;
pub mod migration_hashes;
pub mod sd_notify;

use tokio::sync::watch;
use tracing::{debug, info, warn};

/// Grace period for second Ctrl+C to force exit.
const FORCE_EXIT_GRACE_SECS: u64 = 3;

pub struct LifecycleManager {
    shutdown_tx: watch::Sender<bool>,
    #[allow(dead_code)] // used by subscribe()
    shutdown_rx: watch::Receiver<bool>,
}

impl LifecycleManager {
    pub fn new() -> Self {
        let (tx, rx) = watch::channel(false);
        Self {
            shutdown_tx: tx,
            shutdown_rx: rx,
        }
    }

    #[allow(dead_code)] // available for direct subscriber wiring
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub fn shutdown(&self) {
        info!("ended sent");
        if let Err(e) = self.shutdown_tx.send(true) {
            debug!("channel send failed: {e}");
        }
    }

    pub async fn wait_for_signal(&self) {
        self.wait_first_signal().await;
        info!("Shutting down gracefully... press Ctrl+C again to force exit");
        self.shutdown();

        // Wait for second signal → force exit
        self.wait_second_signal().await;
    }

    async fn wait_first_signal(&self) {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");

            tokio::select! {
                _ = sigint.recv() => {
                    info!("SIGINT received");
                }
                _ = sigterm.recv() => {
                    info!("SIGTERM received");
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to register Ctrl+C handler");
            info!("Ctrl+C received");
        }
    }

    async fn wait_second_signal(&self) {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigint =
                signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");
            let mut sigterm =
                signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");

            tokio::select! {
                _ = sigint.recv() => {
                    warn!("Second SIGINT received — force exit");
                    std::process::exit(1);
                }
                _ = sigterm.recv() => {
                    warn!("SIGTERM received during shutdown — force exit");
                    std::process::exit(1);
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(FORCE_EXIT_GRACE_SECS)) => {
                    // Grace period expired without second signal — exit cleanly
                    info!("Graceful shutdown completed");
                    std::process::exit(0);
                }
            }
        }

        #[cfg(not(unix))]
        {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    warn!("Second Ctrl+C received — force exit");
                    std::process::exit(1);
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(FORCE_EXIT_GRACE_SECS)) => {
                    info!("Graceful shutdown completed");
                    std::process::exit(0);
                }
            }
        }
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_creation() {
        let lm = LifecycleManager::new();
        let rx = lm.subscribe();
        assert!(!*rx.borrow());
    }

    #[test]
    fn shutdown_signal() {
        let lm = LifecycleManager::new();
        let rx = lm.subscribe();
        lm.shutdown();
        assert!(*rx.borrow());
    }
}
