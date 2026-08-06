// OOS-TBD: ADR-013 file split (cycle 33+) — LOC: 690
// Target split: grpc/config/mod.rs (NetworkGrpcConfig struct + defaults),
// grpc/config/tls.rs (mTLS/CA cert loading, TlsConnector construction),
// grpc/config/endpoint.rs (build_endpoint, build_streaming_endpoint — async since cycle 33 W0 #3967).
// Public API unchanged via mod.rs re-exports.
use std::time::Duration;

use maekon_core::config::GrpcConfig as CoreGrpcConfig;
use maekon_core::config::TlsConfig;
use serde::{Deserialize, Serialize};

use crate::error::NetworkError;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

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

    #[serde(default = "default_rest_endpoint")]
    pub rest_endpoint: String,

    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,

    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    #[serde(default = "default_use_tls")]
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

    /// REST fallback TLS config — applied to the internal HttpApiClient.
    /// When `Some`, credentials are sent with TLS enforcement (HTTPS-only).
    /// When `None`, the non-TLS HttpApiClient constructor is used (test/dev only).
    #[serde(default)]
    pub rest_tls: Option<TlsConfig>,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            use_grpc_auth: false,
            use_grpc_context: false,
            grpc_endpoint: default_grpc_endpoint(),
            grpc_fallback_ports: default_grpc_fallback_ports(),
            rest_endpoint: default_rest_endpoint(),
            connect_timeout_secs: default_connect_timeout(),
            request_timeout_secs: default_request_timeout(),
            use_tls: default_use_tls(),
            mtls_enabled: false,
            tls_domain_name: None,
            tls_ca_cert_path: None,
            tls_client_cert_path: None,
            tls_client_key_path: None,
            rest_tls: None,
        }
    }
}

impl From<CoreGrpcConfig> for GrpcConfig {
    fn from(core: CoreGrpcConfig) -> Self {
        Self {
            use_grpc_auth: core.use_grpc_auth,
            use_grpc_context: core.use_grpc_context,
            grpc_endpoint: core.grpc_endpoint,
            grpc_fallback_ports: core.grpc_fallback_ports,
            rest_endpoint: default_rest_endpoint(),
            connect_timeout_secs: core.connect_timeout_secs,
            request_timeout_secs: core.request_timeout_secs,
            use_tls: core.use_tls,
            mtls_enabled: core.mtls_enabled,
            tls_domain_name: core.tls_domain_name,
            tls_ca_cert_path: core.tls_ca_cert_path,
            tls_client_cert_path: core.tls_client_cert_path,
            tls_client_key_path: core.tls_client_key_path,
            rest_tls: None,
        }
    }
}

impl GrpcConfig {
    pub fn from_core_with_rest(core: &CoreGrpcConfig, rest_endpoint: &str) -> Self {
        Self {
            use_grpc_auth: core.use_grpc_auth,
            use_grpc_context: core.use_grpc_context,
            grpc_endpoint: core.grpc_endpoint.clone(),
            grpc_fallback_ports: core.grpc_fallback_ports.clone(),
            rest_endpoint: rest_endpoint.to_string(),
            connect_timeout_secs: core.connect_timeout_secs,
            request_timeout_secs: core.request_timeout_secs,
            use_tls: core.use_tls,
            mtls_enabled: core.mtls_enabled,
            tls_domain_name: core.tls_domain_name.clone(),
            tls_ca_cert_path: core.tls_ca_cert_path.clone(),
            tls_client_cert_path: core.tls_client_cert_path.clone(),
            tls_client_key_path: core.tls_client_key_path.clone(),
            rest_tls: None,
        }
    }

    /// Build from core config with REST endpoint and TLS configuration.
    ///
    /// The `rest_tls` config is applied to the internal HTTP client used for
    /// REST fallback paths (login, feedback), ensuring credentials are sent
    /// with TLS enforcement when the server requires HTTPS.
    pub fn from_core_with_rest_tls(
        core: &CoreGrpcConfig,
        rest_endpoint: &str,
        rest_tls: &TlsConfig,
    ) -> Self {
        let mut config = Self::from_core_with_rest(core, rest_endpoint);
        config.rest_tls = Some(rest_tls.clone());
        config
    }

