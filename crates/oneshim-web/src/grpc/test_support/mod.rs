//! D13-v2b dashboard gRPC — test-only helpers shared across unit + integration
//! tests (and reused by PR-B3 tests once it lands).
//!
//! Gated on `#[cfg(any(test, feature = "test-support"))]` — the `test-support`
//! feature is strictly opt-in (NEVER enabled by default or transitively via
//! `grpc-dashboard`). Integration tests must invoke with
//! `--features grpc-dashboard,test-support` per spec §8 #4.
//!
//! Smoke: `cargo build --no-default-features --features grpc-dashboard`
//! succeeds and the built binary contains NONE of these helpers (IMP-V2-D).

pub mod mock_system_monitor;
