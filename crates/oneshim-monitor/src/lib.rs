// Cast safety: system metrics, CPU percentages, process counters — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: see decision doc.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening.
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # oneshim-monitor

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

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;
