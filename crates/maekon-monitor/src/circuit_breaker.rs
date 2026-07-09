//! Compatibility re-export (#7720 E6 consolidation).
//!
//! The tick-based consecutive-failure `CircuitBreaker` struct (#6828) that
//! backs the subprocess-spawn guards in `linux.rs`, `x11_active_window.rs`,
//! and `macos.rs` now lives in `maekon-core` (std atomics only, `cfg`-free) so
//! `maekon-vision`'s platform accessibility guards can share the same
//! implementation instead of hand-rolling per-site copies. This module keeps
//! the `crate::circuit_breaker::CircuitBreaker` path stable for every existing
//! call site in this crate.

pub(crate) use maekon_core::circuit_breaker::CircuitBreaker;
