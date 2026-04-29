//! Single-instance integration smoke test.
//!
//! Spawns the binary as a child process; expects exit code 0 within 2 seconds
//! when 1st instance is already running.
//!
//! Run with: `cargo test -p oneshim-app --test single_instance_integration -- --ignored`
//!
//! Skipped by default because it requires the binary to be built and may
//! interfere with a running Maekon instance on the developer's machine.

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
#[ignore = "spawns Maekon binary - run manually after build"]
fn second_instance_exits_cleanly_within_2s() {
    // PRE-CONDITION: 1st instance must be running before this test.
    // This test is informational — verifies 2nd instance can spawn + exit
    // without hanging or panicking.

    // Binary name = "oneshim" per [[bin]] in src-tauri/Cargo.toml
    let bin_path =
        std::env::var("ONESHIM_BIN").unwrap_or_else(|_| "target/release/oneshim".to_string());

    let start = Instant::now();
    let mut child = Command::new(&bin_path)
        .arg("--single-instance-test")
        .spawn()
        .expect("failed to spawn 2nd instance");

    let timeout = Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = start.elapsed();
                assert!(
                    elapsed < timeout,
                    "2nd instance took {:?} to exit (expected <{:?})",
                    elapsed,
                    timeout
                );
                assert!(status.success(), "2nd instance exit code != 0: {status:?}");
                return;
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    panic!("2nd instance did not exit within {timeout:?}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("error polling child: {e}"),
        }
    }
}
