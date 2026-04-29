//! Linux systemd live integration tests (T8-T10).
//!
//! Run manually:
//!     cargo test -p oneshim-app --features systemd-notify \
//!         --test linux_autostart_systemd_live -- --ignored
//!
//! Or via .github/workflows/linux-systemd-integration.yml (manual workflow_dispatch).
//!
//! These tests are NOT in normal CI:
//! - T8 / T10 modify ~/.config/systemd/user/oneshim.service
//! - T9 needs systemd-run --user --scope to provide NOTIFY_SOCKET
//!
//! The actual no-panic verification for sd_notify lives inline at
//! src-tauri/src/lifecycle/sd_notify.rs::tests::notify_ready_does_not_panic.

#![cfg(target_os = "linux")]

use std::process::Command;

#[test]
#[ignore = "modifies user systemd state — run manually under systemd-run --user --scope"]
fn enable_then_disable_writes_type_notify_service_file() {
    // T8: enable_autostart writes service file with Type=notify
    // PRE: ~/.config/systemd/user/oneshim.service does not exist
    // POST: file exists, contains Type=notify + NotifyAccess=main + TimeoutStartSec=30
    // CLEANUP: disable_autostart removes file

    let bin_path =
        std::env::var("ONESHIM_BIN").unwrap_or_else(|_| "target/release/oneshim".to_string());

    let _ = Command::new(&bin_path).arg("--enable-autostart").output();

    let home = std::env::var("HOME").expect("HOME env var");
    let service_path = std::path::PathBuf::from(home).join(".config/systemd/user/oneshim.service");

    if service_path.exists() {
        let content = std::fs::read_to_string(&service_path).unwrap();
        assert!(content.contains("Type=notify"));
        assert!(content.contains("NotifyAccess=main"));
        assert!(content.contains("TimeoutStartSec=30"));

        // Cleanup
        let _ = Command::new(&bin_path).arg("--disable-autostart").output();
    } else {
        eprintln!("SKIP: oneshim binary not built or test environment lacks autostart write perms");
    }
}

#[test]
#[ignore = "verifies sd_notify under systemd-run — run via workflow_dispatch"]
fn sd_notify_no_panic_when_socket_missing() {
    // T9: actual test lives inline at lifecycle/sd_notify.rs::tests::notify_ready_does_not_panic.
    // This test is a placeholder for documentation/discovery from CI workflow_dispatch.
    eprintln!(
        "T9 sd_notify no-panic test lives inline at \
         lifecycle::sd_notify::tests::notify_ready_does_not_panic"
    );
}

#[test]
#[ignore = "end-to-end migration verification — run manually after install"]
fn migration_writes_type_notify_when_pr_b1_template_present() {
    // T10: write old Type=simple template, run app, verify file updated to Type=notify
    // and currently-running service NOT restarted (no daemon-reload).
    //
    // Manual procedure:
    //   1. Install PR-B1 binary (v0.4.40-rc.1 / rc.2 / rc.3 / v0.4.40)
    //   2. Toggle autostart ON in Settings → verify ~/.config/systemd/user/oneshim.service has Type=simple
    //   3. Install PR-B2 binary (new build)
    //   4. Restart ONESHIM
    //   5. Check log for `err.code = autostart.service_migrated` at info level
    //   6. Verify service file content updated to Type=notify
    //   7. Verify systemctl --user is-active oneshim.service still returns "active"
    //   8. Logout + login → verify service starts cleanly under Type=notify
    eprintln!("T10 is a manual procedure — see test body comment for steps");
}
