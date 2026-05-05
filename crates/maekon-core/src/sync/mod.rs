//! Cross-device sync primitives.
//!
//! Provides Hybrid Logical Clock (HLC) for causal ordering across devices.
//! See: docs/superpowers/specs/2026-03-19-p3-cross-device-sync-design.md

mod hlc;

pub use hlc::Hlc;
