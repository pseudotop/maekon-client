use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::runtime_state::{EmbeddingRuntimeState, SceneFinderSlot, SyncRuntimeState};

/// Shared connection/health flags plus the cross-loop runtime slots created once
/// at the composition root. Extracted from `build_and_spawn` so the launch
/// module stays a small composition root (ADR-013). Every field is threaded
/// verbatim into the agent runtime, the scheduler health loop, and the managed
/// IPC state — grouping their creation here does not change any behaviour.
pub(super) struct RuntimeFlagsWiring {
    /// Connection status flags — start OPTIMISTIC (healthy), then the health
    /// check loop mirrors the adapter health flags into them every tick. They
    /// match the adapter flags' optimistic default so the tray renders
    /// "connected" at first paint and only flips to "disconnected" once a real
    /// adapter records an observed failure (#8050) — never the reverse, which
    /// was the permanent-disconnected bug (dead adapter writers left these stuck
    /// at `false` forever).
    pub(super) server_connected: Arc<AtomicBool>,
    pub(super) llm_connected: Arc<AtomicBool>,
    pub(super) cli_connected: Arc<AtomicBool>,
    /// Adapter health flags feed the connection-status health loop. They start
    /// OPTIMISTIC (`true` = "no failure observed yet"), matching the
    /// `analysis_health_flag` precedent: a subsystem is reported healthy until
    /// one of its real adapters records an observed failure, and returns to
    /// healthy on the next observed success. This collapses a tri-state
    /// (unknown / up / down) onto a bool where "never exercised" reads as
    /// healthy — so an idle CLI bridge or a chat provider that has not been used
    /// yet no longer drags the tray into a permanent "disconnected" state
    /// (#8050). Writers: server → heartbeat/upload loops
    /// (scheduler/loops/network.rs); llm → the AuditingSession send decorator
    /// (auditing_session.rs); cli → the automation controller command result
    /// (maekon-automation controller/preset.rs).
    pub(super) server_health_flag: Arc<AtomicBool>,
    pub(super) llm_health_flag: Arc<AtomicBool>,
    pub(super) cli_health_flag: Arc<AtomicBool>,
    /// Analysis health starts optimistic and flips on first primary failure.
    pub(super) analysis_health_flag: Arc<AtomicBool>,
    /// D7 (#4812 / E20-20): single shared workspace-wide circuit-breaker registry
    /// threaded into every network adapter (agent, session, automation).
    pub(super) breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    /// #6264: cross-device-sync IPC state whose shared write-once slot is threaded
    /// into the agent runtime (which builds the SyncEngine asynchronously) AND
    /// registered as managed state — both observe the same `Arc<OnceLock<..>>`.
    pub(super) sync_runtime_state: SyncRuntimeState,
    /// #6266: same shared-slot pattern for the reloadable embedding model.
    pub(super) embedding_runtime_state: EmbeddingRuntimeState,
    /// Shared write-once slot for the automation scene finder, populated after
    /// the automation controller builds (post scheduler startup).
    pub(super) scene_finder_slot: SceneFinderSlot,
}

/// Build the shared connection/health flags and cross-loop runtime slots.
pub(super) fn build_runtime_flags() -> RuntimeFlagsWiring {
    RuntimeFlagsWiring {
        server_connected: Arc::new(AtomicBool::new(true)),
        llm_connected: Arc::new(AtomicBool::new(true)),
        cli_connected: Arc::new(AtomicBool::new(true)),
        server_health_flag: Arc::new(AtomicBool::new(true)),
        llm_health_flag: Arc::new(AtomicBool::new(true)),
        cli_health_flag: Arc::new(AtomicBool::new(true)),
        analysis_health_flag: Arc::new(AtomicBool::new(true)),
        breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
        sync_runtime_state: SyncRuntimeState::default(),
        embedding_runtime_state: EmbeddingRuntimeState::default(),
        scene_finder_slot: Arc::new(std::sync::OnceLock::new()),
    }
}
