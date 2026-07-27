use async_trait::async_trait;
use maekon_core::ports::provider_model_catalog::{
    ProviderModelCatalogError, ProviderModelCatalogPort, ProviderModelCatalogRequest,
    ProviderModelCatalogResponse,
};
use reqwest::header::{HeaderName, HeaderValue};

use crate::ANTHROPIC_API_VERSION;

#[derive(Debug, Clone, Default)]
pub struct ReqwestProviderModelCatalogClient;

impl ReqwestProviderModelCatalogClient {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ProviderModelCatalogPort for ReqwestProviderModelCatalogClient {
    async fn fetch_models(
        &self,
        request: ProviderModelCatalogRequest,
    ) -> Result<ProviderModelCatalogResponse, ProviderModelCatalogError> {
        // #6892: redirect=none — model discovery is also a provider egress that carries
        // api-key/Bearer headers, so disable 30x following (F1 sibling; SSRF host guard is #6894).
        // #8045 C3: https_only backstop. When the maekon-web SSRF guard has already
        // resolved+validated the host and pinned it to loopback address(es) (see the
        // `resolved_addrs` pin below), the actual connection target is loopback, so
        // cleartext is safe even if the endpoint's host string is not literally
        // loopback. Otherwise derive from the endpoint URL — remote endpoints are
        // HTTPS-only, loopback endpoints (e.g. Ollama) keep cleartext.
        let pinned_all_loopback = !request.resolved_addrs.is_empty()
            && request.resolved_addrs.iter().all(|a| a.ip().is_loopback());
        let transport_policy = if pinned_all_loopback {
            crate::outbound::TransportPolicy::AllowLoopbackCleartext
        } else {
            crate::outbound::TransportPolicy::for_endpoint(&request.endpoint)
        };
        let mut builder =
            crate::outbound::hardened_client_builder(transport_policy).timeout(request.timeout);

        // #6902: when the caller (maekon-web SSRF guard) has already resolved and validated
        // the endpoint host and passed it via `resolved_addrs`, pin that host to these
        // addresses so the transport cannot re-resolve DNS (closing the TOCTOU rebinding
        // window). Apply `resolve_to_addrs` with the bracket-stripped host key
        // (analysis_client::build_loopback_pinned_client precedent; an IP-literal host makes
        // reqwest skip DNS, so the pin is irrelevant either way).
        if !request.resolved_addrs.is_empty() {
            if let Some(host_key) = reqwest::Url::parse(&request.endpoint)
                .ok()
                .and_then(|url| url.host_str().map(host_key_for_pin))
            {
                builder = builder.resolve_to_addrs(&host_key, &request.resolved_addrs);
            }
        }

        let client = builder
            .build()
            .map_err(|error| ProviderModelCatalogError::ClientBuild(error.to_string()))?;

        let mut builder = client.get(&request.endpoint);
        for header in request.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes()).map_err(|error| {
                ProviderModelCatalogError::InvalidHeader(format!("{}: {error}", header.name))
            })?;
            let value = HeaderValue::from_str(&header.value).map_err(|error| {
                ProviderModelCatalogError::InvalidHeader(format!("{}: {error}", header.name))
            })?;
            builder = builder.header(name, value);
        }

        let response = builder
            .send()
            .await
            .map_err(|error| ProviderModelCatalogError::Request(error.to_string()))?;
        let status = response.status().as_u16();
        // #6949: cap the BYOK provider model-catalog response body — a compromised/
        // MITM provider (this carries the api-key/Bearer) could otherwise stream
        // multi-GB and OOM the agent. The multiline `.text().await` here evaded the
        // single-line grep in the #6939/#6940 sweep (the recurring blind spot).
        let body =
            crate::outbound::read_text_capped(response, crate::outbound::MAX_AI_RESPONSE_BYTES)
                .await
                .map_err(|e| {
                    ProviderModelCatalogError::ResponseBody(match e {
                        crate::outbound::BodyReadError::Transport(error) => error.to_string(),
                        crate::outbound::BodyReadError::TooLarge { len, cap } => {
                            format!("response too large: {len} bytes exceeds cap {cap}")
                        }
                    })
                })?;

        Ok(ProviderModelCatalogResponse { status, body })
    }
}

/// Host key normalization for reqwest `resolve_to_addrs`: an IPv6-literal host comes from
/// `url.host_str()` wrapped in `[..]` brackets, so strip them (reqwest's resolver map keys
/// on the bare host). Other hosts are left as-is.
fn host_key_for_pin(host: &str) -> String {
    host.strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host)
        .to_string()
}

pub fn anthropic_version_header() -> (&'static str, &'static str) {
    ("anthropic-version", ANTHROPIC_API_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::provider_model_catalog::ProviderModelCatalogRequest;
    use std::time::Duration;

    fn req(
        endpoint: String,
        resolved_addrs: Vec<std::net::SocketAddr>,
    ) -> ProviderModelCatalogRequest {
        ProviderModelCatalogRequest {
            endpoint,
            headers: Vec::new(),
            timeout: Duration::from_secs(5),
            resolved_addrs,
        }
    }

    /// #6902: with the host left as an unresolvable fake name and mockito's real address
    /// pinned via `resolved_addrs`, the transport must connect to the pinned address without
    /// re-resolving DNS, so the request reaches it. This proves the guard-validated address is
    /// actually pinned all the way down to the transport (rebinding blocked).
    #[tokio::test]
    async fn pinned_addrs_bypass_host_resolution() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("GET", "/v1/models")
            .with_status(200)
            .with_body("{\"models\":[]}")
            .create_async()
            .await;
        let port = server.socket_address().port();
        // Unresolvable host — without the pin the connection itself fails.
        let endpoint = format!("http://pinned-host.invalid:{port}/v1/models");

        let client = ReqwestProviderModelCatalogClient::new();
        let resp = client
            .fetch_models(req(endpoint, vec![server.socket_address()]))
            .await
            .expect("pin 된 주소로 요청이 도달해야 한다");

        assert_eq!(resp.status, 200, "pin 된 mockito 가 200 을 반환해야 한다");
        m.assert_async().await;
    }

    /// Control: without the pin, the same unresolvable host must fail to connect (reinforcing
    /// that the test above succeeds thanks to the pin).
    #[tokio::test]
    async fn unpinned_unresolvable_host_fails() {
        let endpoint = "http://pinned-host.invalid:9/v1/models".to_string();
        let client = ReqwestProviderModelCatalogClient::new();
        let result = client.fetch_models(req(endpoint, Vec::new())).await;
        assert!(
            matches!(result, Err(ProviderModelCatalogError::Request(_))),
            "핀 없는 해소 불가 host 는 Request 에러여야 한다: {result:?}"
        );
    }
}
