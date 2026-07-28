use maekon_api_contracts::provider_specs::{
    default_surface_id_for_access_mode as default_surface_id_from_catalog,
    resolved_model_catalog_strategy, resolved_surface_spec, ModelCatalogStrategy,
    ProviderSurfaceSpec, SurfaceCapabilityKind,
};
use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};

use crate::error::ApiError;
use crate::services::ai_provider_spec_service;

pub(crate) fn resolve_models_endpoint(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    endpoint: Option<&str>,
) -> Result<String, ApiError> {
    let endpoint = endpoint.and_then(normalize_optional_endpoint);
    let surface = resolved_surface_spec(provider_type, surface_id).map_err(ApiError::Internal)?;
    if !surface.supports.model_catalog || surface.model_catalog_transport.is_none() {
        return Err(ApiError::BadRequest(format!(
            "Selected provider surface '{}' does not support model discovery.",
            surface.surface_id
        )));
    }
    let catalog_strategy =
        resolved_model_catalog_strategy(provider_type, Some(surface.surface_id.as_str()))
            .map_err(ApiError::Internal)?;

    let default_endpoint = ai_provider_spec_service::default_model_catalog_endpoint_for_surface(
        provider_type,
        Some(surface.surface_id.as_str()),
    )?;

    if let Some(endpoint) = endpoint {
        return match catalog_strategy {
            ModelCatalogStrategy::HttpModelsEndpoint => {
                if let Some(derived) =
                    derive_model_catalog_endpoint_from_surface(surface, &endpoint)
                {
                    Ok(derived)
                } else {
                    Err(ApiError::BadRequest(format!(
                        "Could not derive a model catalog endpoint from '{}' for surface '{}'.",
                        endpoint, surface.surface_id
                    )))
                }
            }
            ModelCatalogStrategy::None | ModelCatalogStrategy::SubprocessProbe => {
                Err(ApiError::BadRequest(format!(
                    "Surface '{}' does not support HTTP model discovery from a custom endpoint.",
                    surface.surface_id
                )))
            }
        };
    }

    Ok(default_endpoint)
}

pub(crate) fn resolve_requested_provider_type(
    raw_provider_type: &str,
    surface_id: Option<&str>,
) -> Result<AiProviderType, ApiError> {
    if let Some(surface_id) = surface_id {
        let surface = maekon_api_contracts::provider_specs::provider_surface_spec(surface_id)
            .map_err(ApiError::BadRequest)?;
        return ai_provider_spec_service::resolve_provider_type(&surface.provider_type);
    }

    ai_provider_spec_service::resolve_provider_type(raw_provider_type)
}

pub(crate) fn saved_endpoint_surface_id(
    config: &AiProviderConfig,
    endpoint: &ExternalApiEndpoint,
    requested_surface_kind: Option<&str>,
) -> Option<String> {
    endpoint
        .surface_id
        .as_deref()
        .and_then(|value| normalize_optional_surface_id(Some(value)))
        .or_else(|| {
            default_surface_id_from_catalog(
                endpoint.provider_type,
                config.access_mode,
                match requested_surface_kind
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "ocr" | "ocr_api" => SurfaceCapabilityKind::Ocr,
                    _ => SurfaceCapabilityKind::Llm,
                },
            )
            .ok()
            .flatten()
            .map(|value| value.to_ascii_lowercase())
        })
}

pub(crate) fn normalize_optional_surface_id(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

pub(crate) fn normalize_optional_endpoint(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.trim_end_matches('/').to_string())
}

pub(crate) fn derive_model_catalog_endpoint_from_surface(
    surface: &ProviderSurfaceSpec,
    endpoint: &str,
) -> Option<String> {
    let normalized_endpoint = normalize_optional_endpoint(endpoint)?;
    let configured = reqwest::Url::parse(&normalized_endpoint).ok()?;
    let catalog_transport = surface.model_catalog_transport.as_ref()?;
    let catalog_url = reqwest::Url::parse(&catalog_transport.url).ok()?;

    if configured.path() == catalog_url.path() {
        return Some(normalized_endpoint);
    }

    let candidate_transports = [
        surface
            .llm_transport
            .as_ref()
            .map(|transport| transport.url.as_str()),
        surface
            .ocr_transport
            .as_ref()
            .map(|transport| transport.url.as_str()),
    ];

    for candidate in candidate_transports.into_iter().flatten() {
        let default_transport = reqwest::Url::parse(candidate).ok()?;
        if let Some(derived) = derive_model_catalog_endpoint_from_transport(
            &configured,
            &default_transport,
            &catalog_url,
        ) {
            return Some(derived);
        }
    }

    if configured.path().is_empty() || configured.path() == "/" {
        return Some(rebased_url(&configured, &catalog_url));
    }

    if same_origin(&configured, &catalog_url) {
        return Some(rebased_url(&configured, &catalog_url));
    }

    None
}

