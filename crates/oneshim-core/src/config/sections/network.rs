// 네트워크 연결 설정 — 서버/gRPC/TLS/Web 설정 모음
use serde::{Deserialize, Serialize};

// ── TlsConfig ──────────────────────────────────────────────────────

/// TLS 연결 설정 — 아웃바운드 HTTP/SSE 연결 보안 정책
///
/// 기본값: enabled=true (TLS 강제), allow_self_signed=false (운영 환경 표준).
/// 개발 환경에서는 allow_self_signed=true 또는 enabled=false 로 설정 가능.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    /// TLS 강제 여부 — false 시 http:// 연결 허용 (개발 전용)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 자체 서명 인증서 허용 — 운영 환경에서는 반드시 false 유지
    #[serde(default)]
    pub allow_self_signed: bool,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_self_signed: false,
        }
    }
}

// ── ServerConfig ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub base_url: String,
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "default_sse_max_retry_secs")]
    pub sse_max_retry_secs: u64,
}

// ── GrpcConfig ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcConfig {
    #[serde(default)]
    pub use_grpc_auth: bool,
    #[serde(default)]
    pub use_grpc_context: bool,
    #[serde(default = "default_grpc_endpoint")]
    pub grpc_endpoint: String,
    #[serde(default = "default_grpc_fallback_ports")]
    pub grpc_fallback_ports: Vec<u16>,
    #[serde(default = "default_grpc_connect_timeout")]
    pub connect_timeout_secs: u64,
    #[serde(default = "default_grpc_request_timeout")]
    pub request_timeout_secs: u64,
    #[serde(default)]
    pub use_tls: bool,

    #[serde(default)]
    pub mtls_enabled: bool,
    #[serde(default)]
    pub tls_domain_name: Option<String>,
    #[serde(default)]
    pub tls_ca_cert_path: Option<String>,
    #[serde(default)]
    pub tls_client_cert_path: Option<String>,
    #[serde(default)]
    pub tls_client_key_path: Option<String>,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            use_grpc_auth: false,
            use_grpc_context: false,
            grpc_endpoint: default_grpc_endpoint(),
            grpc_fallback_ports: default_grpc_fallback_ports(),
            connect_timeout_secs: default_grpc_connect_timeout(),
            request_timeout_secs: default_grpc_request_timeout(),
            use_tls: false,
            mtls_enabled: false,
            tls_domain_name: None,
            tls_ca_cert_path: None,
            tls_client_cert_path: None,
            tls_client_key_path: None,
        }
    }
}

// ── WebConfig ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_web_enabled")]
    pub enabled: bool,
    #[serde(default = "default_web_port")]
    pub port: u16,
    #[serde(default)]
    pub allow_external: bool,
    #[serde(default)]
    pub integration_auth_token: Option<String>,
    /// Dedicated port for the loopback gRPC Dashboard server.
    ///
    /// `0` means "use the default". `ONESHIM_DASHBOARD_GRPC_PORT` can still
    /// override this at runtime for ops/CI.
    #[serde(default = "default_grpc_dashboard_port")]
    pub grpc_port: u16,
    /// D13-v2b: gRPC dashboard streaming LoadPolicy thresholds. None = defaults.
    #[serde(default)]
    pub grpc_load_thresholds: Option<LoadThresholds>,
    /// D13-v2b: runtime kill switch for SubscribeMetrics / SubscribeEvents.
    /// false → RPCs return `Status::unavailable("streaming disabled")`. v2a RPCs unaffected.
    #[serde(default = "default_true")]
    pub grpc_streaming_enabled: bool,
    /// D13-v2b: maximum concurrent streaming subscribers (global across both RPCs).
    /// Prevents DoS via subscription flood. Exceeded requests get
    /// `Status::resource_exhausted`.
    #[serde(default = "default_max_concurrent_streams")]
    pub grpc_max_concurrent_streams: usize,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: default_web_enabled(),
            port: default_web_port(),
            allow_external: false,
            integration_auth_token: None,
            grpc_port: default_grpc_dashboard_port(),
            grpc_load_thresholds: None,
            grpc_streaming_enabled: true,
            grpc_max_concurrent_streams: default_max_concurrent_streams(),
        }
    }
}

// ── LoadThresholds (D13-v2b) ───────────────────────────────────────

/// Thresholds for `oneshim-web::grpc::LoadPolicy` CPU%/memory-GiB classification.
///
/// Validation: `cpu_low_pct < cpu_medium_pct < cpu_high_pct <= 100.0`. Enforced
/// at `LoadPolicy::new` construction. Invalid combinations caught at startup, not
/// here at deserialization — malformed configs should produce a runtime panic
/// rather than silently fall through to defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadThresholds {
    #[serde(default = "default_min_free_mem_gb")]
    pub min_free_mem_gb: f32,
    #[serde(default = "default_cpu_low_pct")]
    pub cpu_low_pct: f32,
    #[serde(default = "default_cpu_medium_pct")]
    pub cpu_medium_pct: f32,
    #[serde(default = "default_cpu_high_pct")]
    pub cpu_high_pct: f32,
}