    pub fn validate_transport_security(&self) -> Result<(), NetworkError> {
        if self.mtls_enabled && !self.use_tls {
            return Err(NetworkError::Config(
                "grpc.mtls_enabled requires grpc.use_tls=true".to_string(),
            ));
        }

        // #6924: cleartext h2c (use_tls=false) is only safe to a LOOPBACK endpoint.
        // A non-loopback grpc_endpoint with use_tls=false sends the session Bearer
        // JWT + uploaded event/frame context + suggestion feedback in PLAINTEXT on
        // the wire (on-path credential theft / session hijack). Fail-closed, mirroring
        // the AI-provider cleartext config guard (#6259). all_endpoints() keeps the
        // grpc_endpoint host and only varies the fallback port, so validating the
        // primary host covers the fallbacks too.
        if !self.use_tls {
            if endpoint_is_loopback(&self.grpc_endpoint) {
                return Ok(());
            }
            return Err(NetworkError::Config(format!(
                "grpc.use_tls=false is only permitted for a loopback grpc_endpoint; \
                 '{}' is remote — cleartext h2c would leak the session token and uploaded \
                 context. Set grpc.use_tls=true (with grpc.tls_domain_name) for remote endpoints.",
                self.grpc_endpoint
            )));
        }

        let domain = self
            .tls_domain_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                // Iter-99: "is required when ..." = Missing semantics.
                NetworkError::Core(maekon_core::error::CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    message: "grpc.tls_domain_name is required when grpc.use_tls=true".to_string(),
                })
            })?;

        if domain.contains('/') {
            return Err(NetworkError::Config(
                "grpc.tls_domain_name must be a hostname without path".to_string(),
            ));
        }

        if self.mtls_enabled {
            self.required_path("grpc.tls_ca_cert_path", self.tls_ca_cert_path.as_deref())?;
            self.required_path(
                "grpc.tls_client_cert_path",
                self.tls_client_cert_path.as_deref(),
            )?;
            self.required_path(
                "grpc.tls_client_key_path",
                self.tls_client_key_path.as_deref(),
            )?;
        }

        Ok(())
    }

    /// Build a tonic [`Endpoint`] for **unary RPCs** (login, upload-batch, feedback, etc.).
    ///
    /// # Async I/O note (F-RR-C33-01)
    ///
    /// When mTLS is enabled this function uses `tokio::fs::read` to load PEM certificate files
    /// without blocking the Tokio worker thread.  The previous `std::fs::read` implementation
    /// blocked the worker on each reconnect, which could starve other tasks under load.
    ///
    /// Applies a channel-level request timeout (`GrpcTimeout::server_timeout`) equal to
    /// `request_timeout_secs` (default 30 s).  This fires on the full round-trip, which is
    /// correct for short-lived unary calls.
    ///
    /// IMPORTANT — do NOT use this for `SubscribeSuggestions` or any other server-streaming
    /// RPC.  The 30 s `GrpcTimeout::server_timeout` wraps the *entire stream future* at the
    /// HTTP/2 connection layer and fires at 30 s regardless of per-message activity, forcing a
    /// reconnect and neutralising the exponential-backoff design.
    /// Use [`build_streaming_endpoint`](Self::build_streaming_endpoint) for those RPCs.
    /// See F-RR-C25-02 / F-RR-C25-06.
    pub async fn build_endpoint(&self, endpoint_url: &str) -> Result<Endpoint, NetworkError> {
        self.validate_transport_security()?;

        let mut endpoint = Endpoint::from_shared(endpoint_url.to_string())
            .map_err(|e| NetworkError::Http(format!("invalid gRPC endpoint: {e}")))?
            .connect_timeout(Duration::from_secs(self.connect_timeout_secs))
            .timeout(Duration::from_secs(self.request_timeout_secs));

        if self.use_tls {
            let domain_name = self
                .tls_domain_name
                .as_deref()
                .map(str::trim)
                .ok_or_else(|| {
                    NetworkError::Config(
                        "grpc.tls_domain_name is required when grpc.use_tls=true".to_string(),
                    )
                })?;

            let mut tls = ClientTlsConfig::new().domain_name(domain_name.to_string());

            if let Some(path) = self.tls_ca_cert_path.as_deref().map(str::trim) {
                if !path.is_empty() {
                    // F-RR-C33-01: use async read to avoid blocking the Tokio worker thread.
                    let pem = tokio::fs::read(path).await.map_err(|e| {
                        NetworkError::Config(format!("failed to read grpc.tls_ca_cert_path: {e}"))
                    })?;
                    tls = tls.ca_certificate(Certificate::from_pem(pem));
                }
            }

            if self.mtls_enabled {
                let cert_path = self
                    .tls_client_cert_path
                    .as_deref()
                    .ok_or_else(|| {
                        // Iter-99: required-when → Missing.
                        NetworkError::Core(maekon_core::error::CoreError::Config {
                            code: maekon_core::error_codes::ConfigCode::Missing,
                            message:
                                "grpc.tls_client_cert_path is required when grpc.mtls_enabled=true"
                                    .to_string(),
                        })
                    })?
                    .trim();
                let key_path = self
                    .tls_client_key_path
                    .as_deref()
                    .ok_or_else(|| {
                        // Iter-99: required-when → Missing.
                        NetworkError::Core(maekon_core::error::CoreError::Config {
                            code: maekon_core::error_codes::ConfigCode::Missing,
                            message:
                                "grpc.tls_client_key_path is required when grpc.mtls_enabled=true"
                                    .to_string(),
                        })
                    })?
                    .trim();

                // F-RR-C33-01: use async read to avoid blocking the Tokio worker thread.
                let cert_pem = tokio::fs::read(cert_path).await.map_err(|e| {
                    NetworkError::Config(format!("failed to read grpc.tls_client_cert_path: {e}"))
                })?;
                let key_pem = tokio::fs::read(key_path).await.map_err(|e| {
                    NetworkError::Config(format!("failed to read grpc.tls_client_key_path: {e}"))
                })?;

                tls = tls.identity(Identity::from_pem(cert_pem, key_pem));
            }

            endpoint = endpoint.tls_config(tls).map_err(|e| {
                NetworkError::Config(format!("invalid grpc tls configuration: {e}"))
            })?;
        }

        Ok(endpoint)
    }

    pub async fn connect_channel(&self, endpoint_url: &str) -> Result<Channel, NetworkError> {
        let endpoint = self.build_endpoint(endpoint_url).await?;
        endpoint
            .connect()
            .await
            .map_err(|e| NetworkError::Http(format!("gRPC connection failed: {e}")))
    }

    /// Build a tonic [`Endpoint`] for **server-streaming RPCs** (e.g. `SubscribeSuggestions`).
    ///
    /// # Async I/O note (F-RR-C33-01)
    ///
    /// Same as [`build_endpoint`](Self::build_endpoint): PEM files are now read via
    /// `tokio::fs::read` so the Tokio worker thread is not blocked on reconnect.
    ///
    /// Intentionally omits the channel-level `.timeout()` call so that
    /// `GrpcTimeout::server_timeout` is `None`.  Without a channel deadline the stream can
    /// remain open indefinitely; liveness is instead enforced by the per-message
    /// `MSG_TIMEOUT` (60 s) in `GrpcSseAdapter`.
    ///
    /// `connect_timeout` is still applied so initial handshake failures are detected promptly.
    ///
    /// F-RR-C25-02 / F-RR-C25-06: fixes forced 30 s reconnect that was resetting exponential
    /// backoff to 1 s on every cycle.
    pub async fn build_streaming_endpoint(
        &self,
        endpoint_url: &str,
    ) -> Result<Endpoint, NetworkError> {
        self.validate_transport_security()?;

        // No `.timeout()` — streaming RPCs must not have a channel-level deadline.
        let mut endpoint = Endpoint::from_shared(endpoint_url.to_string())
            .map_err(|e| NetworkError::Http(format!("invalid gRPC streaming endpoint: {e}")))?
            .connect_timeout(Duration::from_secs(self.connect_timeout_secs));

        if self.use_tls {
            let domain_name = self
                .tls_domain_name
                .as_deref()
                .map(str::trim)
                .ok_or_else(|| {
                    NetworkError::Config(
                        "grpc.tls_domain_name is required when grpc.use_tls=true".to_string(),
                    )
                })?;

            let mut tls = ClientTlsConfig::new().domain_name(domain_name.to_string());

            if let Some(path) = self.tls_ca_cert_path.as_deref().map(str::trim) {
                if !path.is_empty() {
                    // F-RR-C33-01: use async read to avoid blocking the Tokio worker thread.
                    let pem = tokio::fs::read(path).await.map_err(|e| {
                        NetworkError::Config(format!("failed to read grpc.tls_ca_cert_path: {e}"))
                    })?;
                    tls = tls.ca_certificate(Certificate::from_pem(pem));
                }
            }

            if self.mtls_enabled {
                let cert_path = self
                    .tls_client_cert_path
                    .as_deref()
                    .ok_or_else(|| {
                        NetworkError::Core(maekon_core::error::CoreError::Config {
                            code: maekon_core::error_codes::ConfigCode::Missing,
                            message:
                                "grpc.tls_client_cert_path is required when grpc.mtls_enabled=true"
                                    .to_string(),
                        })
                    })?
                    .trim();
                let key_path = self
                    .tls_client_key_path
                    .as_deref()
                    .ok_or_else(|| {
                        NetworkError::Core(maekon_core::error::CoreError::Config {
                            code: maekon_core::error_codes::ConfigCode::Missing,
                            message:
                                "grpc.tls_client_key_path is required when grpc.mtls_enabled=true"
                                    .to_string(),
                        })
                    })?
                    .trim();

                // F-RR-C33-01: use async read to avoid blocking the Tokio worker thread.
                let cert_pem = tokio::fs::read(cert_path).await.map_err(|e| {
                    NetworkError::Config(format!("failed to read grpc.tls_client_cert_path: {e}"))
                })?;
                let key_pem = tokio::fs::read(key_path).await.map_err(|e| {
                    NetworkError::Config(format!("failed to read grpc.tls_client_key_path: {e}"))
                })?;

                tls = tls.identity(Identity::from_pem(cert_pem, key_pem));
            }

            endpoint = endpoint.tls_config(tls).map_err(|e| {
                NetworkError::Config(format!("invalid grpc tls configuration: {e}"))
            })?;
        }

        Ok(endpoint)
    }

    /// Connect a [`Channel`] suitable for **server-streaming RPCs**.
    ///
    /// Tries all configured endpoints (`grpc_endpoint` + `grpc_fallback_ports`) in order,
    /// returning the first successful connection.  Uses [`build_streaming_endpoint`] so the
    /// resulting channel has no channel-level request deadline.
    pub async fn connect_streaming_channel(
        &self,
        endpoint_url: &str,
    ) -> Result<Channel, NetworkError> {
        let endpoint = self.build_streaming_endpoint(endpoint_url).await?;
        endpoint
            .connect()
            .await
            .map_err(|e| NetworkError::Http(format!("gRPC streaming connection failed: {e}")))
    }

    /// Build the ordered list of endpoints to try: the primary `grpc_endpoint`
    /// followed by one fallback per configured `grpc_fallback_ports` entry.
    ///
    /// A fallback endpoint is the primary endpoint with its port swapped. To
    /// locate the port we first strip the `scheme://` prefix and then treat a
    /// trailing `:<digits>` (or `]:<digits>` for bracketed IPv6 literals) as the
    /// port boundary. If the authority carries no explicit numeric port we do
    /// **not** invent fallbacks: the previous `rsplit_once(':')` implementation
    /// split on the *scheme* colon for port-less URLs (e.g. `https://host`),
    /// producing garbage endpoints like `https:50052`.
    pub fn all_endpoints(&self) -> Vec<String> {
        let mut endpoints = vec![self.grpc_endpoint.clone()];

        if let Some(host_prefix) = endpoint_host_prefix(&self.grpc_endpoint) {
            for port in &self.grpc_fallback_ports {
                endpoints.push(format!("{host_prefix}:{port}"));
            }
        }

        endpoints
    }
}

