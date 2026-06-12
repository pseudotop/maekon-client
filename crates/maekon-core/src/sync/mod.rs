//! Cross-device sync primitives.
//!
//! Provides Hybrid Logical Clock (HLC) for causal ordering across devices.
//! Public behavior: docs/guides/sync-conflict-resolution.md.

mod hlc;

pub use hlc::Hlc;
