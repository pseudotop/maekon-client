// Network connection config — server/gRPC/TLS/Web settings
use serde::{Deserialize, Serialize};

// ── TlsConfig ──────────────────────────────────────────────────────

/// TLS connection config — security policy for outbound HTTP/SSE connections.
///
/// Defaults: enabled=true (TLS enforced), allow_self_signed=false (production
/// standard). In development, enabled=false explicitly permits local HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsConfig {
    /// Whether TLS is enforced — when false, http:// connections are allowed (dev only).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Compatibility field. The client no longer bypasses certificate verification.
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
    /// Hosts the app may hand off to over `https`, matched exactly and
    /// case-insensitively. Empty means no external https handoff is permitted.
    ///
    /// #9785: this replaces a hardcoded vendor allowlist in
    /// `commands/os_handoff.rs`. Every other origin in this client comes from
    /// configuration — `base_url` defaults to `http://localhost:8000` — and that
    /// one constant was the sole deviation, which is why the public-export
    /// scanner flagged it. It also closes the gap its own author documented:
    /// a self-hosted Console was unreachable through a static list.
    ///
    /// Exact match is deliberate and is preserved from the original design. A
    /// suffix rule would turn `evil-app.<vendor-domain>` into an allowed target,
    /// so no wildcard or suffix form is accepted here either.
    ///
    /// Configuring this is not a privilege escalation: anyone who can write this
    /// file can already repoint `base_url`.
    #[serde(default)]
    pub allowed_handoff_hosts: Vec<String>,
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

// NOTE: Debug is hand-written (not derived) to mask `integration_auth_token`
// (#7600). This is the SOURCE config struct backing the external integration
// bearer secret; a derived Debug would emit it verbatim under any `{:?}`, so
// a single error-path `?config` could leak it to a file/OTel log sink.
#[derive(Clone, Serialize, Deserialize)]
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
    /// `0` means "use the default". `MAEKON_DASHBOARD_GRPC_PORT` can still
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

impl std::fmt::Debug for WebConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebConfig")
            .field("enabled", &self.enabled)
            .field("port", &self.port)
            .field("allow_external", &self.allow_external)
            .field(
                "integration_auth_token",
                &self.integration_auth_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("grpc_port", &self.grpc_port)
            .field("grpc_load_thresholds", &self.grpc_load_thresholds)
            .field("grpc_streaming_enabled", &self.grpc_streaming_enabled)
            .field(
                "grpc_max_concurrent_streams",
                &self.grpc_max_concurrent_streams,
            )
            .finish()
    }
}

impl WebConfig {
    pub fn validate_bounds(&self) -> Result<(), String> {
        if !(DEFAULT_WEB_PORT..=DEFAULT_WEB_PORT_END).contains(&self.port) {
            return Err(format!(
                "web.port must be within the local dashboard CSP range {}-{}",
                DEFAULT_WEB_PORT, DEFAULT_WEB_PORT_END
            ));
        }
        if self.allow_external {
            let token = self.integration_auth_token.as_deref().unwrap_or_default();
            validate_integration_auth_token_strength(token)?;
        }
        Ok(())
    }

    /// Fail-closed clamp for the INITIAL-LOAD path (#6883), the web counterpart of the
    /// section interval clamps (#6169/#6177). The write chokepoints
    /// (`update`/`update_with`/`reload`) run [`Self::validate_bounds`] and REJECT a bad
    /// config, but the startup load path only clamps — so without this, a well-formed
    /// `config.json` written before the #6772 strength gate (or hand-edited / restored
    /// from an older backup) would load `allow_external = true` + a weak / out-of-range
    /// value verbatim and bind the integration API to `0.0.0.0` with a guessable bearer.
    ///
    /// Two corrections, both fail-closed: snap an out-of-CSP-range `port` back to the
    /// default, and force `allow_external` OFF when the persisted token does not meet
    /// `validate_integration_auth_token_strength` (the user can re-enable external access
    /// through the settings write path, which enforces the floor). The returned set
    /// satisfies [`Self::validate_bounds`] afterwards.
    pub(crate) fn clamp_bounds(&mut self) -> Vec<&'static str> {
        let mut clamped = Vec::new();
        if !(DEFAULT_WEB_PORT..=DEFAULT_WEB_PORT_END).contains(&self.port) {
            self.port = DEFAULT_WEB_PORT;
            clamped.push("web.port");
        }
        if self.allow_external {
            let token = self.integration_auth_token.as_deref().unwrap_or_default();
            if validate_integration_auth_token_strength(token).is_err() {
                // Do NOT bind 0.0.0.0 with a sub-strength token at startup.
                self.allow_external = false;
                clamped.push("web.allow_external");
            }
        }
        clamped
    }
}