fn derive_model_catalog_endpoint_from_transport(
    configured: &reqwest::Url,
    default_transport: &reqwest::Url,
    catalog_url: &reqwest::Url,
) -> Option<String> {
    let configured_path = configured.path();
    let default_transport_path = default_transport.path();

    if configured_path.ends_with(default_transport_path) {
        let prefix_len = configured_path
            .len()
            .saturating_sub(default_transport_path.len());
        let derived_path = format!("{}{}", &configured_path[..prefix_len], catalog_url.path());
        return Some(rebased_url_with_path(
            configured,
            &derived_path,
            catalog_url,
        ));
    }

    if path_is_prefix_of(configured_path, default_transport_path) {
        return Some(rebased_url(configured, catalog_url));
    }

    None
}

fn rebased_url(base: &reqwest::Url, catalog_url: &reqwest::Url) -> String {
    rebased_url_with_path(base, catalog_url.path(), catalog_url)
}

fn rebased_url_with_path(base: &reqwest::Url, path: &str, catalog_url: &reqwest::Url) -> String {
    let mut resolved = base.clone();
    resolved.set_path(path);
    resolved.set_query(catalog_url.query());
    resolved.set_fragment(None);
    resolved.to_string()
}

fn path_is_prefix_of(prefix_path: &str, full_path: &str) -> bool {
    let prefix = prefix_path.trim_end_matches('/');
    let full = full_path.trim_end_matches('/');
    if prefix.is_empty() || prefix == "/" {
        return true;
    }
    full == prefix || full.starts_with(&format!("{prefix}/"))
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(l, r)| l.eq_ignore_ascii_case(r))
        && left.port_or_known_default() == right.port_or_known_default()
}

/// #6894: SSRF guard for the integration (external-bind) model discovery path.
///
/// Rejects fail-closed when the resolved endpoint's scheme is not http/https, or when the host
/// resolves to an internal address (loopback / private RFC1918 / link-local 169.254 / CGNAT
/// 100.64 / ULA fc00::/7, etc.). The caller (request JSON) can freely specify `endpoint`, and
/// this path is exposed remotely via a `0.0.0.0` bind under `web.allow_external` + integration
/// auth, so without the guard a remote bearer-token holder could probe the host's internal
/// network / metadata service.
///
/// The loopback-bind local path (`discover_provider_models`) runs on the user's own machine and
/// uses legitimate internal endpoints like a localhost Ollama, so this guard is not applied
/// there — the guard is exclusively for the external integration path.
///
/// Returns: the resolved `SocketAddr`s of the validated endpoint host. The caller passes these to
/// the transport as `ProviderModelCatalogRequest.resolved_addrs` to pin the host and prevent the
/// transport from re-resolving — closing the TOCTOU DNS-rebinding window between the guard's
/// resolution and the transport's re-resolution (#6902). For an IP-literal host the result is the
/// same whether pinned or not.
pub(crate) async fn reject_internal_discovery_endpoint(
    endpoint: &str,
) -> Result<Vec<std::net::SocketAddr>, ApiError> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|e| ApiError::BadRequest(format!("Invalid model discovery endpoint URL: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ApiError::BadRequest(format!(
                "Model discovery endpoint scheme '{other}' is not allowed (http/https only)."
            )));
        }
    }
    // socket_addrs may perform DNS resolution and is therefore blocking, so we offload it onto
    // spawn_blocking to avoid stalling the runtime. An IP-literal host resolves immediately without
    // DNS. We additionally wrap it in a 5-second timeout so a hung DNS server cannot occupy a
    // blocking thread for a long time (mitigating thread-pool exhaustion by an authenticated
    // attacker); the timeout is a fail-closed rejection.
    let url_for_resolve = url.clone();
    let resolve_fut = tokio::task::spawn_blocking(move || url_for_resolve.socket_addrs(|| None));
    let addrs: Vec<std::net::SocketAddr> =
        tokio::time::timeout(std::time::Duration::from_secs(5), resolve_fut)
            .await
            .map_err(|_| {
                ApiError::ServiceUnavailable(
                    "Model discovery endpoint DNS resolution timed out.".to_string(),
                )
            })?
            .map_err(|e| ApiError::Internal(format!("endpoint 해소 작업 실패: {e}")))?
            .map_err(|e| {
                ApiError::BadRequest(format!(
                    "Model discovery endpoint host did not resolve: {e}"
                ))
            })?;
    if addrs.is_empty() {
        return Err(ApiError::BadRequest(
            "Model discovery endpoint host did not resolve to any address.".to_string(),
        ));
    }
    // fail-closed: reject if any of the resolved addresses is internal.
    if let Some(internal) = addrs.iter().find(|a| is_internal_ip(a.ip())) {
        return Err(ApiError::Forbidden(format!(
            "Model discovery endpoint resolves to an internal address ({}); refusing to fetch.",
            internal.ip()
        )));
    }
    // Return the validated external addresses so the transport pins to them (#6902 rebinding block).
    Ok(addrs)
}

