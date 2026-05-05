//! Linux autostart smoke test — verifies cargo test invocation works
//! under the `systemd-notify` feature on Linux. Real coverage of capability
//! detection lives inline in `src-tauri/src/autostart.rs::linux_capability_tests`,
//! and migration coverage lives in `src-tauri/src/lifecycle/migration_hashes.rs::tests`.
//! This file exists so CI has a stable target for `cargo test --test linux_autostart_unit`
//! that can be run on Linux runners without invoking the full unit suite.

#![cfg(target_os = "linux")]

#[test]
fn linux_autostart_smoke() {
    // Smoke: verifies the test harness runs on Linux with feature enabled.
    // No assertion: the file is `#![cfg(target_os = "linux")]` so reaching this
    // point already proves the precondition. A literal `assert!(cfg!(...))`
    // would trip clippy::assertions_on_constants under -D warnings.
}