/// Return the endpoint string up to (but excluding) the `:<port>` separator, so
/// the caller can append a fallback port. Returns `None` when the endpoint has
/// no explicit numeric port (in which case fallback ports must not be invented).
///
/// Parsing rules:
/// - Strip a leading `scheme://` so the scheme colon is never mistaken for the
///   port colon.
/// - For a bracketed IPv6 authority (`[::1]:50051`) the port boundary is the
///   `:` that immediately follows the closing `]`.
/// - Otherwise the port boundary is the last `:` in the authority, and the
///   portion after it must be all ASCII digits.
fn endpoint_host_prefix(endpoint: &str) -> Option<&str> {
    // Length of the scheme (incl. "://") that we must keep in the returned
    // prefix. Splitting here ensures the scheme colon is excluded from the
    // authority-level port search below.
    let scheme_len = endpoint.find("://").map(|i| i + 3).unwrap_or(0);
    let (scheme, authority) = endpoint.split_at(scheme_len);

    // Determine where to start searching for the port colon. For a bracketed
    // IPv6 host the port can only appear after the closing ']' — a malformed
    // bracket (no ']') means no usable port boundary, so bail with None.
    let search_start = if authority.starts_with('[') {
        authority.find(']')? + 1
    } else {
        0
    };

    let colon_in_authority = authority[search_start..]
        .rfind(':')
        .map(|i| search_start + i)?;

    // The text after the colon must be a non-empty run of ASCII digits to be a
    // real port; otherwise this colon is not a port separator (e.g. a port-less
    // IPv6 literal `[::1]` whose last ':' is inside the address).
    let port_str = &authority[colon_in_authority + 1..];
    if port_str.is_empty() || !port_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    // Return scheme + authority-up-to-port. `scheme.len()` is a byte offset on
    // the original string, and `colon_in_authority` is relative to `authority`,
    // so the host prefix is the original string sliced to that combined offset.
    Some(&endpoint[..scheme.len() + colon_in_authority])
}

