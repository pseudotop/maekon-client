use std::fs;
use std::time::Duration;

use oneshim_core::config::GrpcConfig as CoreGrpcConfig;
use oneshim_core::config::TlsConfig;
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

        if !self.use_tls {
            return Ok(());
        }

        let domain = self
            .tls_domain_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                // Iter-99: "is required when ..." = Missing semantics.
                NetworkError::Core(oneshim_core::error::CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Missing,
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

    pub fn build_endpoint(&self, endpoint_url: &str) -> Result<Endpoint, NetworkError> {
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
                    let pem = fs::read(path).map_err(|e| {
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
                        NetworkError::Core(oneshim_core::error::CoreError::Config {
                            code: oneshim_core::error_codes::ConfigCode::Missing,
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
                        NetworkError::Core(oneshim_core::error::CoreError::Config {
                            code: oneshim_core::error_codes::ConfigCode::Missing,
                            message:
                                "grpc.tls_client_key_path is required when grpc.mtls_enabled=true"
                                    .to_string(),
                        })
                    })?
                    .trim();

                let cert_pem = fs::read(cert_path).map_err(|e| {
                    NetworkError::Config(format!("failed to read grpc.tls_client_cert_path: {e}"))
                })?;
                let key_pem = fs::read(key_path).map_err(|e| {
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
        let endpoint = self.build_endpoint(endpoint_url)?;
        endpoint
            .connect()
            .await
            .map_err(|e| NetworkError::Http(format!("gRPC connection failed: {e}")))
    }

    pub fn all_endpoints(&self) -> Vec<String> {
        let mut endpoints = vec![self.grpc_endpoint.clone()];

        if let Some(base) = self.grpc_endpoint.rsplit_once(':') {
            let host = base.0;
            for port in &self.grpc_fallback_ports {
                endpoints.push(format!("{}:{}", host, port));
            }
        }

        endpoints
    }
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
            return Err(NetworkError::Core(oneshim_core::error::CoreError::Config {
                code: oneshim_core::error_codes::ConfigCode::Missing,
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

    #[test]
    fn test_tls_requires_domain_name() {
        let config = GrpcConfig {
            use_tls: true,
            tls_domain_name: None,
            ..Default::default()
        };

        let result = config.validate_transport_security();
        assert!(result.is_err());
    }

    #[test]
    fn test_mtls_requires_tls_enabled() {
        let config = GrpcConfig {
            use_tls: false,
            mtls_enabled: true,
            ..Default::default()
        };

        let result = config.validate_transport_security();
        assert!(result.is_err());
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

        let result = config.validate_transport_security();
        assert!(result.is_err());
    }

    #[test]
    fn test_tls_validation_accepts_domain_only() {
        let config = GrpcConfig {
            use_tls: true,
            mtls_enabled: false,
            tls_domain_name: Some("localhost".to_string()),
            ..Default::default()
        };

        let result = config.validate_transport_security();
        assert!(result.is_ok());
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
        let core: oneshim_core::error::CoreError = err.into();
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
        let core: oneshim_core::error::CoreError = err.into();
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
        let core: oneshim_core::error::CoreError = err.into();
        assert_eq!(core.code(), "config.invalid");
    }
}
