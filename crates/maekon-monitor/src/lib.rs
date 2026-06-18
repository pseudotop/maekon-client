// Cast safety: system metrics, CPU percentages, process counters — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening.
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # maekon-monitor

pub mod error;
pub use error::MonitorError;

pub mod activity;
pub mod clipboard;
pub mod file_access;
pub mod idle;
pub mod input_activity;
pub mod input_detail;
pub mod key_hook;
pub mod keyboard_pattern;
pub mod process;
pub mod system;
pub mod system_info;
pub mod window_layout;

// Pure, cfg-free parsers for the Windows active-window FFI shim (#5120). Kept
// un-gated so the logic is unit-testable on any OS, not only a Windows runner.
mod active_window_parse;
// Cfg-free macOS `pmset -g batt` → PowerStatus parser (#5138), same rationale.
mod power_parse;
// Content-free log digest for window titles + suggestion content (#5591, #6006)
// + workspace-wide textual guard against raw-title/content tracing.
// Un-gated so the guard runs on every host OS.
pub mod log_privacy;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
mod macos_ax_ffi;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;
