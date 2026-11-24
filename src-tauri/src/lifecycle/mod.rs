pub mod autostart_migration;
pub mod migration_hashes;
pub mod sd_notify;

use tokio::sync::watch;
use tracing::{debug, info, warn};

/// What the OS signal bridge must do once [`LifecycleManager::wait_for_signal`]
/// stops waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalOutcome {
    /// The signal listener could not be installed, so graceful shutdown was
    /// requested instead (#12071).
    ListenerFailed,
    /// A second signal arrived while the app was quitting: exit now.
    ForceExit,
}

pub struct LifecycleManager {
    shutdown_tx: watch::Sender<bool>,
    // Only `shutdown_tx` is used internally (`shutdown()` sends on it, called
    // from `wait_for_signal()`). This receiver backs `subscribe()` for a
    // caller that wants to observe LifecycleManager's own OS-signal-driven
    // shutdown; `runtime_bridges.rs`'s current integration instead re-signals
    // the app-wide `AppState.shutdown_tx` directly, so neither is used today.
    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    pub fn shutdown(&self) {
        info!("ended sent");
        if let Err(e) = self.shutdown_tx.send(true) {
            debug!("channel send failed: {e}");
        }
    }

    /// Waits for SIGINT/SIGTERM (Ctrl+C off Unix) and hands the first one to
    /// `request_app_exit`; only a second signal returns [`SignalOutcome::ForceExit`].
    ///
    /// #13933: the first signal used to start a 3 s timer that called
    /// `std::process::exit(0)` on this tokio worker. libc then ran the atexit
    /// handlers and shared-library destructors (Mesa's included) on the worker
    /// while the GTK main thread kept rendering. Install evidence run
    /// 43810494224 logged `Graceful shutdown completed` and, 0.15 s later, a
    /// main-thread segfault in libgallium. The timer also skipped the app's
    /// quit path (`RunEvent::Exit`). `request_app_exit` must hand the exit to
    /// the main thread instead; no timer ends the process from here.
    pub async fn wait_for_signal(&self, request_app_exit: impl FnOnce()) -> SignalOutcome {
        self.wait_for_signal_with(
            self.wait_first_signal(),
            Self::wait_second_signal(),
            request_app_exit,
        )
        .await
    }

    async fn wait_for_signal_with(
        &self,
        first_signal: impl std::future::Future<Output = std::io::Result<()>>,
        second_signal: impl std::future::Future<Output = SignalOutcome>,
        request_app_exit: impl FnOnce(),
    ) -> SignalOutcome {
        if let Err(error) = first_signal.await {
            warn!("Signal listener failed; requesting graceful shutdown: {error}");
            self.shutdown();
            return SignalOutcome::ListenerFailed;
        }
        info!("Shutting down gracefully... press Ctrl+C again to force exit");
        self.shutdown();
        request_app_exit();

        // Wait for second signal → force exit
        second_signal.await
    }