impl Default for LoadThresholds {
    fn default() -> Self {
        Self {
            min_free_mem_gb: default_min_free_mem_gb(),
            cpu_low_pct: default_cpu_low_pct(),
            cpu_medium_pct: default_cpu_medium_pct(),
            cpu_high_pct: default_cpu_high_pct(),
        }
    }
}

// ── Default / helper functions (pub(super) — config/mod.rs 에서 사용) ─

pub(crate) fn default_request_timeout_ms() -> u64 {
    30_000
}

pub(crate) fn default_sse_max_retry_secs() -> u64 {
    30
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_grpc_endpoint() -> String {
    "http://localhost:50051".to_string()
}

fn default_grpc_fallback_ports() -> Vec<u16> {
    vec![50052, 50053]
}

fn default_grpc_connect_timeout() -> u64 {
    10
}

fn default_grpc_request_timeout() -> u64 {
    30
}

/// 로컬 WebServer 기본 포트 — IANA Dynamic/Ephemeral 대역 (49152-65535)
///
/// 9090 등 Well-Known/Registered Port 대역은 Prometheus, Cockpit 등과 충돌 가능.
/// 10090 은 IANA 미등록 Registered Port 대역 (1024-49151) 으로
/// OS ephemeral 아웃바운드 할당과 겹치지 않으면서 충돌 가능성이 낮음.
/// MAX_PORT_ATTEMPTS=10 으로 10090-10099 범위 자동 폴백.
pub const DEFAULT_WEB_PORT: u16 = 10090;

fn default_web_enabled() -> bool {
    true
}

fn default_web_port() -> u16 {
    DEFAULT_WEB_PORT
}

/// Default loopback gRPC Dashboard port.
///
/// Must match `oneshim_web::grpc::DEFAULT_GRPC_DASHBOARD_PORT`. `oneshim-core`
/// cannot depend on `oneshim-web`, so the local unit test pins the contract.
/// Kept in the 10080-10089 band so it does not overlap the HTTP dashboard's
/// 10090-10099 fallback range.
pub const DEFAULT_GRPC_DASHBOARD_PORT: u16 = 10080;

/// Previous default, kept only so ConfigManager can migrate persisted defaults.
pub const LEGACY_GRPC_DASHBOARD_PORT: u16 = 10091;

const _: () = assert!(DEFAULT_GRPC_DASHBOARD_PORT < DEFAULT_WEB_PORT);

fn default_grpc_dashboard_port() -> u16 {
    DEFAULT_GRPC_DASHBOARD_PORT
}

// ── LoadThresholds defaults (D13-v2b) ──────────────────────────────

fn default_min_free_mem_gb() -> f32 {
    2.0
}

fn default_cpu_low_pct() -> f32 {
    50.0
}

fn default_cpu_medium_pct() -> f32 {
    70.0
}

fn default_cpu_high_pct() -> f32 {
    90.0
}

fn default_max_concurrent_streams() -> usize {
    50
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_thresholds_default_values() {
        let t = LoadThresholds::default();
        assert_eq!(t.min_free_mem_gb, 2.0);
        assert_eq!(t.cpu_low_pct, 50.0);
        assert_eq!(t.cpu_medium_pct, 70.0);
        assert_eq!(t.cpu_high_pct, 90.0);
    }

    #[test]
    fn web_config_default_enables_streaming() {
        let cfg = WebConfig::default();
        assert!(cfg.grpc_streaming_enabled);
        assert!(cfg.grpc_load_thresholds.is_none());
    }

    #[test]
    fn default_grpc_dashboard_port_is_in_separate_10080_range() {
        assert_eq!(DEFAULT_GRPC_DASHBOARD_PORT, 10080);
    }

    #[test]
    fn web_config_default_wires_grpc_port() {
        let cfg = WebConfig::default();
        assert_eq!(cfg.grpc_port, DEFAULT_GRPC_DASHBOARD_PORT);
    }

    #[test]
    fn web_config_default_max_concurrent_streams_50() {
        let cfg = WebConfig::default();
        assert_eq!(cfg.grpc_max_concurrent_streams, 50);
    }

    #[test]
    fn web_config_deserializes_partial_json_with_thresholds() {
        let json = r#"{
            "enabled": true,
            "port": 10090,
            "allow_external": false,
            "grpc_load_thresholds": { "cpu_low_pct": 30.0 }
        }"#;
        let cfg: WebConfig = serde_json::from_str(json).expect("parse");
        let t = cfg.grpc_load_thresholds.expect("thresholds set");
        assert_eq!(t.cpu_low_pct, 30.0);
        // Other fields fall back to defaults
        assert_eq!(t.cpu_medium_pct, 70.0);
        assert_eq!(t.min_free_mem_gb, 2.0);
        assert_eq!(cfg.grpc_port, DEFAULT_GRPC_DASHBOARD_PORT);
    }

    #[test]
    fn web_config_grpc_port_roundtrips_via_serde() {
        let cfg = WebConfig {
            grpc_port: 55_555,
            ..WebConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        let parsed: WebConfig = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.grpc_port, 55_555);
    }
}
