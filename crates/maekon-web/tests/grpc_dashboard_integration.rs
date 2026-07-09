//! D13-v2 integration tests: end-to-end gRPC dashboard server ↔ client.
//!
//! Spawns `serve_optional()` on an ephemeral port, connects a tonic client,
//! exercises each RPC, and verifies the wire protocol + service
//! registration + port wiring end-to-end.
//!
//! Feature-gated by `grpc-dashboard,test-support` — the entire file compiles
//! away unless the production gRPC surface and its mock support are both
//! enabled.
//!
//! # Layout (#7730)
//!
//! This used to be a single ~1,900-line file. It is now a thin crate root
//! that wires together scenario-family modules under `tests/grpc_dashboard/`
//! via `#[path]` — the SAME single test binary (`--test
//! grpc_dashboard_integration`) as before, just navigable by scenario:
//!
//! - [`common`] — shared server/config helpers (not itself a suite).
//! - [`core_rpc`] — GetAgentInfo, local-auth gate, HealthCheck, sequential
//!   call stability.
//! - [`query_rpc`] — GetSessionStats / GetRecentFrames /
//!   GetProductivityMetrics / GetFocusStats (empty-DB + seeded-aggregation).
//! - [`subscribe_metrics`] — B2-10 SubscribeMetrics streaming (8 tests).
//! - [`failing_storage`] — `FailingStorage` test double (pre-existing
//!   `tests/support/` companion file — unchanged by this split).
//! - [`subscribe_events`] — B3-7 SubscribeEvents streaming (12 tests + an
//!   HTTP/2 keepalive source check).
//!
//! `tests/grpc_dashboard/*.rs` are plain modules (not `main.rs`-rooted), so
//! cargo does NOT auto-discover them as separate test binaries — this
//! mirrors the pre-existing `tests/support/failing_storage.rs` mechanism.

#![cfg(all(feature = "grpc-dashboard", feature = "test-support"))]

#[path = "grpc_dashboard/common.rs"]
mod common;
#[path = "grpc_dashboard/core_rpc.rs"]
mod core_rpc;
#[path = "grpc_dashboard/query_rpc.rs"]
mod query_rpc;
#[path = "grpc_dashboard/subscribe_metrics.rs"]
mod subscribe_metrics;

// ── B3-7: FailingStorage test harness ────────────────────────────────────
// Declared at the crate root (unchanged from the pre-split layout) so its
// `#[path]` stays resolvable relative to `tests/` regardless of which
// scenario-family module references `crate::failing_storage`.
#[cfg(feature = "grpc-dashboard")]
#[path = "support/failing_storage.rs"]
mod failing_storage;

#[path = "grpc_dashboard/subscribe_events.rs"]
mod subscribe_events;
