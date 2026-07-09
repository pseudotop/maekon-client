//! D13-v2c end-to-end integration tests for the external gRPC server.
//!
//! Each test spins up a full `serve_external` instance on an ephemeral port,
//! connects a tonic TLS client (with the self-signed server cert as CA), and
//! exercises the auth matrix. The server runs the real `DashboardServiceImpl`
//! (wired in Task 13) with `integration_auth_token: None`; a successful auth
//! handshake is therefore proven by an `Ok(AgentInfoResponse)` carrying a
//! non-empty `build_profile`.
//!
//! Feature gate: requires `grpc-dashboard-external,external-grpc-tools,test-support`.
//!
//! # Layout (#7730)
//!
//! This used to be a single ~3,900-line file. It is now a thin crate root
//! that wires together scenario-family modules under `tests/external_grpc/`
//! via `#[path]` — the SAME single test binary (`--test
//! external_grpc_integration`) as before, just navigable by scenario:
//!
//! - [`common`] — shared server/config/harness helpers (not itself a suite).
//! - [`auth_matrix`] — JWT / mTLS / combined auth modes, IP ban, cert
//!   hot-reload, short-lived cert rejection, loopback isolation,
//!   concurrent-stream cap, port collision, shutdown drain.
//! - [`request_id`] — D14 `RequestIdLayer` header correlation.
//! - [`audit_trail`] — `AuditLayer` Started/Completed pairing + Task 9.2
//!   handler-returned-status mapping via a fixture `DashboardService`.
//! - [`live_reload`] — D33 / Task 9.4 live-config-reload convergence.
//! - [`streaming_fallback`] — D22 streaming-enabled fallback semantics.
//!
//! `tests/external_grpc/*.rs` are plain modules (not `main.rs`-rooted), so
//! cargo does NOT auto-discover them as separate test binaries — this
//! mirrors the existing `tests/support/failing_storage.rs` pattern already
//! used by `grpc_dashboard_integration.rs`.

#![cfg(all(
    feature = "grpc-dashboard-external",
    feature = "external-grpc-tools",
    feature = "test-support"
))]
// Integration test binary (`tests/*.rs` is its own crate root, entirely
// test-only) — not covered by the library's `#[cfg_attr(test, allow(...))]`
// (#7719 `significant_drop_tightening` workspace enforcement).
#![allow(clippy::significant_drop_tightening)]

// #7730: `in_memory_storage()` lives under `tests/support/` (not the
// `maekon-web` library) because it needs `maekon-storage`, an adapter crate
// — see that file's doc comment + `scripts/check-crate-boundaries.sh`.
#[path = "support/in_memory_storage.rs"]
mod in_memory_storage_support;

#[path = "external_grpc/audit_trail.rs"]
mod audit_trail;
#[path = "external_grpc/auth_matrix.rs"]
mod auth_matrix;
#[path = "external_grpc/common.rs"]
mod common;
#[path = "external_grpc/live_reload.rs"]
mod live_reload;
#[path = "external_grpc/request_id.rs"]
mod request_id;
#[path = "external_grpc/streaming_fallback.rs"]
mod streaming_fallback;
