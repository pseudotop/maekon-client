//! `ExternalGrpcSpawnConfig` — runtime config struct for the external gRPC server.
//!
//! Mirrors the `GrpcSpawnConfig` pattern in the loopback server:
//! custom `Debug` impl redacts all sensitive key material so config dumps
//! never leak cert bytes, JWT public-key PEM, or similar secrets.

use std::net::SocketAddr;
use std::sync::Arc;

use oneshim_api_contracts::stream::{AiRuntimeStatus, RealtimeEvent};
use oneshim_core::config::ExternalGrpcConfig;
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_core::ports::monitor::SystemMonitor;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use tokio::sync::{broadcast, watch};

use crate::storage_port::WebStorage;

use super::cert_resolver::HotReloadCertResolver;
use super::ip_ban::IpBan;
use super::jwt_verifier::JwtVerifier;
use super::live_config::LiveExternalConfig;
use super::metrics::ExternalMetrics;
use super::mtls_verifier::MtlsVerifier;

/// Runtime configuration for the external gRPC server.
///
/// All heavy state (storage, verifiers, cert resolver, etc.) is behind `Arc`
/// so this struct is `Clone` without deep copies.
#[derive(Clone)]
pub struct ExternalGrpcSpawnConfig {
    /// Socket address the server will bind to.
    pub bind_addr: SocketAddr,
    /// Static config (from `AppConfig.external_grpc`).
    pub config: ExternalGrpcConfig,
    /// Storage port (read-only queries from DashboardService).
    pub storage: Arc<dyn WebStorage>,
    /// System monitor port (for streaming metrics).
    pub system_monitor: Arc<dyn SystemMonitor>,
    /// Broadcast sender for realtime events.
    pub event_tx: broadcast::Sender<RealtimeEvent>,
    /// Audit log port — each authenticated request is emitted here.
    pub audit_port: Arc<dyn AuditLogPort>,
    /// Hot-reload TLS cert resolver (swapped atomically on cert rotation).
    pub cert_resolver: Arc<HotReloadCertResolver>,
    /// JWT verifier (present when `auth_mode` includes JWT).
    pub jwt_verifier: Option<Arc<JwtVerifier>>,
    /// mTLS verifier (present when `auth_mode` includes mTLS).
    pub mtls_verifier: Option<Arc<MtlsVerifier>>,
    /// IP ban tracker shared between accept loop and auth layer.
    pub ip_ban: Arc<IpBan>,
    /// In-process atomic counters (requests, auth failures, active streams).
    pub metrics: Arc<ExternalMetrics>,
    /// Shutdown signal receiver — cloned by cert watcher + expiry monitor tasks, and
    /// by `serve_external` for tonic graceful shutdown.
    ///
    /// When `true` is sent on the paired `shutdown_tx`, all three background tasks
    /// (cert watcher, expiry monitor, tonic server) break out of their loops cleanly.
    pub shutdown_rx: watch::Receiver<bool>,
    /// Shutdown signal sender — kept alive for the lifetime of `spawn_with_supervisor`.
    ///
    /// Dropping this `Arc` (when the last reference is released as the supervisor exits
    /// or gives up) closes the watch channel, which also unblocks any pending
    /// `shutdown_rx.changed()` calls with an `Err` — causing the watcher and expiry tasks
    /// to exit.
    pub shutdown_tx: Arc<watch::Sender<bool>>,
    /// PII sanitizer for `AiRuntimeStatus` fallback-reason fields. Passed through
    /// from loopback; the external `DashboardServiceImpl` uses it identically.
    pub pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    /// AiRuntimeStatus snapshot (build-time). Passed through from loopback.
    pub ai_runtime_status_snapshot: Option<AiRuntimeStatus>,
    /// Runtime-tunable config (streaming_enabled + load_policy). Atomic snapshot
    /// via ArcSwap — readers call `live.snapshot()` once per request entry.
    pub live: Arc<LiveExternalConfig>,
}

/// Custom `Debug` impl — redacts cert key material and verifier contents.
/// The `jwt_verifier` and `mtls_verifier` contain public-key bytes; we emit
/// boolean-presence flags only, mirroring `GrpcSpawnConfig`'s redaction of
/// `integration_auth_token`.
///
/// Takes a single `live.snapshot()` to print both `streaming_enabled_live` and
/// `load_policy_snapshot_summary` atomically — avoids torn reads within one Debug
/// print (though concurrent prints remain racy by design; documented and accepted).
impl std::fmt::Debug for ExternalGrpcSpawnConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snap = self.live.snapshot();
        let t = snap.load_policy.thresholds();
        f.debug_struct("ExternalGrpcSpawnConfig")
            .field("bind_addr", &self.bind_addr)
            .field("auth_mode", &self.config.auth_mode)
            .field(
                "max_concurrent_streams",
                &self.config.max_concurrent_streams,
            )
            .field("max_connections", &self.config.max_connections)
            .field("jwt_verifier_present", &self.jwt_verifier.is_some())
            .field("mtls_verifier_present", &self.mtls_verifier.is_some())
            .field("shutdown_signalled", &*self.shutdown_rx.borrow())
            .field("pii_sanitizer_present", &self.pii_sanitizer.is_some())
            .field(
                "ai_runtime_status_present",
                &self.ai_runtime_status_snapshot.is_some(),
            )
            .field("streaming_enabled_live", &snap.streaming_enabled)
            .field(
                "load_policy_snapshot_summary",
                &format_args!(
                    "cpu {:.0}/{:.0}/{:.0}, mem_gb {:.1}",
                    t.cpu_low_pct, t.cpu_medium_pct, t.cpu_high_pct, t.min_free_mem_gb
                ),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, feature = "test-support", feature = "external-grpc-tools"))]
