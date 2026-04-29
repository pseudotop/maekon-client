//! API contracts for the external gRPC introspection endpoints.
//!
//! Covers `GET /api/external-grpc/live-config` (spec §5.11 / D29).

use serde::{Deserialize, Serialize};

/// Flattened view of the `LoadPolicy` thresholds for JSON serialisation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadPolicyView {
    pub cpu_low_pct: f32,
    pub cpu_medium_pct: f32,
    pub cpu_high_pct: f32,
    pub min_free_mem_gb: f32,
    /// Milliseconds since the LoadPolicy (and thus the external gRPC server) started.
    pub started_at_elapsed_ms: u64,
    /// True during the 30-second warm-up window after server start.
    pub in_warmup: bool,
}

/// Response body for `GET /api/external-grpc/live-config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveConfigResponse {
    pub streaming_enabled: bool,
    pub load_policy_snapshot: LoadPolicyView,
    /// True once `ConfigReloadTask` has entered its main loop.
    pub config_reload_task_alive: bool,
}