pub const MIN_INTEGRATION_AUTH_TOKEN_LEN: usize = 32;
pub const MIN_INTEGRATION_AUTH_TOKEN_CLASSES: usize = 2;

pub fn validate_integration_auth_token_strength(token: &str) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err(
            "web.integration_auth_token is required before enabling web.allow_external".to_string(),
        );
    }
    if token.len() < MIN_INTEGRATION_AUTH_TOKEN_LEN {
        return Err(format!(
            "web.integration_auth_token must be at least {} characters before enabling web.allow_external",
            MIN_INTEGRATION_AUTH_TOKEN_LEN
        ));
    }

    let mut has_lower = false;
    let mut has_upper = false;
    let mut has_digit = false;
    let mut has_symbol = false;
    for ch in token.chars() {
        if ch.is_ascii_lowercase() {
            has_lower = true;
        } else if ch.is_ascii_uppercase() {
            has_upper = true;
        } else if ch.is_ascii_digit() {
            has_digit = true;
        } else {
            has_symbol = true;
        }
    }

    let class_count = [has_lower, has_upper, has_digit, has_symbol]
        .into_iter()
        .filter(|present| *present)
        .count();
    if class_count < MIN_INTEGRATION_AUTH_TOKEN_CLASSES {
        return Err(format!(
            "web.integration_auth_token must contain at least {} character classes before enabling web.allow_external",
            MIN_INTEGRATION_AUTH_TOKEN_CLASSES
        ));
    }

    Ok(())
}

// ── LoadThresholds (D13-v2b) ───────────────────────────────────────

/// Thresholds for `maekon-web::grpc::LoadPolicy` CPU%/memory-GiB classification.
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

// ── Default / helper functions (pub(super) — used by config/mod.rs) ─

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

/// Default port for the local web server.
///
/// Well-Known/Registered ports such as 9090 can collide with Prometheus, Cockpit,
/// etc. 10090 sits in the unassigned Registered Port range (1024-49151), so it
/// avoids common service defaults while staying in the static Tauri CSP allowlist.
pub const DEFAULT_WEB_PORT: u16 = 10090;
pub const WEB_PORT_FALLBACK_ATTEMPTS: u16 = 10;
pub const DEFAULT_WEB_PORT_END: u16 = DEFAULT_WEB_PORT + WEB_PORT_FALLBACK_ATTEMPTS - 1;

fn default_web_enabled() -> bool {
    true
}

fn default_web_port() -> u16 {
    DEFAULT_WEB_PORT
}