mod tests {
    use super::*;
    use oneshim_core::config::LoadThresholds;
    use oneshim_core::models::ai_session::SessionAuditEntry;
    use oneshim_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
    use oneshim_storage::sqlite::SqliteStorage;

    use crate::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
    use crate::grpc::load_policy::LoadPolicy;
    use crate::grpc::test_support::mock_system_monitor::MockSystemMonitor;

    struct NoopAudit;
    #[async_trait::async_trait]
    impl AuditLogPort for NoopAudit {
        async fn pending_count(&self) -> usize {
            0
        }
        async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_status(&self, _status: &AuditStatus, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_action_prefix(&self, _prefix: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
            vec![]
        }
        async fn stats(&self) -> AuditStats {
            AuditStats::default()
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }
        async fn log_event(&self, _action_type: &str, _session_id: &str, _details: &str) {}
        async fn log_start_if(
            &self,
            _level: AuditLevel,
            _command_id: &str,
            _session_id: &str,
            _action_type: &str,
        ) {
        }
        async fn log_complete_with_time(
            &self,
            _level: AuditLevel,
            _command_id: &str,
            _session_id: &str,
            _details: &str,
            _execution_time_ms: u64,
        ) {
        }
        async fn drain_batch(&self) -> Vec<AuditEntry> {
            vec![]
        }
        async fn drain_all(&self) -> Vec<AuditEntry> {
            vec![]
        }
        async fn record_session_event(&self, _entry: SessionAuditEntry) {}
    }

    fn fixture_spawn_config(bind_addr: SocketAddr) -> ExternalGrpcSpawnConfig {
        use rcgen::{CertificateParams, KeyPair};

        let kp = KeyPair::generate().expect("keypair");
        let params = CertificateParams::new(vec!["localhost".into()]).expect("params");
        let cert = params.self_signed(&kp).expect("cert");
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der()).expect("key");
        let signing = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der).expect("sign");
        let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert_der], signing));
        let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

        let storage =
            Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite")) as Arc<dyn WebStorage>;
        let (event_tx, _) = broadcast::channel(16);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        ExternalGrpcSpawnConfig {
            bind_addr,
            config: ExternalGrpcConfig {
                enabled: true,
                auth_mode: Some(oneshim_core::config::AuthMode::Jwt),
                max_concurrent_streams: 4,
                max_connections: 16,
                ..Default::default()
            },
            storage,
            system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
            event_tx,
            audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
            cert_resolver,
            jwt_verifier: None,
            mtls_verifier: None,
            ip_ban: Arc::new(IpBan::new()),
            metrics: Arc::new(ExternalMetrics::new()),
            shutdown_rx,
            shutdown_tx: Arc::new(shutdown_tx),
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
                streaming_enabled: true,
                load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
            })),
        }
    }

    /// Verify `{:?}` does NOT contain cert key bytes, PEM strings, or JWT material.
    #[test]
    fn spawn_config_debug_redacts_sensitive_fields() {
        let bind_addr = "127.0.0.1:10092".parse().unwrap();
        let cfg = fixture_spawn_config(bind_addr);
        let dbg = format!("{:?}", cfg);

        // Must contain field indicators
        assert!(
            dbg.contains("jwt_verifier_present"),
            "Debug must show jwt_verifier_present; got: {dbg}"
        );
        assert!(
            dbg.contains("mtls_verifier_present"),
            "Debug must show mtls_verifier_present; got: {dbg}"
        );
        // Live config fields must appear
        assert!(
            dbg.contains("streaming_enabled_live"),
            "Debug must show live field: {dbg}"
        );
        assert!(
            dbg.contains("load_policy_snapshot_summary"),
            "Debug must show policy summary: {dbg}"
        );
        // Must NOT leak key material
        assert!(
            !dbg.contains("BEGIN"),
            "Debug must not leak PEM headers; got: {dbg}"
        );
        assert!(
            !dbg.contains("PRIVATE"),
            "Debug must not leak private key material; got: {dbg}"
        );
        // bind_addr must still appear
        assert!(
            dbg.contains("10092"),
            "bind_addr port should appear in Debug; got: {dbg}"
        );
    }

    /// Verify that Clone works (all Arcs clone shallow).
    #[test]
    fn spawn_config_clone_is_shallow() {
        let bind_addr = "127.0.0.1:10093".parse().unwrap();
        let cfg = fixture_spawn_config(bind_addr);
        let clone = cfg.clone();
        // Same Arc pointers — not deep copies.
        assert!(Arc::ptr_eq(&cfg.ip_ban, &clone.ip_ban));
        assert!(Arc::ptr_eq(&cfg.cert_resolver, &clone.cert_resolver));
        assert!(Arc::ptr_eq(&cfg.metrics, &clone.metrics));
        assert!(Arc::ptr_eq(&cfg.live, &clone.live));
    }
}