/// #6924: true if the gRPC endpoint's host is a loopback address (localhost / 127/8
/// / ::1). Reuses `http_client::host_is_loopback` (handles bracketed IPv6 + IP
/// literals), normalizing a scheme-less endpoint by prepending `http://` first so a
/// genuinely-loopback `host:port` (no scheme) is not falsely rejected by the
/// cleartext guard. A malformed/unparseable endpoint is treated as non-loopback
/// (fail-closed — the cleartext guard then rejects it).
fn endpoint_is_loopback(endpoint: &str) -> bool {
    let normalized = if endpoint.contains("://") {
        endpoint.to_string()
    } else {
        format!("http://{endpoint}")
    };
    maekon_http_core::outbound::host_is_loopback(&normalized)
}

fn default_grpc_endpoint() -> String {
    "http://localhost:50051".to_string()
}

fn default_grpc_fallback_ports() -> Vec<u16> {
    vec![50052, 50053]
}

fn default_rest_endpoint() -> String {
    "http://localhost:8000".to_string()
}

fn default_connect_timeout() -> u64 {
    10
}

fn default_request_timeout() -> u64 {
    30
}

fn default_use_tls() -> bool {
    false
}

impl GrpcConfig {
    fn required_path(&self, field: &str, value: Option<&str>) -> Result<(), NetworkError> {
        let valid = value
            .map(str::trim)
            .map(|path| !path.is_empty())
            .unwrap_or(false);

        if !valid {
            // Iter-99: "is required when" = Missing (not Invalid).
            return Err(NetworkError::Core(maekon_core::error::CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Missing,
                message: format!("{field} is required when grpc.mtls_enabled=true"),
            }));
        }

