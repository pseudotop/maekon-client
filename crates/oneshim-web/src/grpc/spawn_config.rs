//! D13-v2b dashboard gRPC — `GrpcSpawnConfig` struct + custom Debug redaction.
//!
//! Replaces the v2a positional `(port, storage)` args on `serve`/`serve_optional`
//! with a named-field struct. Extensibility: future v2c can add fields without
//! breaking call sites. Custom `Debug` impl redacts `integration_auth_token`
//! so a logged-config dump never leaks the token value (spec IMP-16).

use std::sync::Arc;

use oneshim_api_contracts::stream::{AiRuntimeStatus, RealtimeEvent};
use oneshim_core::ports::monitor::SystemMonitor;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use tokio::sync::broadcast;

use crate::storage_port::WebStorage;

use super::load_policy::LoadPolicy;

pub struct GrpcSpawnConfig {
    pub port: u16,
    pub storage: Arc<dyn WebStorage>,
    pub system_monitor: Arc<dyn SystemMonitor>,
    pub event_tx: broadcast::Sender<RealtimeEvent>,
    /// Forward-compat trust signal; under v2b's loopback-only bind the
    /// token branch of `honor_opt_out` is unreachable (see spec §4.3).
    pub integration_auth_token: Option<String>,
    /// PII sanitisation port; applied to AiRuntimeStatus.*_fallback_reason
    /// before snapshot emission on SubscribeEvents. `None` → pass-through
    /// (acceptable for test builds; prod wiring always sets Some).
    pub pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    /// AiRuntimeStatus snapshot captured at server build-time. Emitted
    /// exactly once per SubscribeEvents subscription (§A A2). `None` →
    /// sentinel "unknown" emission (§A C2).
    pub ai_runtime_status_snapshot: Option<AiRuntimeStatus>,
    pub load_policy: Arc<LoadPolicy>,
    /// When false, SubscribeMetrics / SubscribeEvents return
    /// `Status::unavailable("streaming disabled")`. Unary v1/v2a RPCs unaffected.
    pub streaming_enabled: bool,
    /// Cap on concurrent streaming subscribers (global across both RPCs).
    pub max_concurrent_streams: usize,
}

// IMP-16: redact `integration_auth_token` so config dumps never leak the value.
// B3-0: also redact `pii_sanitizer` and `ai_runtime_status_snapshot` (boolean-only).
impl std::fmt::Debug for GrpcSpawnConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcSpawnConfig")
            .field("port", &self.port)
            .field(
                "integration_auth_token",
                &format_args!(
                    "[REDACTED; present={}]",
                    self.integration_auth_token.is_some()
                ),
            )
            .field("pii_sanitizer_present", &self.pii_sanitizer.is_some())
            .field(
                "ai_runtime_status_present",
                &self.ai_runtime_status_snapshot.is_some(),
            )
            .field("streaming_enabled", &self.streaming_enabled)
            .field("max_concurrent_streams", &self.max_concurrent_streams)
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::grpc::test_support::mock_system_monitor::MockSystemMonitor;
    use oneshim_core::config::LoadThresholds;
    use oneshim_storage::sqlite::SqliteStorage;

    #[tokio::test]
    async fn grpc_spawn_config_debug_redacts_token() {
        let storage =
            Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite")) as Arc<dyn WebStorage>;
        let (event_tx, _) = broadcast::channel(16);
        let cfg = GrpcSpawnConfig {
            port: 10080,
            storage,
            system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
            event_tx,
            integration_auth_token: Some("super-secret-token-xyz".to_string()),
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
            streaming_enabled: true,
            max_concurrent_streams: 50,
        };
        let dbg = format!("{:?}", cfg);
        assert!(
            !dbg.contains("super-secret-token-xyz"),
            "Debug impl must NOT leak the token; got: {dbg}"
        );
        assert!(
            dbg.contains("REDACTED"),
            "Debug impl should mark the token as redacted; got: {dbg}"
        );
    }
}