    async fn wait_first_signal(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigint = signal(SignalKind::interrupt())?;
            let mut sigterm = signal(SignalKind::terminate())?;

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
            tokio::signal::ctrl_c().await?;
            info!("Ctrl+C received");
        }
        Ok(())
    }

    /// Resolves only when a second signal arrives. The app is already quitting
    /// through `request_app_exit`, so a listener that cannot be installed just
    /// means there is no forced exit.
    async fn wait_second_signal() -> SignalOutcome {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let (mut sigint, mut sigterm) = match (
                signal(SignalKind::interrupt()),
                signal(SignalKind::terminate()),
            ) {
                (Ok(sigint), Ok(sigterm)) => (sigint, sigterm),
                (Err(error), _) | (_, Err(error)) => {
                    warn!(
                        "Second-signal listener unavailable; waiting for the app to quit: {error}"
                    );
                    return std::future::pending().await;
                }
            };

            tokio::select! {
                _ = sigint.recv() => {
                    warn!("Second SIGINT received — force exit");
                }
                _ = sigterm.recv() => {
                    warn!("SIGTERM received during shutdown — force exit");
                }
            }
        }

        #[cfg(not(unix))]
        {
            if let Err(error) = tokio::signal::ctrl_c().await {
                warn!("Second-signal listener unavailable; waiting for the app to quit: {error}");
                return std::future::pending().await;
            }
            warn!("Second Ctrl+C received — force exit");
        }
        SignalOutcome::ForceExit
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    #[tokio::test]
    async fn first_signal_stays_pending_until_a_signal_arrives() {
        let manager = LifecycleManager::new();
        let first_signal = manager.wait_first_signal();
        tokio::pin!(first_signal);

        let first_poll = std::future::poll_fn(|context| {
            std::task::Poll::Ready(std::future::Future::poll(first_signal.as_mut(), context))
        })
        .await;

        assert!(
            first_poll.is_pending(),
            "listener completed without a signal: {first_poll:?}"
        );
    }

    #[tokio::test]
    async fn listener_failure_requests_shutdown_without_waiting_for_another_signal() {
        let manager = LifecycleManager::new();
        let receiver = manager.subscribe();
        let failure = async {
            Err(std::io::Error::other(
                "synthetic signal registration failure",
            ))
        };
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            manager.wait_for_signal_with(failure, std::future::pending::<SignalOutcome>(), || {
                panic!("a failed listener received no signal, so it must not quit the app")
            }),
        )
        .await
        .expect("listener failure must not wait for a second signal");
        assert_eq!(outcome, SignalOutcome::ListenerFailed);
        assert!(
            *receiver.borrow(),
            "listeners must observe the shutdown request"
        );
    }

    /// Runs `body` in a child copy of this test binary. The #13933 timer ended
    /// the process with `std::process::exit(0)`; inside this binary that stops
    /// the whole run with status 0, which cargo reports as a pass. The parent
    /// therefore also requires the child's own libtest summary.
    fn in_child_test_process(test_name: &str, body: impl FnOnce()) {
        const CHILD: &str = "MAEKON_LIFECYCLE_TEST_CHILD";
        if std::env::var(CHILD).as_deref() == Ok(test_name) {
            body();
            return;
        }
        let filter = format!("lifecycle::tests::{test_name}");
        let output = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([filter.as_str(), "--exact", "--nocapture"])
            .env(CHILD, test_name)
            .output()
            .expect("child test process");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "the child must finish its test instead of ending the process ({})\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn paused_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .expect("paused test runtime")
    }

    /// #13933: the old 3 s grace ended the process from this worker. Here no
    /// second signal ever arrives, so after the single exit request nothing may
    /// end the wait, however long the app takes to quit.
    #[test]
    fn first_signal_requests_app_exit_once_and_no_timer_ends_the_wait() {
        in_child_test_process(
            "first_signal_requests_app_exit_once_and_no_timer_ends_the_wait",
            || {
                let manager = LifecycleManager::new();
                let receiver = manager.subscribe();
                let requests = Arc::new(AtomicUsize::new(0));
                let counter = Arc::clone(&requests);

                let waiting = paused_runtime().block_on(async {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        manager.wait_for_signal_with(
                            async { Ok(()) },
                            std::future::pending::<SignalOutcome>(),
                            move || {
                                counter.fetch_add(1, Ordering::SeqCst);
                            },
                        ),
                    )
                    .await
                });

                let _elapsed: tokio::time::error::Elapsed =
                    waiting.expect_err("only a second signal may end the wait");
                assert_eq!(requests.load(Ordering::SeqCst), 1);
                assert!(*receiver.borrow(), "shutdown must still be announced");
            },
        );
    }

    #[tokio::test]
    async fn second_signal_forces_exit_after_the_app_exit_request() {
        let manager = LifecycleManager::new();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        let observed = Arc::clone(&requests);

        let outcome = manager
            .wait_for_signal_with(
                async { Ok(()) },
                async move {
                    assert_eq!(
                        observed.load(Ordering::SeqCst),
                        1,
                        "the app exit request must precede the second-signal wait"
                    );
                    SignalOutcome::ForceExit
                },
                move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                },
            )
            .await;

        assert_eq!(outcome, SignalOutcome::ForceExit);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    /// #13933: the real second-signal listener has no timer either.
    #[test]
    fn second_signal_listener_has_no_timed_exit() {
        in_child_test_process("second_signal_listener_has_no_timed_exit", || {
            let waiting = paused_runtime().block_on(async {
                tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    LifecycleManager::wait_second_signal(),
                )
                .await
            });

            let _elapsed: tokio::time::error::Elapsed =
                waiting.expect_err("no second signal arrived, so nothing may end the wait");
        });
    }
}
