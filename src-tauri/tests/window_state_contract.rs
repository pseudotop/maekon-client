//! Window state persistence contract for CRT-PRV-WIN-004.
//!
//! This test intentionally stays at the source-contract layer: the native
//! restart evidence is produced by the private macOS harness, while this keeps
//! the product wiring from regressing silently.

use std::fs;
use std::path::PathBuf;

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn crt_prv_win_004_main_window_state_persistence_is_wired() {
    let module_path = src_dir().join("window_state.rs");
    assert!(
        module_path.exists(),
        "CRT-PRV-WIN-004 requires a window_state module"
    );

    let module_src = fs::read_to_string(&module_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", module_path.display()));
    assert!(
        module_src.contains("restore_main_window_state"),
        "window_state module must expose restore_main_window_state"
    );
    assert!(
        module_src.contains("persist_main_window_state"),
        "window_state module must expose persist_main_window_state"
    );

    let startup_src = fs::read_to_string(src_dir().join("desktop_startup.rs"))
        .expect("desktop_startup.rs must be readable");
    assert!(
        startup_src.contains("restore_main_window_state"),
        "desktop startup must restore main window geometry before showing the window"
    );

    let main_src = fs::read_to_string(src_dir().join("lib.rs")).expect("lib.rs must be readable");
    assert!(
        main_src.contains("persist_main_window_state"),
        "window events must persist main window geometry"
    );
}