        Ok(())
    }

    pub fn needs_rest_fallback(&self) -> bool {
        !self.use_grpc_auth || !self.use_grpc_context
    }

    pub fn should_use_grpc_for_auth(&self) -> bool {
        self.use_grpc_auth
    }

    pub fn should_use_grpc_for_context(&self) -> bool {
        self.use_grpc_context
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = GrpcConfig::default();
        assert!(!config.use_grpc_auth);
        assert!(!config.use_grpc_context);
        assert!(!config.use_tls);
        assert!(!config.mtls_enabled);
        assert_eq!(config.grpc_endpoint, "http://localhost:50051");
        assert!(config.needs_rest_fallback());
    }

    #[test]
    fn test_grpc_enabled() {
        let config = GrpcConfig {
            use_grpc_auth: true,
            use_grpc_context: true,
            ..Default::default()
        };
        assert!(!config.needs_rest_fallback());
        assert!(config.should_use_grpc_for_auth());
        assert!(config.should_use_grpc_for_context());
    }

    #[test]
    fn test_fallback_ports() {
        let config = GrpcConfig::default();
        assert_eq!(config.grpc_fallback_ports, vec![50052, 50053]);
    }

    #[test]
    fn test_all_endpoints() {
        let config = GrpcConfig::default();
        let endpoints = config.all_endpoints();
        assert_eq!(endpoints.len(), 3);
        assert_eq!(endpoints[0], "http://localhost:50051");
        assert_eq!(endpoints[1], "http://localhost:50052");
        assert_eq!(endpoints[2], "http://localhost:50053");
    }

    #[test]
    fn test_all_endpoints_custom() {
        let config = GrpcConfig {
            grpc_endpoint: "http://example.com:9000".to_string(),
            grpc_fallback_ports: vec![9001, 9002],
            ..Default::default()
        };
        let endpoints = config.all_endpoints();
        assert_eq!(endpoints.len(), 3);
        assert_eq!(endpoints[0], "http://example.com:9000");
        assert_eq!(endpoints[1], "http://example.com:9001");
        assert_eq!(endpoints[2], "http://example.com:9002");
    }

    /// Regression (#11): a port-less endpoint must NOT generate garbage
    /// fallbacks. The old `rsplit_once(':')` split on the scheme colon, turning
    /// `https://host` into a host of `https`, yielding `https:50052` etc.
    #[test]
    fn test_all_endpoints_portless_no_garbage_fallback() {
        let config = GrpcConfig {
            grpc_endpoint: "https://host".to_string(),
            grpc_fallback_ports: vec![50052, 50053],
            ..Default::default()
        };
        let endpoints = config.all_endpoints();
        // Only the primary endpoint is returned — no port can be swapped in.
        assert_eq!(
            endpoints,
            vec!["https://host".to_string()],
            "a port-less endpoint must not synthesize fallback endpoints"
        );
        // Explicitly assert none of the classic garbage forms appears.
        assert!(
            !endpoints.iter().any(|e| e.starts_with("https:5")),
            "scheme colon must never be treated as the port boundary: {endpoints:?}"
        );
    }

    /// A port-less endpoint with a path (e.g. behind a reverse proxy) likewise
    /// yields no fallbacks — there is no numeric port to swap.
    #[test]
    fn test_all_endpoints_portless_with_path_no_fallback() {
        let config = GrpcConfig {
            grpc_endpoint: "https://grpc.example.com/api".to_string(),
            grpc_fallback_ports: vec![50052],
            ..Default::default()
        };
        let endpoints = config.all_endpoints();
        assert_eq!(endpoints, vec!["https://grpc.example.com/api".to_string()]);
    }

    /// A bracketed IPv6 authority WITH a port swaps the port correctly and does
    /// not get confused by the colons inside the address.
    #[test]
    fn test_all_endpoints_ipv6_with_port() {
        let config = GrpcConfig {
            grpc_endpoint: "http://[::1]:50051".to_string(),
            grpc_fallback_ports: vec![50052, 50053],
            ..Default::default()
        };
        let endpoints = config.all_endpoints();
        assert_eq!(endpoints.len(), 3);
        assert_eq!(endpoints[0], "http://[::1]:50051");
        assert_eq!(endpoints[1], "http://[::1]:50052");
        assert_eq!(endpoints[2], "http://[::1]:50053");
    }

    /// A bracketed IPv6 authority WITHOUT a port must not treat the address's
    /// internal colons as a port boundary, so no fallback is produced.
    #[test]
    fn test_all_endpoints_ipv6_without_port_no_fallback() {
        let config = GrpcConfig {
            grpc_endpoint: "http://[::1]".to_string(),
            grpc_fallback_ports: vec![50052],
            ..Default::default()
        };
        let endpoints = config.all_endpoints();
        assert_eq!(endpoints, vec!["http://[::1]".to_string()]);
    }

    /// An endpoint with no scheme but an explicit port still swaps correctly.
    #[test]
    fn test_all_endpoints_no_scheme_with_port() {
        let config = GrpcConfig {
            grpc_endpoint: "localhost:50051".to_string(),
            grpc_fallback_ports: vec![50052],
            ..Default::default()
        };
        let endpoints = config.all_endpoints();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0], "localhost:50051");
        assert_eq!(endpoints[1], "localhost:50052");
    }

    #[test]
    fn test_tls_requires_domain_name() {
        let config = GrpcConfig {
            use_tls: true,
            tls_domain_name: None,
            ..Default::default()
        };

        let err = config.validate_transport_security().unwrap_err();
        assert!(
            matches!(err, NetworkError::Core(maekon_core::error::CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Missing, ..
            })),
            "missing tls_domain_name must return NetworkError::Core(CoreError::Config::Missing), got: {err:?}"
        );
    }

    #[test]
    fn test_mtls_requires_tls_enabled() {
        let config = GrpcConfig {
            use_tls: false,
            mtls_enabled: true,
            ..Default::default()
        };

        let err = config.validate_transport_security().unwrap_err();
        assert!(
            matches!(err, NetworkError::Config(_)),
            "mtls without tls must return NetworkError::Config, got: {err:?}"
        );
        assert!(
            err.to_string()
                .contains("grpc.mtls_enabled requires grpc.use_tls=true"),
            "error message mismatch, got: {err:?}"
        );
    }

    #[test]
    fn test_mtls_requires_all_pem_paths() {
        let config = GrpcConfig {
            use_tls: true,
            mtls_enabled: true,
            tls_domain_name: Some("localhost".to_string()),
            tls_ca_cert_path: Some("/tmp/ca.pem".to_string()),
            tls_client_cert_path: None,
            tls_client_key_path: Some("/tmp/client.key".to_string()),
            ..Default::default()
        };

        let err = config.validate_transport_security().unwrap_err();
        assert!(
            matches!(err, NetworkError::Core(maekon_core::error::CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Missing, ..
            })),
            "missing mtls cert path must return NetworkError::Core(CoreError::Config::Missing), got: {err:?}"
        );
    }

    #[test]
    fn test_tls_validation_accepts_domain_only() {
        let config = GrpcConfig {
            use_tls: true,
            mtls_enabled: false,
            tls_domain_name: Some("localhost".to_string()),
            ..Default::default()
        };

        // use_tls=true + domain present + mtls disabled must pass validation.
        config
            .validate_transport_security()
            .expect("TLS-only (no mTLS) with a domain name must pass validate_transport_security");
    }

    /// #6924: cleartext h2c (use_tls=false) to a LOOPBACK endpoint is allowed —
    /// this is the legitimate dev/default path (server-side h2c per ADR-074 is
    /// loopback-only). Covers localhost, 127.0.0.1, [::1], and scheme-less host:port.
    #[test]
    fn test_cleartext_allowed_for_loopback() {
        for ep in [
            "http://localhost:50051",
            "http://127.0.0.1:50051",
            "http://[::1]:50051",
            "localhost:50051", // scheme-less must still be recognized as loopback
        ] {
            let config = GrpcConfig {
                use_tls: false,
                grpc_endpoint: ep.to_string(),
                ..Default::default()
            };
            config.validate_transport_security().unwrap_or_else(|e| {
                panic!("cleartext to loopback {ep} must be allowed, got: {e:?}")
            });
        }
    }

    /// #6924: cleartext h2c (use_tls=false) to a NON-loopback endpoint must
    /// fail-closed — otherwise the session JWT + uploaded context egress in
    /// plaintext. Pre-fix this returned Ok(()).
    #[test]
    fn test_cleartext_rejected_for_remote() {
        for ep in [
            "http://example.com:9000",
            "http://10.0.0.5:50051",
            // RFC 2606 reserved documentation host — a remote (non-loopback) gRPC
            // endpoint on the TLS port. Must not name a real/operator domain so the
            // public OSS export carries no internal references (the release/export
            // guardrail rejects operator-domain leaks). The assertion is
            // host-agnostic — any non-loopback host over cleartext must fail-closed.
            "http://grpc.example.org:443",
        ] {
            let config = GrpcConfig {
                use_tls: false,
                grpc_endpoint: ep.to_string(),
                ..Default::default()
            };
            let err = config
                .validate_transport_security()
                .expect_err(&format!("cleartext to remote {ep} must be rejected"));
            assert!(
                matches!(err, NetworkError::Config(_)),
                "remote cleartext must return NetworkError::Config, got: {err:?}"
            );
        }
    }

    /// #6924: a remote endpoint with use_tls=true is fine (TLS protects the wire).
    #[test]
    fn test_remote_endpoint_allowed_with_tls() {
        let config = GrpcConfig {
            use_tls: true,
            grpc_endpoint: "https://grpc.example.com:443".to_string(),
            tls_domain_name: Some("grpc.example.com".to_string()),
            ..Default::default()
        };
        config
            .validate_transport_security()
            .expect("remote endpoint with TLS + domain must pass");
    }

    #[test]
    fn test_default_rest_tls_is_none() {
        let config = GrpcConfig::default();
        assert!(config.rest_tls.is_none());
    }

    #[test]
    fn test_from_core_with_rest_tls() {
        let core = CoreGrpcConfig::default();
        let tls = TlsConfig {
            enabled: true,
            allow_self_signed: false,
        };
        let config = GrpcConfig::from_core_with_rest_tls(&core, "https://api.example.com", &tls);

        assert_eq!(config.rest_endpoint, "https://api.example.com");
        assert!(config.rest_tls.is_some());
        let rest_tls = config.rest_tls.unwrap();
        assert!(rest_tls.enabled);
        assert!(!rest_tls.allow_self_signed);
    }

    #[test]
    fn test_from_core_with_rest_has_no_tls() {
        let core = CoreGrpcConfig::default();
        let config = GrpcConfig::from_core_with_rest(&core, "http://localhost:8000");

        assert_eq!(config.rest_endpoint, "http://localhost:8000");
        assert!(config.rest_tls.is_none());
    }

    /// Iter-99 regression guards: "is required" validation errors emit
    /// ConfigCode::Missing (wire code `config.missing`), not the old
    /// ConfigCode::Invalid. Lets telemetry distinguish missing TLS config
    /// from invalid format/combination.
    #[test]
    fn test_validate_missing_tls_domain_name_emits_missing() {
        let config = GrpcConfig {
            use_tls: true,
            tls_domain_name: None,
            ..GrpcConfig::default()
        };
        let err = config.validate_transport_security().unwrap_err();
        let core: maekon_core::error::CoreError = err.into();
        assert_eq!(core.code(), "config.missing");
    }

    #[test]
    fn test_validate_missing_client_cert_emits_missing() {
        let config = GrpcConfig {
            use_tls: true,
            mtls_enabled: true,
            tls_domain_name: Some("api.example.com".to_string()),
            // tls_ca_cert_path is optional per required_path for the ca field
            tls_ca_cert_path: Some("/tmp/ca.pem".to_string()),
            tls_client_cert_path: None, // Missing
            tls_client_key_path: Some("/tmp/key.pem".to_string()),
            ..GrpcConfig::default()
        };
        let err = config.validate_transport_security().unwrap_err();
        let core: maekon_core::error::CoreError = err.into();
        assert_eq!(core.code(), "config.missing");
    }

    /// Pre-iter-99 guard: mtls+use_tls combination check stays as Invalid
    /// (the values are present, their combination is illegal).
    #[test]
    fn test_validate_mtls_without_tls_stays_invalid() {
        let config = GrpcConfig {
            mtls_enabled: true,
            use_tls: false,
            ..GrpcConfig::default()
        };
        let err = config.validate_transport_security().unwrap_err();
        let core: maekon_core::error::CoreError = err.into();
        assert_eq!(core.code(), "config.invalid");
    }

    // --- F-RR-C25-02/06: build_streaming_endpoint omits channel-level timeout ---

    /// `build_streaming_endpoint` must produce an `Endpoint` that does NOT carry a
    /// channel-level `timeout` deadline.  In tonic 0.14 the `Endpoint::timeout` field is
    /// `pub(crate)`, so we verify indirectly: if the endpoint builds successfully *and*
    /// `build_endpoint` (with `.timeout()`) also builds successfully for the same URL, the
    /// divergence is that the streaming variant omits the deadline baked into
    /// `GrpcTimeout::server_timeout`.  This test verifies the happy-path success of
    /// `build_streaming_endpoint` with a non-TLS config (the meaningful correctness is
    /// exercised by the integration assertion in context_client tests).
    // F-RR-C33-01: build_streaming_endpoint is now async; use #[tokio::test].
    #[tokio::test]
    async fn build_streaming_endpoint_succeeds_for_plain_url() {
        let config = GrpcConfig::default(); // use_tls = false, request_timeout_secs = 30
        let result = config
            .build_streaming_endpoint("http://localhost:50051")
            .await;
        let endpoint =
            result.expect("build_streaming_endpoint must succeed for a plain http endpoint");
        // Verify the returned Endpoint carries the correct URI.  The streaming variant
        // must NOT call .timeout(), so only connect_timeout is applied.
        assert_eq!(
            endpoint.uri().to_string(),
            "http://localhost:50051/",
            "build_streaming_endpoint: Endpoint URI must round-trip to the input URL"
        );
    }

    /// `build_streaming_endpoint` rejects the same invalid TLS combinations that
    /// `build_endpoint` rejects — `validate_transport_security` is called in both paths.
    // F-RR-C33-01: build_streaming_endpoint is now async; use #[tokio::test].
    #[tokio::test]
    async fn build_streaming_endpoint_rejects_tls_without_domain() {
        let config = GrpcConfig {
            use_tls: true,
            tls_domain_name: None,
            ..GrpcConfig::default()
        };
        let result = config
            .build_streaming_endpoint("https://localhost:50051")
            .await;
        let cfg_err = result.unwrap_err();
        // Iter-99 moved this validation to the typed ADR-019 form:
        // "is required when ..." = ConfigCode::Missing under CoreError::Config.
        assert!(
            matches!(
                &cfg_err,
                crate::NetworkError::Core(maekon_core::error::CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    ..
                })
            ),
            "build_streaming_endpoint must fail with the typed Missing config error when tls_domain_name is missing; got: {cfg_err:?}"
        );
    }
}
