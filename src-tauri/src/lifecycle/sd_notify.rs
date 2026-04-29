//! systemd Type=notify integration.
//!
//! No-op on non-Linux platforms or when `systemd-notify` feature disabled.
//! When run outside systemd (e.g., `cargo run`, manual launch), `sd_notify::notify`
//! returns Err which we log at debug — no user-visible impact.

#[cfg(all(target_os = "linux", feature = "systemd-notify"))]
pub fn notify_ready() {
    use oneshim_core::error_codes::AutostartCode;
    if let Err(e) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
        tracing::debug!(
            err.code = AutostartCode::SdNotifySkipped.as_str(),
            "sd_notify READY skipped (not run under systemd): {e}"
        );
    }
}

#[cfg(not(all(target_os = "linux", feature = "systemd-notify")))]
pub fn notify_ready() {
    // No-op on non-Linux or when systemd-notify feature disabled.
}

#[cfg(all(target_os = "linux", feature = "systemd-notify"))]
#[allow(dead_code)] // no caller yet; will be wired in shutdown path
pub fn notify_stopping() {
    let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
}

#[cfg(not(all(target_os = "linux", feature = "systemd-notify")))]
#[allow(dead_code)] // no caller on non-Linux yet; will be wired in shutdown path
pub fn notify_stopping() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_ready_does_not_panic() {
        // Whether feature enabled or not, this must not panic
        notify_ready();
    }

    #[test]
    fn notify_stopping_does_not_panic() {
        notify_stopping();
    }
}
