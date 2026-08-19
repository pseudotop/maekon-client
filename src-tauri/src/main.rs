#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Thin composition-root binary shim (Tauri v2 standard shape).
//!
//! All application logic lives in the `maekon_app` library crate
//! (`src/lib.rs`); this binary only carries the Windows-subsystem attribute
//! (bin-only semantics) and delegates immediately to `maekon_app::run()`.

fn main() {
    maekon_app::run();
}
