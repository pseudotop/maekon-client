//! ADR-013: gRPC DashboardService RPC handler modules.
//!
//! Each submodule owns the business logic for one RPC group. The `mod.rs`
//! `impl DashboardService` block stays in the parent `grpc/mod.rs` as thin
//! dispatchers — this is the only layout Rust's trait-impl coherence allows.

pub(super) mod agent_info;
pub(super) mod focus;
pub(super) mod frames;
pub(super) mod productivity;
pub(super) mod session_stats;
