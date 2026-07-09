// OOS-TBD: ADR-013 file split (cycle 35+) — LOC: 869
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use crate::runtime_state::SecretBackendCapabilities;
use crate::subprocess_provider::{
    cli_discovery_report_for_detected, probe_for_surface_id, probe_known_cli_surfaces,
    runtime_ready_for_surface, runtime_supported_for_surface, ProbedSubprocessCli,
    SubprocessCliAuthStatus, SubprocessCliDependencyStatus, SubprocessCliDiscoveryReport,
};
use maekon_api_contracts::provider_specs::{
    provider_surface_catalog, ProviderAuthScheme, ProviderSurfaceSpec, SurfaceExecutionKind,
    SurfacePlacementKind, SurfaceStability,
};
use maekon_core::models::provider_cli_diagnostics::ProviderCliDiagnosticSummary;
use maekon_web::services::provider_cli_diagnostics::{
    ProviderCliDiagnosticsFuture, ProviderCliDiagnosticsProvider,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureMaturity {
    Stable,
    Beta,
    Experimental,
    Deprecated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureAvailability {
    Available,
    Unavailable,
    PartiallyAvailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCliReadiness {
    NotDetected,
    AuthRequired,
    AuthUnsupported,
    AuthStale,
    InteractiveRequired,
    AuthUnverified,
    AuthReady,
    InvocationReady,
    RuntimeUnsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureCapability {
    pub feature_id: String,
    pub maturity: FeatureMaturity,
    pub availability: FeatureAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_cli_readiness: Option<ProviderCliReadiness>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_cli_discovery: Option<SubprocessCliDiscoveryReport>,
    pub preferred: bool,
    pub requires: Vec<String>,
    pub status_reason: Option<String>,
    pub status_copy_key: Option<String>,
    pub setup_copy_key: Option<String>,
    pub setup_docs_url: Option<String>,
    pub configuration_env_vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureCapabilitySnapshot {
    pub features: Vec<FeatureCapability>,
    /// COMPILE-capability flag (#7600): whether this binary was built with the
    /// `audio` cargo feature. `maekon-audio` (cpal capture + Whisper STT) is
    /// compiled OUT of the shipped `grpc,windows-sandbox` release build, so
    /// `config.audio.enabled` alone is not a truthful signal — a user can flip
    /// it on and start a model download that dead-ends in `service.unavailable`.
    /// The frontend AudioTab and chat voice-input entry point read this field
    /// to disable audio controls instead of offering a doomed download.
    pub audio_compiled: bool,
    /// PLATFORM-capability flag (#7678): whether ANY local OCR engine
    /// (platform-native Vision.framework/WinRT, or leptess/Tesseract) is
    /// actually compiled + usable on this platform. See
    /// `maekon_vision::local_ocr_engine_available()`. `false` on Linux in
    /// every shipped build today — native OCR is macOS/Windows-only and
    /// leptess is never enabled — so selecting `ai_provider.ocr_provider =
    /// Local` there silently produces zero OCR regions forever. The
    /// AI-automation settings tab reads this to warn instead of staying
    /// silent.
    pub ocr_available: bool,
    /// PLATFORM-capability flag (#7678): whether `SystemMonitor::current_power_status()`
    /// returns REAL battery/power data (only macOS today — see
    /// `maekon_monitor::system::power_status_platform_supported()`). Windows
    /// and Linux always get an empty `PowerStatus::default()`, so
    /// `schedule.pause_on_battery_saver` is a toggle that can never actually
    /// fire there. The schedule settings tab reads this to disable the
    /// toggle instead of offering a dead one.
    pub power_status_available: bool,
    /// PLATFORM-capability flag (#7678): whether active-window detection is
    /// expected to work reliably on this platform/session. `true` on
    /// macOS/Windows always; on Linux, `false` when a Wayland session is
    /// detected (no single dependable native path — see
    /// `maekon_monitor::active_window_reliable()` and
    /// `maekon_monitor::linux::get_active_window_linux()`'s GNOME/Sway/XWayland
    /// fallback chain). No dedicated settings UI reads this today; it exists
    /// so telemetry/consumers can observe the gap explicitly instead of a
    /// silently-empty active-window signal.
    pub active_window_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderEndpointProbeResult {
    pub surface_id: String,
    pub endpoint_kind: String,
    pub endpoint: String,
    pub availability: FeatureAvailability,
    pub status_reason: Option<String>,
    pub status_copy_key: Option<String>,
}

pub struct FeatureCapabilityState(pub SecretBackendCapabilities);

pub(crate) struct ProviderCliDiagnosticsSnapshotProvider {
    secret_backend: SecretBackendCapabilities,
}

impl ProviderCliDiagnosticsSnapshotProvider {
    pub(crate) fn new(secret_backend: SecretBackendCapabilities) -> Self {
        Self { secret_backend }
    }
}

impl ProviderCliDiagnosticsProvider for ProviderCliDiagnosticsSnapshotProvider {
    fn provider_cli_diagnostics(&self) -> ProviderCliDiagnosticsFuture<'_> {
        let secret_backend = self.secret_backend.clone();
        Box::pin(async move {
            let snapshot = build_feature_capability_snapshot(&secret_backend).await;
            provider_cli_diagnostics_from_snapshot(&snapshot)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointProbeKind {
    LlmApi,
    OcrApi,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderEndpointProbePolicyError {
    ExternalEgressConsentRequired,
}

/// Computes the three PLATFORM-capability descriptor fields shared by both
/// `FeatureCapabilitySnapshot` construction sites below, so the early-return
/// (catalog load failure) and success paths can never drift out of sync with
/// each other (#7678 — mirrors the `audio_compiled: cfg!(feature = "audio")`
/// duplication this same pattern already had at both sites).
fn platform_capability_flags() -> (bool, bool, bool) {
    (
        maekon_vision::local_ocr_engine_available(),
        maekon_monitor::system::power_status_platform_supported(),
        maekon_monitor::active_window_reliable(),
    )
}

pub async fn build_feature_capability_snapshot(
    secret_backend: &SecretBackendCapabilities,
) -> FeatureCapabilitySnapshot {
    let detected_surfaces = tokio::task::spawn_blocking(probe_known_cli_surfaces)
        .await
        .unwrap_or_default();
    build_feature_capability_snapshot_with_probes(secret_backend, &detected_surfaces).await
}

async fn build_feature_capability_snapshot_with_probes(
    secret_backend: &SecretBackendCapabilities,
    detected_surfaces: &[ProbedSubprocessCli],
) -> FeatureCapabilitySnapshot {
    let catalog = match provider_surface_catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to load provider surface catalog for feature capability snapshot."
            );
            let (ocr_available, power_status_available, active_window_available) =
                platform_capability_flags();
            return FeatureCapabilitySnapshot {
                features: Vec::new(),
                audio_compiled: cfg!(feature = "audio"),
                ocr_available,
                power_status_available,
                active_window_available,
            };
        }
    };

    let mut features = Vec::new();
    for surface in &catalog.surfaces {
        match surface.execution_kind {
            SurfaceExecutionKind::ManagedHttp
                if surface
                    .credential_kind
                    .eq_ignore_ascii_case("managed_oauth") =>
            {
                features.push(managed_oauth_feature(surface, secret_backend));
            }
            SurfaceExecutionKind::SubprocessCli => {
                features.push(subprocess_cli_feature(surface, detected_surfaces));
            }
            SurfaceExecutionKind::DirectHttp if surface.availability_probe.is_some() => {
                features.push(probed_http_surface_feature(surface).await);
            }
            _ => {}
        }
    }

    let (ocr_available, power_status_available, active_window_available) =
        platform_capability_flags();
    FeatureCapabilitySnapshot {
        features,
        audio_compiled: cfg!(feature = "audio"),
        ocr_available,
        power_status_available,
        active_window_available,
    }
}

fn managed_oauth_feature(
    surface: &ProviderSurfaceSpec,
    secret_backend: &SecretBackendCapabilities,
) -> FeatureCapability {
    let provider_configured = secret_backend
        .oauth_provider_ids
        .iter()
        .any(|provider_id| provider_id.eq_ignore_ascii_case(&surface.vendor_id));
    let available = secret_backend.oauth_available
        && secret_backend.os_secret_store_available
        && provider_configured;
    let status_reason =
        if !secret_backend.os_secret_store_available || !secret_backend.oauth_available {
            Some("os_secret_store_unavailable".to_string())
        } else if !provider_configured {
            Some("oauth_provider_not_configured".to_string())
        } else {
            None
        };
    let provisioning = (!provider_configured)
        .then_some(surface.provisioning.as_ref())
        .flatten();
    FeatureCapability {
        feature_id: surface.surface_id.clone(),
        maturity: feature_maturity(surface),
        availability: if available {
            FeatureAvailability::Available
        } else {
            FeatureAvailability::Unavailable
        },
        provider_cli_readiness: None,
        provider_cli_discovery: None,
        preferred: surface.preferred_for_product_auth,
        requires: vec![
            "os_secret_store".to_string(),
            format!("oauth_provider:{}", surface.vendor_id),
        ],
        status_reason,
        status_copy_key: Some(surface_status_copy_key(
            &surface.surface_id,
            if available {
                "available"
            } else {
                "unavailable"
            },
        )),
        setup_copy_key: provisioning
            .as_ref()
            .and_then(|value| value.setup_copy_key.clone()),
        setup_docs_url: provisioning
            .as_ref()
            .and_then(|value| value.docs_url.clone()),
        configuration_env_vars: provisioning
            .as_ref()
            .map(|value| value.configuration_env_vars.clone())
            .unwrap_or_default(),
    }
}

fn subprocess_cli_feature(
    surface: &ProviderSurfaceSpec,
    detected_surfaces: &[ProbedSubprocessCli],
) -> FeatureCapability {
    let detected = probe_for_surface_id(detected_surfaces, &surface.surface_id);
    let runtime_supported = runtime_supported_for_surface(&surface.surface_id);
    let provider_cli_discovery =
        detected.map(|surface| cli_discovery_report_for_detected(&surface.detected));
    let provider_cli_readiness = subprocess_cli_readiness(detected, runtime_supported);
    let dependency_blocked = provider_cli_discovery
        .as_ref()
        .is_some_and(|discovery| dependency_blocks_invocation(discovery.dependency_status));
    let availability = match (provider_cli_readiness, dependency_blocked) {
        (_, true) => FeatureAvailability::PartiallyAvailable,
        (ProviderCliReadiness::InvocationReady, false) => FeatureAvailability::Available,
        (ProviderCliReadiness::NotDetected, false) => FeatureAvailability::Unavailable,
        _ => FeatureAvailability::PartiallyAvailable,
    };

    let (status_reason, copy_suffix): (String, &'static str) = match (
        detected,
        provider_cli_readiness,
        provider_cli_discovery.as_ref(),
    ) {
        (Some(_), ProviderCliReadiness::AuthRequired, _) => {
            ("cli_detected_auth_required".to_string(), "auth_required")
        }
        (Some(_), ProviderCliReadiness::AuthUnsupported, _) => {
            ("cli_auth_unsupported".to_string(), "partially_available")
        }
        (Some(_), ProviderCliReadiness::AuthStale, _) => {
            ("cli_auth_stale".to_string(), "partially_available")
        }
        (Some(_), ProviderCliReadiness::InteractiveRequired, _) => (
            "cli_auth_interactive_required".to_string(),
            "partially_available",
        ),
        (Some(_), _, Some(discovery))
            if dependency_blocks_invocation(discovery.dependency_status) =>
        {
            (
                discovery
                    .status_reason
                    .clone()
                    .unwrap_or_else(|| "cli_detected_dependency_missing".to_string()),
                "partially_available",
            )
        }
        (Some(surface), ProviderCliReadiness::InvocationReady, _) => (
            if surface.auth_status == SubprocessCliAuthStatus::Unknown {
                "cli_ready_probe_skipped".to_string()
            } else {
                "cli_ready".to_string()
            },
            "available",
        ),
        (Some(_), ProviderCliReadiness::AuthReady, _) if !runtime_supported => (
            "cli_auth_ready_runtime_unsupported".to_string(),
            "partially_available",
        ),
        (Some(_), ProviderCliReadiness::AuthReady, _) => (
            "cli_auth_ready_runtime_pending".to_string(),
            "partially_available",
        ),
        (Some(_), ProviderCliReadiness::AuthUnverified, _) => (
            "cli_detected_auth_unverified".to_string(),
            "partially_available",
        ),
        (Some(_), ProviderCliReadiness::RuntimeUnsupported, _) => (
            "cli_detected_runtime_unsupported".to_string(),
            "partially_available",
        ),
        (Some(_), ProviderCliReadiness::NotDetected, _) => (
            "cli_detected_auth_unverified".to_string(),
            "partially_available",
        ),
        (None, _, _) => ("cli_not_installed".to_string(), "unavailable"),
    };

    FeatureCapability {
        feature_id: surface.surface_id.clone(),
        maturity: feature_maturity(surface),
        availability,
        provider_cli_readiness: Some(provider_cli_readiness),
        provider_cli_discovery,
        preferred: surface.preferred_for_product_auth,
        requires: surface
            .subprocess_transport
            .as_ref()
            .map(|transport| vec![format!("cli:{}", transport.tool_id)])
            .unwrap_or_default(),
        status_reason: Some(status_reason),
        status_copy_key: Some(surface_status_copy_key(&surface.surface_id, copy_suffix)),
        setup_copy_key: None,
        setup_docs_url: None,
        configuration_env_vars: Vec::new(),
    }
}

fn dependency_blocks_invocation(status: SubprocessCliDependencyStatus) -> bool {
    matches!(
        status,
        SubprocessCliDependencyStatus::Missing | SubprocessCliDependencyStatus::StaleProcessEnv
    )
}

fn subprocess_cli_readiness(
    detected: Option<&ProbedSubprocessCli>,
    runtime_supported: bool,
) -> ProviderCliReadiness {
    let Some(surface) = detected else {
        return ProviderCliReadiness::NotDetected;
    };

    let invocation_ready = runtime_supported
        && runtime_ready_for_surface(&surface.detected.surface_id, surface.auth_status);
    match surface.auth_status {
        SubprocessCliAuthStatus::Authenticated if invocation_ready => {
            ProviderCliReadiness::InvocationReady
        }
        SubprocessCliAuthStatus::Authenticated => ProviderCliReadiness::AuthReady,
        SubprocessCliAuthStatus::Unauthenticated => ProviderCliReadiness::AuthRequired,
        SubprocessCliAuthStatus::Unsupported => ProviderCliReadiness::AuthUnsupported,
        SubprocessCliAuthStatus::StaleSession => ProviderCliReadiness::AuthStale,
        SubprocessCliAuthStatus::InteractiveRequired => ProviderCliReadiness::InteractiveRequired,
        SubprocessCliAuthStatus::Unknown if invocation_ready => {
            ProviderCliReadiness::InvocationReady
        }
        SubprocessCliAuthStatus::Unknown if !runtime_supported => {
            ProviderCliReadiness::RuntimeUnsupported
        }
        SubprocessCliAuthStatus::Unknown => ProviderCliReadiness::AuthUnverified,
    }
}

async fn probed_http_surface_feature(surface: &ProviderSurfaceSpec) -> FeatureCapability {
    if let Some((availability, status_reason, copy_suffix)) = direct_http_runtime_status(surface) {
        return FeatureCapability {
            feature_id: surface.surface_id.clone(),
            maturity: feature_maturity(surface),
            availability,
            provider_cli_readiness: None,
            provider_cli_discovery: None,
            preferred: surface.preferred_for_product_auth,
            requires: vec![format!("endpoint:{}", surface.vendor_id)],
            status_reason: Some(status_reason),
            status_copy_key: Some(surface_status_copy_key(&surface.surface_id, copy_suffix)),
            setup_copy_key: None,
            setup_docs_url: None,
            configuration_env_vars: Vec::new(),
        };
    }

    let availability = match probe_surface_http_reachability(surface).await {
        Ok(true) => FeatureAvailability::Available,
        Ok(false) => FeatureAvailability::Unavailable,
        Err(error) => {
            tracing::warn!(
                surface_id = %surface.surface_id,
                error = %error,
                "Provider surface availability probe failed."
            );
            FeatureAvailability::PartiallyAvailable
        }
    };

    let requires = match surface.placement_kind {
        SurfacePlacementKind::SelfHosted => vec![format!("local_server:{}", surface.vendor_id)],
        SurfacePlacementKind::CustomHosted => vec![format!("endpoint:{}", surface.vendor_id)],
        _ => Vec::new(),
    };
    let (status_reason, copy_suffix) = match availability {
        FeatureAvailability::Available => ("service_reachable", "available"),
        FeatureAvailability::Unavailable => ("service_unreachable", "unavailable"),
        FeatureAvailability::PartiallyAvailable => ("service_probe_failed", "partially_available"),
    };

    FeatureCapability {
        feature_id: surface.surface_id.clone(),
        maturity: feature_maturity(surface),
        availability,
        provider_cli_readiness: None,
        provider_cli_discovery: None,
        preferred: surface.preferred_for_product_auth,
        requires,
        status_reason: Some(status_reason.to_string()),
        status_copy_key: Some(surface_status_copy_key(&surface.surface_id, copy_suffix)),
        setup_copy_key: None,
        setup_docs_url: None,
        configuration_env_vars: Vec::new(),
    }
}

async fn probe_surface_http_reachability(surface: &ProviderSurfaceSpec) -> Result<bool, String> {
    let probe_url = surface
        .availability_probe
        .as_ref()
        .map(|probe| probe.url.clone())
        .ok_or_else(|| {
            format!(
                "Surface '{}' is missing availability_probe.",
                surface.surface_id
            )
        })?;
    probe_surface_http_reachability_at_url(surface, &probe_url).await
}

fn direct_http_runtime_status(
    surface: &ProviderSurfaceSpec,
) -> Option<(FeatureAvailability, String, &'static str)> {
    let transport = surface
        .llm_transport
        .as_ref()
        .filter(|_| surface.supports.llm)
        .or_else(|| {
            surface
                .ocr_transport
                .as_ref()
                .filter(|_| surface.supports.ocr)
        })?;

    let auth_scheme = transport.auth_scheme.trim().to_ascii_lowercase();
    if auth_scheme == "aws_signature_v4" {
        return Some((
            FeatureAvailability::Unavailable,
            "runtime_unsupported:aws_signature_v4".to_string(),
            "unavailable",
        ));
    }

    let request_shape = transport.request_shape.trim().to_ascii_lowercase();
    if request_shape == "bedrock_converse" {
        return Some((
            FeatureAvailability::Unavailable,
            "runtime_unsupported:bedrock_converse".to_string(),
            "unavailable",
        ));
    }

    None
}

async fn probe_surface_http_reachability_at_url(
    surface: &ProviderSurfaceSpec,
    probe_url: &str,
) -> Result<bool, String> {
    let probe = surface.availability_probe.as_ref().ok_or_else(|| {
        format!(
            "Surface '{}' is missing availability_probe.",
            surface.surface_id
        )
    })?;
    let auth_scheme = match probe.auth_scheme.trim().to_ascii_lowercase().as_str() {
        "none" => ProviderAuthScheme::None,
        other => {
            return Err(format!(
                "Self-hosted availability probe for '{}' uses unsupported auth_scheme '{}'.",
                surface.surface_id, other
            ));
        }
    };
    if auth_scheme != ProviderAuthScheme::None {
        return Err(format!(
            "Self-hosted availability probe for '{}' currently requires auth_scheme=none.",
            surface.surface_id
        ));
    }

    // #7698 S2: SSRF hardening. `probe_url` traces back to the WebView-supplied
    // `endpoint` IPC argument (see `probe_provider_surface_endpoint`), so a
    // compromised/malicious renderer could point this backend GET/HEAD at an
    // internal service (loopback, LAN, or the cloud metadata endpoint
    // 169.254.169.254) and use the boolean reachability result as an SSRF
    // oracle. `SelfHosted`/`CustomHosted` surfaces are exempt — their whole
    // purpose is a user-run server that is legitimately on localhost/LAN
    // (e.g. Ollama) — every other placement kind must resolve to a
    // non-internal address.
    let is_explicitly_provisioned_self_hosted = matches!(
        surface.placement_kind,
        SurfacePlacementKind::SelfHosted | SurfacePlacementKind::CustomHosted
    );

    let mut client_builder = reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .redirect(reqwest::redirect::Policy::none());

    if !is_explicitly_provisioned_self_hosted {
        let parsed_probe_url = reqwest::Url::parse(probe_url)
            .map_err(|error| format!("Probe URL is invalid: {error}"))?;
        let host = parsed_probe_url
            .host_str()
            .ok_or_else(|| "Probe URL has no host".to_string())?
            .to_string();
        let port = parsed_probe_url
            .port_or_known_default()
            .ok_or_else(|| "Probe URL has no resolvable port".to_string())?;

        let validated_addrs = resolve_and_validate_probe_host(&host, port).await?;
        // Pin the actual request to the exact addresses just validated, so a
        // second, independent DNS answer at connect time (DNS rebinding)
        // cannot swap in a blocked address after this check passes — closes
        // the resolve-vs-connect TOCTOU gap instead of only narrowing it.
        client_builder = client_builder.resolve_to_addrs(&host, &validated_addrs);
    }

    let client = client_builder
        .build()
        .map_err(|error| format!("Failed to build availability probe client: {error}"))?;
    let method = probe.method.trim().to_ascii_uppercase();
    let response = match method.as_str() {
        "GET" => client.get(probe_url).send().await,
        "HEAD" => client.head(probe_url).send().await,
        other => {
            return Err(format!(
                "Self-hosted availability probe for '{}' uses unsupported method '{}'.",
                surface.surface_id, other
            ));
        }
    }
    .map_err(|error| format!("Availability probe request failed: {error}"))?;

    Ok(response.status().is_success())
}

pub async fn probe_provider_surface_endpoint(
    surface_id: &str,
    endpoint_kind: &str,
    endpoint: &str,
    allow_external_egress: bool,
    consent: &maekon_core::consent::ConsentPermissions,
) -> ProviderEndpointProbeResult {
    let normalized_endpoint = endpoint.trim().to_string();
    let sanitized_endpoint = sanitize_endpoint_for_probe_result(&normalized_endpoint);
    let copy_key = |suffix: &str| Some(surface_status_copy_key(surface_id, suffix));

    // Parsed once, up front: both the consent-authority selection below (LLM
    // vs. OCR dual-consent field) and the later probe-URL resolution need it.
    let parsed_kind = match parse_endpoint_probe_kind(endpoint_kind) {
        Ok(kind) => kind,
        Err(error) => {
            return ProviderEndpointProbeResult {
                surface_id: surface_id.to_string(),
                endpoint_kind: endpoint_kind.to_string(),
                endpoint: sanitized_endpoint,
                availability: FeatureAvailability::PartiallyAvailable,
                status_reason: Some(format!("endpoint_kind_invalid:{error}")),
                status_copy_key: copy_key("partially_available"),
            };
        }
    };

    if let Err(error) = evaluate_provider_endpoint_probe_policy(
        &normalized_endpoint,
        parsed_kind,
        allow_external_egress,
        consent,
    ) {
        let status_reason = match error {
            ProviderEndpointProbePolicyError::ExternalEgressConsentRequired => {
                "external_probe_blocked:consent_required"
            }
        };
        audit_provider_endpoint_probe_policy(
            surface_id,
            endpoint_kind,
            &sanitized_endpoint,
            "denied",
            status_reason,
        );
        return ProviderEndpointProbeResult {
            surface_id: surface_id.to_string(),
            endpoint_kind: endpoint_kind.to_string(),
            endpoint: sanitized_endpoint,
            availability: FeatureAvailability::Unavailable,
            status_reason: Some(status_reason.to_string()),
            status_copy_key: copy_key("unavailable"),
        };
    }
    audit_provider_endpoint_probe_policy(
        surface_id,
        endpoint_kind,
        &sanitized_endpoint,
        "allowed",
        if allow_external_egress {
            "opt_in_and_server_side_consent_granted"
        } else {
            "loopback_endpoint"
        },
    );

    let surface = match maekon_api_contracts::provider_specs::provider_surface_spec(surface_id) {
        Ok(surface) => surface,
        Err(error) => {
            return ProviderEndpointProbeResult {
                surface_id: surface_id.to_string(),
                endpoint_kind: endpoint_kind.to_string(),
                endpoint: sanitized_endpoint,
                availability: FeatureAvailability::PartiallyAvailable,
                status_reason: Some(format!("surface_missing:{error}")),
                status_copy_key: copy_key("partially_available"),
            };
        }
    };

    if let Some((availability, status_reason, copy_suffix)) = direct_http_runtime_status(surface) {
        return ProviderEndpointProbeResult {
            surface_id: surface.surface_id.clone(),
            endpoint_kind: endpoint_kind.to_string(),
            endpoint: sanitized_endpoint,
            availability,
            status_reason: Some(status_reason),
            status_copy_key: copy_key(copy_suffix),
        };
    }

    let probe_url = match probe_url_for_endpoint(surface, parsed_kind, endpoint) {
        Ok(url) => url,
        Err(error) => {
            return ProviderEndpointProbeResult {
                surface_id: surface.surface_id.clone(),
                endpoint_kind: endpoint_kind.to_string(),
                endpoint: sanitized_endpoint,
                availability: FeatureAvailability::PartiallyAvailable,
                status_reason: Some(format!("probe_url_invalid:{error}")),
                status_copy_key: copy_key("partially_available"),
            };
        }
    };

    let (availability, status_reason, copy_suffix) =
        match probe_surface_http_reachability_at_url(surface, &probe_url).await {
            Ok(true) => (
                FeatureAvailability::Available,
                Some("service_reachable".to_string()),
                "available",
            ),
            Ok(false) => (
                FeatureAvailability::Unavailable,
                Some("service_unreachable".to_string()),
                "unavailable",
            ),
            Err(error) => (
                FeatureAvailability::PartiallyAvailable,
                Some(format!("service_probe_failed:{error}")),
                "partially_available",
            ),
        };

    ProviderEndpointProbeResult {
        surface_id: surface.surface_id.clone(),
        endpoint_kind: endpoint_kind.to_string(),
        endpoint: sanitized_endpoint,
        availability,
        status_reason,
        status_copy_key: copy_key(copy_suffix),
    }
}

fn parse_endpoint_probe_kind(raw: &str) -> Result<EndpointProbeKind, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "llm_api" => Ok(EndpointProbeKind::LlmApi),
        "ocr_api" => Ok(EndpointProbeKind::OcrApi),
        other => Err(format!("Unsupported endpoint kind '{other}'.")),
    }
}

fn probe_url_for_endpoint(
    surface: &ProviderSurfaceSpec,
    endpoint_kind: EndpointProbeKind,
    endpoint: &str,
) -> Result<String, String> {
    let configured_endpoint = reqwest::Url::parse(endpoint.trim())
        .map_err(|error| format!("Configured endpoint is invalid: {error}"))?;
    let default_endpoint = default_surface_transport_url(surface, endpoint_kind)?;
    let probe = surface.availability_probe.as_ref().ok_or_else(|| {
        format!(
            "Surface '{}' is missing availability_probe.",
            surface.surface_id
        )
    })?;
    let default_probe = reqwest::Url::parse(&probe.url)
        .map_err(|error| format!("Availability probe URL is invalid: {error}"))?;

    let configured_path = configured_endpoint.path().to_string();
    let default_endpoint_path = default_endpoint.path().to_string();
    let probe_path = default_probe.path().to_string();

    let resolved_path =
        if !default_endpoint_path.is_empty() && configured_path.ends_with(&default_endpoint_path) {
            let prefix_len = configured_path.len() - default_endpoint_path.len();
            format!("{}{}", &configured_path[..prefix_len], probe_path)
        } else {
            probe_path
        };

    let mut resolved = configured_endpoint;
    resolved.set_path(&resolved_path);
    resolved.set_query(default_probe.query());
    resolved.set_fragment(None);
    Ok(resolved.to_string())
}

/// #7698 S2: decide whether the backend may issue the outbound probe request.
///
/// `allow_external_egress` is a WebView/renderer-supplied UI toggle and MUST
/// NOT be the sole authority for an outbound network call — before this fix
/// it was: `allow_external_egress: true` alone bypassed every check. It is
/// now, at most, an ADDITIONAL opt-in on top of the durable, server-side
/// `consent` snapshot (`ConsentManager::effective_permissions()`), which is
/// the actual security decision.
///
/// The consent field consulted is the exact same "dual consent" authority
/// that already gates real external OCR/LLM egress elsewhere in this crate —
/// `full_text_extraction` for `llm_api` (see
/// `provider_adapters::ensure_external_llm_allowed`) and `ocr_processing` for
/// `ocr_api` (see `PrivacyGateway::prepare_image_for_external_with_override`)
/// — so this probe cannot grant broader network access than the app's actual
/// external-AI egress paths already require.
///
/// Fail-closed: a loopback/localhost endpoint is the only case that needs no
/// consent; an endpoint whose external/internal status cannot even be
/// determined (unparseable URL, missing host) is treated as external rather
/// than silently allowed.
fn evaluate_provider_endpoint_probe_policy(
    endpoint: &str,
    endpoint_kind: EndpointProbeKind,
    allow_external_egress: bool,
    consent: &maekon_core::consent::ConsentPermissions,
) -> Result<(), ProviderEndpointProbePolicyError> {
    if endpoint_requires_external_egress(endpoint) == Some(false) {
        return Ok(());
    }

    let server_side_permitted = match endpoint_kind {
        EndpointProbeKind::LlmApi => consent.full_text_extraction,
        EndpointProbeKind::OcrApi => consent.ocr_processing,
    };

    if allow_external_egress && server_side_permitted {
        Ok(())
    } else {
        Err(ProviderEndpointProbePolicyError::ExternalEgressConsentRequired)
    }
}

fn endpoint_requires_external_egress(endpoint: &str) -> Option<bool> {
    let url = reqwest::Url::parse(endpoint.trim()).ok()?;
    let host = url.host_str()?;
    if host.eq_ignore_ascii_case("localhost") {
        return Some(false);
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(!ip.is_loopback());
    }

    Some(true)
}

/// #7698 S2: SSRF address blocklist for the provider-endpoint probe.
///
/// Rejects loopback (127.0.0.0/8, ::1), RFC 1918 private ranges (10.0.0.0/8,
/// 172.16.0.0/12, 192.168.0.0/16), link-local (169.254.0.0/16 — this
/// includes the cloud metadata endpoint 169.254.169.254), the unspecified
/// addresses (0.0.0.0, ::), and IPv6 unique-local (fc00::/7). IPv4-mapped
/// IPv6 addresses (`::ffff:a.b.c.d`) are unwrapped and re-checked against the
/// same IPv4 rules so that representation cannot be used to smuggle a
/// blocked address past the filter.
///
/// Implemented against explicit bit ranges / stable `std::net` predicates
/// rather than nightly-only helpers like `is_global()`, so this compiles on
/// the stable toolchain this workspace targets.
///
/// #7723: this is `maekon_core::net_policy::InternalRangePolicy::ssrf_blocklist()`
/// — the minimal baseline (no CGNAT/NAT64/multicast extras; see
/// `maekon-web`'s `ai_model_catalog_endpoint::is_internal_ip` for the stricter
/// sibling policy and why the two intentionally differ).
fn is_blocked_probe_address(ip: IpAddr) -> bool {
    maekon_core::net_policy::InternalRangePolicy::ssrf_blocklist().is_internal(ip)
}

/// Resolve `host:port` and verify EVERY resolved address against
/// `is_blocked_probe_address`. Returns the validated address list (which the
/// caller pins the actual HTTP client to via `resolve_to_addrs`, so a later
/// independent DNS answer at connect time cannot substitute a blocked
/// address — closing the resolve-vs-connect TOCTOU/DNS-rebind gap rather
/// than merely narrowing it).
///
/// Fail-closed: both a resolution error and an empty answer set are treated
/// as a block (`Err`), matching the rest of this probe's fail-closed posture.
async fn resolve_and_validate_probe_host(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| format!("DNS resolution failed for '{host}': {error}"))?
        .collect();

    if addrs.is_empty() {
        return Err(format!("DNS resolution for '{host}' returned no addresses"));
    }

    if let Some(blocked) = addrs
        .iter()
        .find(|addr| is_blocked_probe_address(addr.ip()))
    {
        return Err(format!(
            "resolved address {} for '{host}' is in a blocked (loopback/private/link-local) range",
            blocked.ip()
        ));
    }

    Ok(addrs)
}

fn sanitize_endpoint_for_probe_result(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    let Ok(mut url) = reqwest::Url::parse(trimmed) else {
        return trimmed.to_string();
    };

    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn audit_provider_endpoint_probe_policy(
    surface_id: &str,
    endpoint_kind: &str,
    sanitized_endpoint: &str,
    decision: &str,
    reason: &str,
) {
    tracing::info!(
        target: "provider_endpoint_probe.audit",
        surface_id = %surface_id,
        endpoint_kind = %endpoint_kind,
        endpoint = %sanitized_endpoint,
        decision = %decision,
        reason = %reason,
        "provider endpoint probe policy decision"
    );
}

fn default_surface_transport_url(
    surface: &ProviderSurfaceSpec,
    endpoint_kind: EndpointProbeKind,
) -> Result<reqwest::Url, String> {
    let raw = match endpoint_kind {
        EndpointProbeKind::LlmApi => surface
            .llm_transport
            .as_ref()
            .map(|transport| transport.url.as_str())
            .ok_or_else(|| {
                format!(
                    "Surface '{}' does not define an llm_transport.",
                    surface.surface_id
                )
            })?,
        EndpointProbeKind::OcrApi => surface
            .ocr_transport
            .as_ref()
            .map(|transport| transport.url.as_str())
            .ok_or_else(|| {
                format!(
                    "Surface '{}' does not define an ocr_transport.",
                    surface.surface_id
                )
            })?,
    };

    reqwest::Url::parse(raw).map_err(|error| {
        format!(
            "Default transport URL for '{}' is invalid: {error}",
            surface.surface_id
        )
    })
}

fn feature_maturity(surface: &ProviderSurfaceSpec) -> FeatureMaturity {
    match surface.stability {
        SurfaceStability::Ga => FeatureMaturity::Stable,
        SurfaceStability::Preview => FeatureMaturity::Beta,
        SurfaceStability::Experimental => FeatureMaturity::Experimental,
        SurfaceStability::Deprecated => FeatureMaturity::Deprecated,
    }
}

fn surface_status_copy_key(surface_id: &str, suffix: &str) -> String {
    format!("featureCapability.surface.{surface_id}.{suffix}")
}

pub(crate) fn provider_cli_diagnostics_from_snapshot(
    snapshot: &FeatureCapabilitySnapshot,
) -> Vec<ProviderCliDiagnosticSummary> {
    snapshot
        .features
        .iter()
        .filter(|feature| feature.provider_cli_readiness.is_some())
        .map(|feature| {
            let discovery = feature.provider_cli_discovery.as_ref();
            ProviderCliDiagnosticSummary {
                surface_id: feature.feature_id.clone(),
                tool_id: tool_id_from_requires(&feature.requires),
                candidate_name: discovery.map(|report| report.candidate_name.clone()),
                executable_hint: discovery
                    .and_then(|report| executable_file_name_hint(&report.executable_path)),
                readiness: provider_cli_readiness_wire(
                    feature
                        .provider_cli_readiness
                        .unwrap_or(ProviderCliReadiness::NotDetected),
                )
                .to_string(),
                availability: feature_availability_wire(feature.availability).to_string(),
                dependency_status: discovery
                    .map(|report| dependency_status_wire(report.dependency_status).to_string()),
                status_reason: feature.status_reason.clone().or_else(|| {
                    discovery
                        .and_then(|report| report.status_reason.as_ref())
                        .cloned()
                }),
                env_refresh_required: discovery.is_some_and(|report| report.env_refresh_required),
            }
        })
        .collect()
}

fn tool_id_from_requires(requires: &[String]) -> Option<String> {
    requires
        .iter()
        .find_map(|value| value.strip_prefix("cli:").map(str::to_string))
}

fn executable_file_name_hint(value: &str) -> Option<String> {
    value
        .rsplit(['\\', '/'])
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

fn feature_availability_wire(value: FeatureAvailability) -> &'static str {
    match value {
        FeatureAvailability::Available => "available",
        FeatureAvailability::Unavailable => "unavailable",
        FeatureAvailability::PartiallyAvailable => "partially_available",
    }
}

fn provider_cli_readiness_wire(value: ProviderCliReadiness) -> &'static str {
    match value {
        ProviderCliReadiness::NotDetected => "not_detected",
        ProviderCliReadiness::AuthRequired => "auth_required",
        ProviderCliReadiness::AuthUnsupported => "auth_unsupported",
        ProviderCliReadiness::AuthStale => "auth_stale",
        ProviderCliReadiness::InteractiveRequired => "interactive_required",
        ProviderCliReadiness::AuthUnverified => "auth_unverified",
        ProviderCliReadiness::AuthReady => "auth_ready",
        ProviderCliReadiness::InvocationReady => "invocation_ready",
        ProviderCliReadiness::RuntimeUnsupported => "runtime_unsupported",
    }
}

fn dependency_status_wire(value: SubprocessCliDependencyStatus) -> &'static str {
    match value {
        SubprocessCliDependencyStatus::Ready => "ready",
        SubprocessCliDependencyStatus::Missing => "missing",
        SubprocessCliDependencyStatus::StaleProcessEnv => "stale_process_env",
        SubprocessCliDependencyStatus::NotRequired => "not_required",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn backend_caps(
        oauth_available: bool,
        oauth_provider_ids: &[&str],
    ) -> SecretBackendCapabilities {
        SecretBackendCapabilities {
            os_secret_store_available: oauth_available,
            oauth_available,
            oauth_provider_ids: oauth_provider_ids
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            default_backend_kind: "os_secret_store".into(),
            byok_backend_kind: "os_secret_store".into(),
            fallback_backend_kind: "unavailable".into(),
        }
    }

    #[test]
    fn managed_oauth_feature_is_experimental_and_unavailable_without_keychain() {
        let feature = managed_oauth_feature(
            &provider_surface_catalog()
                .expect("catalog should load")
                .surfaces
                .iter()
                .find(|surface| surface.surface_id == "provider_surface.openai.managed_oauth")
                .expect("openai managed oauth surface should exist")
                .clone(),
            &backend_caps(false, &["openai"]),
        );
        assert_eq!(feature.maturity, FeatureMaturity::Experimental);
        assert_eq!(feature.availability, FeatureAvailability::Unavailable);
        assert_eq!(feature.provider_cli_readiness, None);
        assert!(!feature.preferred);
    }

    #[test]
    fn managed_oauth_feature_is_unavailable_when_provider_is_not_configured() {
        let feature = managed_oauth_feature(
            &provider_surface_catalog()
                .expect("catalog should load")
                .surfaces
                .iter()
                .find(|surface| surface.surface_id == "provider_surface.google.managed_oauth")
                .expect("google managed oauth surface should exist")
                .clone(),
            &backend_caps(true, &["openai"]),
        );
        assert_eq!(feature.availability, FeatureAvailability::Unavailable);
        assert_eq!(feature.provider_cli_readiness, None);
        assert_eq!(
            feature.status_reason.as_deref(),
            Some("oauth_provider_not_configured")
        );
        assert_eq!(
            feature.setup_copy_key.as_deref(),
            Some("featureCapability.surface.provider_surface.google.managed_oauth.setup")
        );
        assert_eq!(
            feature.setup_docs_url.as_deref(),
            Some("https://developers.google.com/identity/protocols/oauth2/native-app")
        );
        assert_eq!(
            feature.configuration_env_vars,
            vec!["MAEKON_GOOGLE_OAUTH_CLIENT_ID".to_string()]
        );
    }

    #[test]
    fn subprocess_cli_feature_is_preferred_but_partial_when_cli_detected() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.openai.subprocess_cli")
            .expect("openai subprocess surface should exist")
            .clone();
        let feature = subprocess_cli_feature(
            &surface,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                    executable_path: "/usr/bin/codex".into(),
                },
                auth_status: SubprocessCliAuthStatus::Unknown,
                auth_detail: Some("probe_failed:test".to_string()),
            }],
        );
        assert_eq!(
            feature.availability,
            FeatureAvailability::PartiallyAvailable
        );
        assert!(feature.preferred);
        assert_eq!(feature.requires, vec!["cli:codex".to_string()]);
        assert_eq!(
            feature.status_reason.as_deref(),
            Some("cli_detected_auth_unverified")
        );
        assert_eq!(
            feature.status_copy_key.as_deref(),
            Some(
                "featureCapability.surface.provider_surface.openai.subprocess_cli.partially_available"
            )
        );
    }

    #[test]
    fn subprocess_cli_feature_is_available_when_cli_is_authenticated() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.openai.subprocess_cli")
            .expect("openai subprocess surface should exist")
            .clone();
        let feature = subprocess_cli_feature(
            &surface,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                    executable_path: "/usr/bin/codex".into(),
                },
                auth_status: SubprocessCliAuthStatus::Authenticated,
                auth_detail: Some("cli_authenticated".to_string()),
            }],
        );
        assert_eq!(feature.maturity, FeatureMaturity::Beta);
        assert_eq!(feature.availability, FeatureAvailability::Available);
        assert_eq!(
            feature.provider_cli_readiness,
            Some(ProviderCliReadiness::InvocationReady)
        );
        assert_eq!(feature.status_reason.as_deref(), Some("cli_ready"));
        assert_eq!(
            feature.status_copy_key.as_deref(),
            Some("featureCapability.surface.provider_surface.openai.subprocess_cli.available")
        );
    }

    #[test]
    fn subprocess_cli_feature_reports_auth_required_when_logged_out() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.anthropic.subprocess_cli")
            .expect("anthropic subprocess surface should exist")
            .clone();
        let feature = subprocess_cli_feature(
            &surface,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                    executable_path: "/usr/bin/claude".into(),
                },
                auth_status: SubprocessCliAuthStatus::Unauthenticated,
                auth_detail: Some("cli_auth_required".to_string()),
            }],
        );
        assert_eq!(
            feature.availability,
            FeatureAvailability::PartiallyAvailable
        );
        assert_eq!(
            feature.status_reason.as_deref(),
            Some("cli_detected_auth_required")
        );
        assert_eq!(
            feature.status_copy_key.as_deref(),
            Some(
                "featureCapability.surface.provider_surface.anthropic.subprocess_cli.auth_required"
            )
        );
    }

    #[test]
    fn subprocess_cli_feature_reports_auth_edge_states() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.openai.subprocess_cli")
            .expect("openai subprocess surface should exist")
            .clone();

        for (auth_status, readiness, status_reason) in [
            (
                SubprocessCliAuthStatus::Unsupported,
                ProviderCliReadiness::AuthUnsupported,
                "cli_auth_unsupported",
            ),
            (
                SubprocessCliAuthStatus::StaleSession,
                ProviderCliReadiness::AuthStale,
                "cli_auth_stale",
            ),
            (
                SubprocessCliAuthStatus::InteractiveRequired,
                ProviderCliReadiness::InteractiveRequired,
                "cli_auth_interactive_required",
            ),
        ] {
            let feature = subprocess_cli_feature(
                &surface,
                &[ProbedSubprocessCli {
                    detected: crate::subprocess_provider::DetectedSubprocessCli {
                        surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                        executable_path: "/usr/bin/codex".into(),
                    },
                    auth_status,
                    auth_detail: Some(status_reason.to_string()),
                }],
            );

            assert_eq!(feature.provider_cli_readiness, Some(readiness));
            assert_eq!(feature.status_reason.as_deref(), Some(status_reason));
            assert_eq!(
                feature.availability,
                FeatureAvailability::PartiallyAvailable
            );
        }
    }

    #[test]
    fn subprocess_cli_feature_reports_auth_ready_when_runtime_is_not_invocable() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.google.antigravity_cli")
            .expect("google antigravity subprocess surface should exist")
            .clone();
        let feature = subprocess_cli_feature(
            &surface,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.google.antigravity_cli".to_string(),
                    executable_path: "/usr/bin/antigravity".into(),
                },
                auth_status: SubprocessCliAuthStatus::Authenticated,
                auth_detail: Some("cli_authenticated".to_string()),
            }],
        );
        assert_eq!(
            feature.availability,
            FeatureAvailability::PartiallyAvailable
        );
        assert_eq!(
            feature.provider_cli_readiness,
            Some(ProviderCliReadiness::AuthReady)
        );
        assert_eq!(
            feature.status_reason.as_deref(),
            Some("cli_auth_ready_runtime_unsupported")
        );
        assert_eq!(
            feature.status_copy_key.as_deref(),
            Some(
                "featureCapability.surface.provider_surface.google.antigravity_cli.partially_available"
            )
        );
    }

    #[test]
    fn subprocess_cli_feature_is_available_when_auth_probe_is_skipped() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.google.subprocess_cli")
            .expect("google subprocess surface should exist")
            .clone();
        let feature = subprocess_cli_feature(
            &surface,
            &[ProbedSubprocessCli {
                detected: crate::subprocess_provider::DetectedSubprocessCli {
                    surface_id: "provider_surface.google.subprocess_cli".to_string(),
                    executable_path: "/usr/bin/gemini".into(),
                },
                auth_status: SubprocessCliAuthStatus::Unknown,
                auth_detail: Some("auth_status_probe_not_implemented".to_string()),
            }],
        );
        assert_eq!(feature.availability, FeatureAvailability::Available);
        assert_eq!(
            feature.status_reason.as_deref(),
            Some("cli_ready_probe_skipped")
        );
        assert_eq!(
            feature.status_copy_key.as_deref(),
            Some("featureCapability.surface.provider_surface.google.subprocess_cli.available")
        );
    }

    #[test]
    fn provider_cli_diagnostics_from_snapshot_keeps_safe_executable_hint() {
        let snapshot = FeatureCapabilitySnapshot {
            features: vec![FeatureCapability {
                feature_id: "provider_surface.openai.subprocess_cli".to_string(),
                maturity: FeatureMaturity::Beta,
                availability: FeatureAvailability::Available,
                provider_cli_readiness: Some(ProviderCliReadiness::InvocationReady),
                provider_cli_discovery: Some(SubprocessCliDiscoveryReport {
                    candidate_name: "codex".to_string(),
                    executable_path: "C:\\Users\\alice\\AppData\\Local\\Programs\\Codex\\codex.exe"
                        .to_string(),
                    version_status:
                        crate::subprocess_provider::SubprocessCliVersionStatus::NotChecked,
                    dependency_status: SubprocessCliDependencyStatus::Ready,
                    status_reason: None,
                    env_refresh_required: false,
                }),
                preferred: true,
                requires: vec!["cli:codex".to_string()],
                status_reason: Some("cli_ready".to_string()),
                status_copy_key: None,
                setup_copy_key: None,
                setup_docs_url: None,
                configuration_env_vars: vec![],
            }],
            audio_compiled: false,
            ocr_available: false,
            power_status_available: false,
            active_window_available: false,
        };

        let diagnostics = provider_cli_diagnostics_from_snapshot(&snapshot);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].surface_id,
            "provider_surface.openai.subprocess_cli"
        );
        assert_eq!(diagnostics[0].tool_id.as_deref(), Some("codex"));
        assert_eq!(diagnostics[0].executable_hint.as_deref(), Some("codex.exe"));
        assert_eq!(diagnostics[0].readiness, "invocation_ready");
        assert_eq!(diagnostics[0].availability, "available");
        assert_eq!(diagnostics[0].dependency_status.as_deref(), Some("ready"));
    }

    #[tokio::test]
    async fn snapshot_contains_expected_feature_ids() {
        let snapshot = build_feature_capability_snapshot(&backend_caps(true, &["openai"])).await;
        let ids: Vec<&str> = snapshot
            .features
            .iter()
            .map(|feature| feature.feature_id.as_str())
            .collect();
        assert!(ids.contains(&"provider_surface.openai.managed_oauth"));
        assert!(ids.contains(&"provider_surface.openai.subprocess_cli"));
        assert!(ids.contains(&"provider_surface.anthropic.subprocess_cli"));
        assert!(ids.contains(&"provider_surface.google.subprocess_cli"));
        assert!(ids.contains(&"provider_surface.ollama.local_http"));
    }

    /// #7600: `audio_compiled` must reflect the real `audio` cargo feature at
    /// build time — not a runtime/config-derived guess. The shipped release
    /// build (`grpc,windows-sandbox`) never enables `audio`, so this is the
    /// signal the frontend AudioTab uses to disable controls instead of
    /// offering a download that dead-ends in `service.unavailable`.
    #[tokio::test]
    async fn snapshot_audio_compiled_matches_cargo_feature() {
        let snapshot = build_feature_capability_snapshot(&backend_caps(true, &["openai"])).await;
        assert_eq!(snapshot.audio_compiled, cfg!(feature = "audio"));
    }

    /// #7678: the three PLATFORM-capability descriptors must carry the exact
    /// per-platform cfg values computed by `platform_capability_flags()` — this
    /// is the regression guard that a future edit which forgets to thread one
    /// of them through a builder site (there are two: the catalog-load-failure
    /// early return and the success path) is caught immediately instead of
    /// silently reverting to a stale/default value.
    #[tokio::test]
    async fn snapshot_platform_capability_flags_match_builder_helper() {
        let snapshot = build_feature_capability_snapshot(&backend_caps(true, &["openai"])).await;
        let (expected_ocr, expected_power, expected_active_window) = platform_capability_flags();
        assert_eq!(snapshot.ocr_available, expected_ocr);
        assert_eq!(snapshot.power_status_available, expected_power);
        assert_eq!(snapshot.active_window_available, expected_active_window);
    }

    /// #7678: `power_status_available` is macOS-only — pin the documented
    /// allow-list directly (not merely re-deriving the same `cfg!` expression)
    /// so this test fails loudly if a future platform gains real power-status
    /// support without updating the doc comment / this assertion together.
    #[tokio::test]
    async fn snapshot_power_status_available_is_macos_only() {
        let snapshot = build_feature_capability_snapshot(&backend_caps(true, &["openai"])).await;
        assert_eq!(snapshot.power_status_available, cfg!(target_os = "macos"));
    }

    /// #7678: `platform_capability_flags()` is a pure cfg-derived computation
    /// (no I/O, no hidden state) — repeated calls must be stable. This is the
    /// property the catalog-load-failure early-return path in
    /// `build_feature_capability_snapshot_with_probes` relies on to stay
    /// honest without a dedicated integration test for that unreachable-in-CI
    /// branch (the bundled provider surface catalog does not fail to load).
    #[test]
    fn platform_capability_flags_is_pure_and_stable() {
        assert_eq!(platform_capability_flags(), platform_capability_flags());
    }

    #[tokio::test]
    async fn self_hosted_surface_feature_declares_local_service_requirement() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.ollama.local_http")
            .expect("ollama surface should exist")
            .clone();
        let feature = probed_http_surface_feature(&surface).await;
        assert_eq!(feature.provider_cli_readiness, None);
        assert!(feature
            .requires
            .iter()
            .any(|value| value == "local_server:ollama"));
    }

    /// ADR-019 regression guard: Bedrock is intentionally unsupported.
    /// The `provider_surface.bedrock.direct_api` catalog entry was removed
    /// in Phase 3 of the error-code-infrastructure migration. This test
    /// asserts the catalog does NOT contain Bedrock — catching accidental
    /// re-introduction without the SigV4 implementation checklist (ADR-019 §5).
    #[tokio::test]
    async fn bedrock_surface_removed_per_adr_019() {
        let catalog = provider_surface_catalog().expect("catalog should load");
        let bedrock_surface = catalog
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.bedrock.direct_api");
        assert!(
            bedrock_surface.is_none(),
            "Bedrock surface must remain absent from catalog (ADR-019 §5)"
        );
        let bedrock_vendor = catalog
            .vendors
            .iter()
            .find(|vendor| vendor.vendor_id == "bedrock");
        assert!(
            bedrock_vendor.is_none(),
            "Bedrock vendor must remain absent from catalog (ADR-019 §5)"
        );
    }

    #[test]
    fn probe_url_for_endpoint_keeps_custom_path_prefix() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.ollama.local_http")
            .expect("ollama surface should exist")
            .clone();

        let url = probe_url_for_endpoint(
            &surface,
            EndpointProbeKind::LlmApi,
            "http://127.0.0.1:11434/edge/ollama/v1/responses",
        )
        .expect("probe url should resolve");

        assert_eq!(url, "http://127.0.0.1:11434/edge/ollama/api/version");
    }

    #[test]
    fn probe_url_for_endpoint_falls_back_to_probe_path_when_suffix_does_not_match() {
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.ollama.local_http")
            .expect("ollama surface should exist")
            .clone();

        let url = probe_url_for_endpoint(
            &surface,
            EndpointProbeKind::LlmApi,
            "http://127.0.0.1:11434/custom-endpoint",
        )
        .expect("probe url should resolve");

        assert_eq!(url, "http://127.0.0.1:11434/api/version");
    }

    #[tokio::test]
    async fn availability_probe_does_not_follow_redirects() {
        let mut server = mockito::Server::new_async().await;
        let redirect = server
            .mock("GET", "/redirect")
            .with_status(302)
            .with_header("location", "/ok")
            .create_async()
            .await;
        let ok = server
            .mock("GET", "/ok")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        let surface = provider_surface_catalog()
            .expect("catalog should load")
            .surfaces
            .iter()
            .find(|surface| surface.surface_id == "provider_surface.ollama.local_http")
            .expect("ollama surface should exist")
            .clone();

        let reachable =
            probe_surface_http_reachability_at_url(&surface, &format!("{}/redirect", server.url()))
                .await
                .expect("probe should complete");

        assert!(!reachable);
        redirect.assert_async().await;
        ok.assert_async().await;
    }

    #[test]
    fn provider_endpoint_probe_policy_requires_gate_for_non_loopback_endpoint() {
        let decision = evaluate_provider_endpoint_probe_policy(
            "https://api.example.com/v1/responses?key=secret#fragment",
            EndpointProbeKind::LlmApi,
            false,
            &maekon_core::consent::ConsentPermissions::default(),
        );

        assert_eq!(
            decision,
            Err(ProviderEndpointProbePolicyError::ExternalEgressConsentRequired)
        );
    }

    #[test]
    fn provider_endpoint_probe_policy_allows_loopback_without_external_gate() {
        // Loopback needs no consent at all — even with both the caller opt-in
        // AND the server-side consent absent, the endpoint itself never
        // requires external-egress authority.
        let decision = evaluate_provider_endpoint_probe_policy(
            "http://127.0.0.1:11434/v1/responses",
            EndpointProbeKind::LlmApi,
            false,
            &maekon_core::consent::ConsentPermissions::default(),
        );

        assert_eq!(decision, Ok(()));
    }

    /// #7698 S2 regression: prior to this fix, `allow_external_egress: true`
    /// (a WebView-supplied boolean) was the SOLE authority —
    /// `evaluate_provider_endpoint_probe_policy` returned `Ok(())`
    /// immediately whenever the caller flag was true, with no server-side
    /// check at all. This proves the fix: the caller flag alone, with the
    /// durable consent snapshot showing NO grant, must still be denied.
    #[test]
    fn provider_endpoint_probe_policy_denies_when_caller_true_but_server_side_consent_absent() {
        let decision = evaluate_provider_endpoint_probe_policy(
            "https://api.example.com/v1/responses",
            EndpointProbeKind::LlmApi,
            true, // caller-supplied opt-in — must NOT be sufficient alone
            &maekon_core::consent::ConsentPermissions::default(), // full_text_extraction: false
        );

        assert_eq!(
            decision,
            Err(ProviderEndpointProbePolicyError::ExternalEgressConsentRequired),
            "caller boolean alone must never authorize external egress"
        );
    }

    /// The inverse of the above: server-side consent granted but the caller
    /// opt-in withheld must ALSO be denied — consent is necessary but not
    /// sufficient either; both signals are required.
    #[test]
    fn provider_endpoint_probe_policy_denies_when_consent_granted_but_caller_opt_in_missing() {
        let consent = maekon_core::consent::ConsentPermissions {
            full_text_extraction: true,
            ..Default::default()
        };
        let decision = evaluate_provider_endpoint_probe_policy(
            "https://api.example.com/v1/responses",
            EndpointProbeKind::LlmApi,
            false,
            &consent,
        );

        assert_eq!(
            decision,
            Err(ProviderEndpointProbePolicyError::ExternalEgressConsentRequired)
        );
    }

    /// Both the caller opt-in AND the matching dual-consent field granted:
    /// the probe is allowed. Covers both `llm_api` (`full_text_extraction`)
    /// and `ocr_api` (`ocr_processing`) — the two existing consent
    /// authorities this policy reuses rather than inventing a new one.
    #[test]
    fn provider_endpoint_probe_policy_allows_when_both_caller_and_consent_granted() {
        let llm_consent = maekon_core::consent::ConsentPermissions {
            full_text_extraction: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_provider_endpoint_probe_policy(
                "https://api.example.com/v1/responses",
                EndpointProbeKind::LlmApi,
                true,
                &llm_consent,
            ),
            Ok(())
        );

        let ocr_consent = maekon_core::consent::ConsentPermissions {
            ocr_processing: true,
            ..Default::default()
        };
        assert_eq!(
            evaluate_provider_endpoint_probe_policy(
                "https://api.example.com/v1/ocr",
                EndpointProbeKind::OcrApi,
                true,
                &ocr_consent,
            ),
            Ok(())
        );

        // Cross-check: `ocr_processing` alone must NOT authorize `llm_api`
        // (each endpoint kind requires its OWN matching consent field).
        assert_eq!(
            evaluate_provider_endpoint_probe_policy(
                "https://api.example.com/v1/responses",
                EndpointProbeKind::LlmApi,
                true,
                &ocr_consent,
            ),
            Err(ProviderEndpointProbePolicyError::ExternalEgressConsentRequired)
        );
    }

    #[test]
    fn provider_endpoint_probe_result_endpoint_strips_credentials_query_and_fragment() {
        let sanitized = sanitize_endpoint_for_probe_result(
            "https://user:secret@api.example.com/v1/responses?api_key=secret#fragment",
        );

        assert_eq!(sanitized, "https://api.example.com/v1/responses");
    }

    #[tokio::test]
    async fn provider_endpoint_probe_denies_remote_without_external_egress_gate() {
        let result = probe_provider_surface_endpoint(
            "provider_surface.ollama.local_http",
            "llm_api",
            "https://api.example.com/v1/responses?api_key=secret",
            false,
            &maekon_core::consent::ConsentPermissions::default(),
        )
        .await;

        assert_eq!(result.availability, FeatureAvailability::Unavailable);
        assert_eq!(
            result.status_reason.as_deref(),
            Some("external_probe_blocked:consent_required")
        );
        assert_eq!(result.endpoint, "https://api.example.com/v1/responses");
    }

    /// #7698 S2 regression at the full `probe_provider_surface_endpoint` entry
    /// point (not just the inner policy function): a caller passing
    /// `allow_external_egress: true` for a `full_text_extraction`-less
    /// consent snapshot must still be denied — proves the fix end-to-end,
    /// including the audit-decision branch.
    #[tokio::test]
    async fn provider_endpoint_probe_denies_when_caller_opts_in_without_consent() {
        let result = probe_provider_surface_endpoint(
            "provider_surface.ollama.local_http",
            "llm_api",
            "https://api.example.com/v1/responses",
            true,                                                 // caller opt-in present
            &maekon_core::consent::ConsentPermissions::default(), // no consent
        )
        .await;

        assert_eq!(result.availability, FeatureAvailability::Unavailable);
        assert_eq!(
            result.status_reason.as_deref(),
            Some("external_probe_blocked:consent_required")
        );
    }

    // ── #7698 S2: SSRF address blocklist ────────────────────────────────

    #[test]
    fn ssrf_blocklist_rejects_ipv4_loopback() {
        assert!(is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            127, 0, 0, 1
        ))));
    }

    #[test]
    fn ssrf_blocklist_rejects_cloud_metadata_link_local() {
        // 169.254.169.254 — AWS/GCP/Azure instance metadata endpoint, the
        // canonical SSRF target this filter must never let through.
        assert!(is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            169, 254, 169, 254
        ))));
    }

    #[test]
    fn ssrf_blocklist_rejects_rfc1918_private_ranges() {
        assert!(is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            10, 0, 0, 5
        ))));
        assert!(is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            172, 16, 0, 1
        ))));
        assert!(is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            192, 168, 1, 1
        ))));
    }

    #[test]
    fn ssrf_blocklist_rejects_unspecified_v4() {
        assert!(is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            0, 0, 0, 0
        ))));
    }

    #[test]
    fn ssrf_blocklist_rejects_ipv6_loopback_and_unique_local() {
        assert!(is_blocked_probe_address(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        // fc00::/7
        assert!(is_blocked_probe_address(IpAddr::V6(Ipv6Addr::new(
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn ssrf_blocklist_rejects_ipv4_mapped_ipv6_of_a_blocked_address() {
        // ::ffff:169.254.169.254 — representation smuggling attempt.
        let mapped = Ipv4Addr::new(169, 254, 169, 254).to_ipv6_mapped();
        assert!(is_blocked_probe_address(IpAddr::V6(mapped)));
    }

    /// Argues the "allows a permitted public host" half of the S2 SSRF
    /// requirement without depending on live DNS/network access in CI —
    /// the blocklist predicate is the actual security boundary; a live
    /// `resolve_and_validate_probe_host` call against a real public host is
    /// an environment-dependent integration concern, not a unit-test one.
    #[test]
    fn ssrf_blocklist_allows_public_addresses() {
        assert!(!is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            8, 8, 8, 8
        ))));
        assert!(!is_blocked_probe_address(IpAddr::V4(Ipv4Addr::new(
            1, 1, 1, 1
        ))));
    }
}