/// Whether a resolved discovery endpoint targets loopback (the user's own machine).
///
/// #8047 E4: the local discovery path (`discover_provider_models`) is allowed to skip the
/// SSRF/internal-range guard ONLY when it targets loopback — the legitimate case being a
/// localhost Ollama at `127.0.0.1:11434`. A non-loopback endpoint on the local path (e.g. an
/// RFC1918 `192.168.x.x` host) must instead clear the same internal-range guard as the external
/// integration path, so this classifier is the gate that decides which branch runs.
///
/// "Loopback" is `127.0.0.0/8`, `::1`, and the literal host `localhost`. The `localhost`
/// name is trusted by name here (not resolved): on the user's own machine that is the
/// intended local target, and this path is loopback-bound and token-gated (LOW severity).
/// IP-literal hosts are classified by `IpAddr::is_loopback()`, which covers all of
/// `127.0.0.0/8` and `::1`.
pub(crate) fn is_loopback_discovery_endpoint(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // `host_str()` keeps the brackets on an IPv6 literal (`[::1]`); strip them before parsing.
    let host_ip = host.trim_start_matches('[').trim_end_matches(']');
    host_ip
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Whether the address is an internal one that must not be exposed externally — loopback /
/// private / link-local / CGNAT / ULA / unspecified / multicast, etc. IPv4-mapped IPv6 is reduced
/// to its inner v4 and checked.
///
/// #7723: this is `maekon_core::net_policy::InternalRangePolicy::strict_remote_discovery_guard()`
/// — the stricter of the workspace's two SSRF blocklists (see that constructor's doc comment for
/// the full range list and why this call site keeps the extra CGNAT/NAT64/multicast/etc. checks
/// that the `feature_capabilities.rs` SSRF blocklist does not).
fn is_internal_ip(ip: std::net::IpAddr) -> bool {
    maekon_core::net_policy::InternalRangePolicy::strict_remote_discovery_guard().is_internal(ip)
}

#[cfg(test)]
mod ssrf_guard_tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn classifies_internal_ipv4() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254", // cloud metadata
            "0.0.0.0",
            "100.64.0.1", // CGNAT
        ] {
            assert!(
                is_internal_ip(ip.parse::<IpAddr>().unwrap()),
                "{ip} 은 내부로 분류되어야 한다"
            );
        }
    }

    #[test]
    fn classifies_internal_ipv6() {
        for ip in [
            "::1",
            "::",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",    // IPv4-mapped private
            "64:ff9b::a00:1",     // NAT64 well-known prefix → 10.0.0.1 (RFC 6146)
            "64:ff9b::a9fe:a9fe", // NAT64 → 169.254.169.254 (cloud metadata)
        ] {
            assert!(
                is_internal_ip(ip.parse::<IpAddr>().unwrap()),
                "{ip} 은 내부로 분류되어야 한다"
            );
        }
    }

    #[test]
    fn allows_public_ip() {
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(
                !is_internal_ip(ip.parse::<IpAddr>().unwrap()),
                "{ip} 은 외부(허용)로 분류되어야 한다"
            );
        }
    }

    #[tokio::test]
    async fn rejects_internal_literal_endpoints() {
        // IP literals resolve without DNS, so this is testable without a network.
        for url in [
            "http://127.0.0.1:8080/v1/models",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/v1/models",
            "https://192.168.1.10/v1/models",
        ] {
            let r = reject_internal_discovery_endpoint(url).await;
            assert!(
                matches!(r, Err(ApiError::Forbidden(_))),
                "{url} 은 Forbidden 으로 거부되어야 한다: {r:?}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let r = reject_internal_discovery_endpoint("ftp://example.com/x").await;
        assert!(
            matches!(r, Err(ApiError::BadRequest(_))),
            "비-http(s) scheme 은 BadRequest 로 거부되어야 한다: {r:?}"
        );
    }

    #[test]
    fn classifies_loopback_discovery_endpoints() {
        // #8047 E4: loopback endpoints (localhost Ollama etc.) are allowed to skip the guard.
        for url in [
            "http://127.0.0.1:11434/v1/models",
            "http://127.5.6.7:8080/v1/models", // anywhere in 127.0.0.0/8
            "http://localhost:11434/v1/models",
            "http://LOCALHOST:11434/v1/models", // case-insensitive
            "http://[::1]:11434/v1/models",
        ] {
            assert!(
                is_loopback_discovery_endpoint(url),
                "{url} should classify as loopback"
            );
        }
    }

    #[test]
    fn classifies_non_loopback_discovery_endpoints() {
        // Non-loopback endpoints (RFC1918, link-local metadata, public) are NOT loopback and
        // therefore fall through to the internal-range guard on the local path.
        for url in [
            "http://192.168.1.10/v1/models", // RFC1918 private — guard must then block it
            "http://10.0.0.1/v1/models",
            "http://169.254.169.254/latest/meta-data", // cloud metadata
            "https://8.8.8.8/v1/models",               // public — guard then allows it
            "https://api.openai.com/v1/models",        // public domain
        ] {
            assert!(
                !is_loopback_discovery_endpoint(url),
                "{url} should NOT classify as loopback"
            );
        }
    }

    #[tokio::test]
    async fn non_loopback_local_path_endpoints_are_guarded() {
        // End-to-end intent for the local path's non-loopback branch: an RFC1918 host is
        // blocked (Forbidden) while a public host is allowed — mirroring the external path's
        // guard exactly (see `rejects_internal_literal_endpoints` / `allows_public_literal_endpoint`).
        let rfc1918 = "http://192.168.1.10/v1/models";
        assert!(!is_loopback_discovery_endpoint(rfc1918));
        let blocked = reject_internal_discovery_endpoint(rfc1918).await;
        assert!(
            matches!(blocked, Err(ApiError::Forbidden(_))),
            "RFC1918 non-loopback local endpoint must be blocked: {blocked:?}"
        );

        let public = "https://8.8.8.8/v1/models";
        assert!(!is_loopback_discovery_endpoint(public));
        let addrs = reject_internal_discovery_endpoint(public)
            .await
            .unwrap_or_else(|e| panic!("public non-loopback endpoint must be allowed: {e:?}"));
        assert!(
            !addrs.is_empty(),
            "guard returns validated pins for a public host"
        );
    }

    #[tokio::test]
    async fn allows_public_literal_endpoint() {
        // A public IP literal must pass the guard (no network required).
        reject_internal_discovery_endpoint("https://8.8.8.8/v1/models")
            .await
            .unwrap_or_else(|e| panic!("공인 IP endpoint 는 허용되어야 한다: {e:?}"));
    }

    /// #6902: the guard must return the resolved addresses of the validated endpoint host
    /// (for transport pinning). A public IP-literal carries that IP as-is.
    #[tokio::test]
    async fn returns_resolved_addrs_for_pinning() {
        let addrs = reject_internal_discovery_endpoint("https://8.8.8.8:443/v1/models")
            .await
            .expect("공인 endpoint 허용");
        assert!(!addrs.is_empty(), "검증된 주소를 반환해야 한다");
        assert!(
            addrs
                .iter()
                .all(|a| a.ip() == "8.8.8.8".parse::<std::net::IpAddr>().unwrap()),
            "반환 주소는 endpoint IP(8.8.8.8)여야 한다: {addrs:?}"
        );
    }
}