/// Default loopback gRPC Dashboard port.
///
/// Must match `maekon_web::grpc::DEFAULT_GRPC_DASHBOARD_PORT`. `maekon-core`
/// cannot depend on `maekon-web`, so the local unit test pins the contract.
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
    fn web_config_default_port_matches_csp_fallback_range() {
        let cfg = WebConfig::default();
        assert_eq!(cfg.port, DEFAULT_WEB_PORT);
        assert_eq!(DEFAULT_WEB_PORT_END, 10099);
        cfg.validate_bounds()
            .expect("default web port must satisfy CSP range bounds");
    }

    #[test]
    fn web_config_rejects_ports_outside_csp_fallback_range() {
        let low = WebConfig {
            port: DEFAULT_WEB_PORT - 1,
            ..WebConfig::default()
        };
        let high = WebConfig {
            port: DEFAULT_WEB_PORT_END + 1,
            ..WebConfig::default()
        };

        assert!(low.validate_bounds().unwrap_err().contains("web.port"));
        assert!(high.validate_bounds().unwrap_err().contains("web.port"));
    }

    #[test]
    fn web_config_rejects_weak_external_integration_token() {
        let missing = WebConfig {
            allow_external: true,
            integration_auth_token: None,
            ..WebConfig::default()
        };
        let short = WebConfig {
            allow_external: true,
            integration_auth_token: Some("short".to_string()),
            ..WebConfig::default()
        };
        let single_class = WebConfig {
            allow_external: true,
            integration_auth_token: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string()),
            ..WebConfig::default()
        };
        let strong = WebConfig {
            allow_external: true,
            integration_auth_token: Some("integration-secret-0123456789abcdef".to_string()),
            ..WebConfig::default()
        };

        assert!(missing
            .validate_bounds()
            .unwrap_err()
            .contains("integration_auth_token"));
        assert!(short.validate_bounds().unwrap_err().contains("at least 32"));
        assert!(single_class
            .validate_bounds()
            .unwrap_err()
            .contains("character classes"));
        strong
            .validate_bounds()
            .expect("strong token should allow external integration bind");
    }

    // ── #6883 INITIAL-LOAD clamp (fail-closed) ──────────────────────────────

    #[test]
    fn web_config_clamp_bounds_fail_closes_weak_external_token() {
        // The #6883 vector: a well-formed external config that never passed the #6772
        // write-path strength gate (downgrade / hand-edit / restored old backup).
        let mut weak = WebConfig {
            allow_external: true,
            integration_auth_token: Some("short".to_string()),
            ..WebConfig::default()
        };
        let clamped = weak.clamp_bounds();
        assert!(
            clamped.contains(&"web.allow_external"),
            "a sub-strength external token must fail-close allow_external"
        );
        assert!(
            !weak.allow_external,
            "allow_external must be forced off on load"
        );
        // Contract: the clamped config must satisfy validate_bounds afterward.
        weak.validate_bounds()
            .expect("clamped web config must satisfy validate_bounds");
    }

    #[test]
    fn web_config_clamp_bounds_snaps_out_of_range_port() {
        let mut high = WebConfig {
            port: DEFAULT_WEB_PORT_END + 5,
            ..WebConfig::default()
        };
        let clamped = high.clamp_bounds();
        assert!(clamped.contains(&"web.port"));
        assert_eq!(
            high.port, DEFAULT_WEB_PORT,
            "out-of-CSP-range port snaps to default"
        );
        high.validate_bounds()
            .expect("clamped port must satisfy validate_bounds");
    }

    #[test]
    fn web_config_clamp_bounds_preserves_valid_external_config() {
        // allow_external=true + strong token + in-range port → nothing to clamp.
        let mut strong = WebConfig {
            allow_external: true,
            integration_auth_token: Some("integration-secret-0123456789abcdef".to_string()),
            ..WebConfig::default()
        };
        let clamped = strong.clamp_bounds();
        assert!(
            clamped.is_empty(),
            "a valid external config must not be clamped"
        );
        assert!(
            strong.allow_external,
            "valid external access must be preserved"
        );
        strong
            .validate_bounds()
            .expect("an unchanged strong config still validates");
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

    #[test]
    fn web_config_debug_redacts_integration_auth_token() {
        let cfg = WebConfig {
            allow_external: true,
            integration_auth_token: Some("integration-secret-0123456789abcdef".to_string()),
            ..WebConfig::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(
            !rendered.contains("integration-secret-0123456789abcdef"),
            "Debug must not leak the integration_auth_token: {rendered}"
        );
        assert!(
            rendered.contains("[REDACTED]"),
            "integration_auth_token must render as [REDACTED]: {rendered}"
        );
        // Non-secret fields must still be visible for diagnostics.
        assert!(rendered.contains("allow_external"));
    }

    #[test]
    fn web_config_debug_none_token_does_not_claim_redacted() {
        let cfg = WebConfig::default();
        let rendered = format!("{cfg:?}");
        assert!(
            rendered.contains("integration_auth_token: None"),
            "an absent token must render as None, not a redacted placeholder: {rendered}"
        );
    }
}
