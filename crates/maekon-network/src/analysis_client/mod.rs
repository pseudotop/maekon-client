use async_trait::async_trait;
use maekon_api_contracts::provider_specs::{
    self, ProviderAuthScheme, ProviderRequestShape, ProviderTransportKind,
};
use maekon_core::config::{AiProviderType, ExternalApiEndpoint, PiiFilterLevel};
use maekon_core::error::CoreError;
use maekon_core::models::memory_graph::{ClaimStatusChange, RelationEdgeProposal};
use maekon_core::models::suggestion::Suggestion;
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::credential_source::CredentialSource;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::error::NetworkError;
use maekon_http_core::circuit_breaker::{CircuitBreaker, CircuitBreakerRegistry, CircuitState};
use maekon_http_core::provider_error_body::provider_error_message;
use maekon_http_core::resilience::{classify_for_breaker, endpoint_authority, BreakerSignal};

use self::requests::{apply_auth_headers, build_request_body};
use self::responses::{
    candidate_to_suggestion, extract_text, parse_candidates, parse_relation_proposals,
    parse_status_changes,
};

mod requests;
mod responses;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_surface_id;

/// MG-PII-02 DNS-rebind hardening: resolve the endpoint host once, assert EVERY
/// resolved address is loopback, and pin reqwest to those addresses so a
/// subsequent re-resolution cannot drift to a remote IP. Fail-closed: returns
/// `None` on an unparseable URL, a missing host/port, an empty resolution, or
/// any non-loopback address.
fn build_loopback_pinned_client(config: &ExternalApiEndpoint) -> Option<reqwest::Client> {
    let url = reqwest::Url::parse(&config.endpoint).ok()?;
    // Resolve host:port (handles IPv6 + scheme-default ports) — the BLOCKER-2 fix
    // (the prior `endpoint_authority` impl passed "scheme://host:port" to
    // `to_socket_addrs` and always errored, silently disabling all enrichment).
    let addrs: Vec<std::net::SocketAddr> = url.socket_addrs(|| None).ok()?;
    if addrs.is_empty() || !addrs.iter().all(|a| a.ip().is_loopback()) {
        return None; // fail-closed
    }
    // Pin the resolver to the loopback addrs so a `localhost` name cannot drift to
    // a remote IP on re-resolution. `resolve_to_addrs` keys on the bare host
    // (IPv6 brackets stripped); for IP-literal hosts reqwest skips DNS anyway.
    let host = url.host_str()?;
    let host_key = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    // #6892: redirect=none — prevents an analysis-provider 30x from leaking the api-key header + body.
    // #8045 C3: this constructor already fails closed unless the endpoint is provably
    // loopback (checked above), so cleartext is intentionally permitted here.
    maekon_http_core::outbound::hardened_client_builder(
        maekon_http_core::outbound::TransportPolicy::AllowLoopbackCleartext,
    )
    .timeout(std::time::Duration::from_secs(config.timeout_secs))
    .resolve_to_addrs(host_key, &addrs)
    .build()
    .ok()
}

/// Normalize an Ollama endpoint URL to use `/v1/chat/completions`.
///
/// `AnalysisClient` sends a chat-completions shaped body (`requests.rs:22-37`)
/// and parses `choices[0].message.content` (`responses.rs:73`). The provider-
/// surface catalog declares Ollama's `llm_transport` URL as `/v1/responses` with
/// `request_shape: openai_responses` — that is the catalog-canonical pairing used
/// by `RemoteLlmProvider` (which consults `resolved_request_shape`). This file
/// uses a FIXED chat-completions body/parser and must therefore also use the
/// chat-completions URL. The rewrite is:
///
/// - `…/v1/responses`       → `…/v1/chat/completions`  (catalog default)
/// - `…/v1/chat/completions` → unchanged (idempotent)
/// - any other path          → pass-through (unknown suffix)
/// - custom path prefix preserved: `…/edge/ollama/v1/responses` →
///   `…/edge/ollama/v1/chat/completions`
///
/// Only applied when `provider_type == Ollama` in `AnalysisClient::new`.
/// This intentional drift from the catalog request_shape is documented here
/// so future maintainers understand the two separate pairing contexts:
/// (1) RemoteLlmProvider: catalog-shape-aware (openai_responses@/v1/responses)
/// (2) AnalysisClient:    always chat-completions body + choices[] parser
///
/// `new_local_enrichment` delegates to `Self::new`, so the enrichment path also
/// benefits from this rewrite (latent bug fix for wizard-configured Ollama users).
pub(crate) fn ollama_chat_completions_url(endpoint: &str) -> String {
    const FROM: &str = "/v1/responses";
    const TO: &str = "/v1/chat/completions";

    let Ok(mut url) = reqwest::Url::parse(endpoint.trim()) else {
        return endpoint.to_string();
    };
    let path = url.path().to_string();
    if path.ends_with(TO) {
        // Already chat/completions — idempotent.
        return endpoint.to_string();
    }
    if let Some(prefix) = path.strip_suffix(FROM) {
        url.set_path(&format!("{prefix}{TO}"));
        return url.to_string();
    }
    // Unknown suffix — pass through unchanged.
    endpoint.to_string()
}

/// Adapter implementing `AnalysisProvider` by calling a remote LLM API.
/// Reuses the same multi-provider HTTP pattern as `RemoteLlmProvider`.
///
/// D7: Guarded by a per-endpoint `CircuitBreaker` shared across both
/// funnels (`analyze` + `summarize_text`) — a persistent outage at the
/// analysis endpoint fast-fails either call in microseconds.
pub struct AnalysisClient {
    http_client: reqwest::Client,
    endpoint: String,
    /// Credential source resolved at request time.
    ///
    /// `new` populates this from the inline `api_key` field (ApiKey) or from
    /// the Ollama surface (NoAuth), preserving 100% backward-compatible
    /// behaviour for all existing call sites that call `new`.
    /// `new_with_credential` accepts a pre-built source so Settings-persisted
    /// keys can flow through the `StoredSecret` variant.
    credential: CredentialSource,
    model: String,
    provider_type: AiProviderType,
    /// #10055: the configured provider surface, threaded so this client resolves
    /// its auth scheme and servable request shape from the catalog exactly like
    /// its siblings (`RemoteLlmProvider`, `RemoteOcrProvider`, `HttpApiSession`).
    /// Before this it was the ONLY HTTP adapter that ignored `surface_id`, so a
    /// `provider_type`-only `match` decided the auth header — sending
    /// `Authorization: Bearer` to a `x_goog_api_key` surface, and papering over
    /// the `Generic` surface's bearer/none split.
    surface_id: Option<String>,
    timeout_secs: u64,
    breaker: Arc<CircuitBreaker>,
    /// D5 iter-5: sanitize LLM-returned suggestion text before it leaves this
    /// client. LLMs can echo back user-context PII (e.g., "your email
    /// user@example.com..."). Apply at the candidate_to_suggestion exit.
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    pii_level: PiiFilterLevel,
}

impl AnalysisClient {
    pub fn new(
        config: &ExternalApiEndpoint,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        // Derive a credential from the inline api_key, preserving existing
        // behaviour: Ollama uses NoAuth (no key needed), all other providers use
        // ApiKey with the configured inline key.
        let credential = if matches!(config.provider_type, AiProviderType::Ollama) {
            CredentialSource::NoAuth
        } else {
            if config.api_key.is_empty() {
                warn!(
                    "AnalysisClient: empty API key for {:?} provider",
                    config.provider_type
                );
            }
            CredentialSource::ApiKey(config.api_key.clone())
        };

        Self::new_with_credential(config, credential, breaker_registry)
    }

    /// Build an `AnalysisClient` with a pre-resolved credential source.
    ///
    /// Use this constructor when the credential has already been resolved from
    /// the Settings secret store (e.g. via
    /// `CredentialSource::from_api_key_endpoint_for_profile`), so that
    /// `StoredSecret` keys are looked up at request time rather than read once
    /// at construction time.
    ///
    /// Callers that cannot produce a `CredentialSource` (e.g. because no
    /// endpoint credential binding exists) should call `new` instead, which
    /// falls back to the inline `api_key` field.
    pub fn new_with_credential(
        config: &ExternalApiEndpoint,
        credential: CredentialSource,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Self {
        // Normalize the Ollama endpoint to /v1/chat/completions. AnalysisClient
        // always sends a chat-completions shaped body (requests.rs:22-37) and
        // parses choices[0].message.content (responses.rs:73). The catalog default
        // is /v1/responses (openai_responses shape used by RemoteLlmProvider), but
        // Ollama serves chat-completions on /v1/chat/completions — see
        // `ollama_chat_completions_url` doc comment for the intentional drift
        // rationale. new_local_enrichment delegates here, so the enrichment path
        // also benefits (latent bug fix, step 4 / Decision 7).
        let resolved_endpoint = if matches!(config.provider_type, AiProviderType::Ollama) {
            ollama_chat_completions_url(&config.endpoint)
        } else {
            config.endpoint.clone()
        };

        // #6892: redirect=none — prevents an analysis-provider 30x from leaking the api-key header + body.
        // build() only fails on TLS backend initialization failure (process-fatal); in that case we
        // explicitly panic rather than fall back to a redirect-following reqwest::Client::default().
        // #8045 C3: https_only backstop derived from the resolved endpoint (loopback Ollama keeps
        // cleartext; a remote analysis provider is HTTPS-only).
        let http_client = maekon_http_core::outbound::hardened_client_builder(
            maekon_http_core::outbound::TransportPolicy::for_endpoint(&resolved_endpoint),
        )
        .timeout(std::time::Duration::from_secs(config.timeout_secs))
        .build()
        .unwrap_or_else(|error| {
            panic!("hardened analysis HTTP client build failed (TLS backend init): {error}")
        });

        // D7: resolve per-endpoint breaker.
        let breaker_key = endpoint_authority(&resolved_endpoint)
            .unwrap_or_else(|_| format!("malformed::{}", resolved_endpoint));
        let breaker = breaker_registry.get(&breaker_key);

        Self {
            http_client,
            endpoint: resolved_endpoint,
            credential,
            model: config.model.clone().unwrap_or_else(|| {
                crate::default_model_for_provider(&config.provider_type).to_string()
            }),
            provider_type: config.provider_type,
            surface_id: config.surface_id.clone(),
            timeout_secs: config.timeout_secs,
            breaker,
            pii_sanitizer: None,
            pii_level: PiiFilterLevel::Standard,
        }
    }

    /// ADR-023 MG-PII-02: enrichment-only constructor. Fails **closed** —
    /// returns `None` unless the configured endpoint host is provably loopback,
    /// and pins the HTTP client to the resolved loopback address(es) so a
    /// `localhost`-named host cannot re-resolve to a remote IP (DNS-rebind). The
    /// caller degrades to `NoOp` on `None`. Use this — never `new` — for any
    /// path that sends durable memory-graph claim text to an LLM.
    ///
    /// The enrichment path always uses the inline `api_key` (typically empty for
    /// loopback Ollama). If a `StoredSecret` is needed for a local provider that
    /// requires authentication, call `new_with_credential` directly with a
    /// loopback endpoint and a pinned client.
    pub fn new_local_enrichment(
        config: &ExternalApiEndpoint,
        breaker_registry: Arc<CircuitBreakerRegistry>,
    ) -> Option<Self> {
        if !maekon_http_core::outbound::host_is_loopback(&config.endpoint) {
            warn!(
                endpoint = %config.endpoint,
                "MG-PII-02: refusing to build enrichment client for non-loopback endpoint"
            );
            return None;
        }
        let pinned = build_loopback_pinned_client(config)?; // None on resolve/build err (fail-closed)
        let mut client = Self::new(config, breaker_registry);
        client.http_client = pinned;
        Some(client)
    }

    /// D5 iter-5: attach a PII sanitizer for suggestion-text sanitization.
    pub fn with_pii_sanitizer(
        mut self,
        sanitizer: Arc<dyn PiiSanitizer>,
        level: PiiFilterLevel,
    ) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self.pii_level = level;
        self
    }

    /// #10055 AC1: catalog-resolved auth scheme for this client's LLM transport.
    ///
    /// Same resolver the sibling adapters use, so a surface that overrides its
    /// vendor default (notably `Generic`, which splits `bearer` / `none` by
    /// surface) authenticates identically on the suggestion path and the
    /// automation path.
    fn llm_auth_scheme(&self) -> Result<ProviderAuthScheme, NetworkError> {
        provider_specs::resolved_auth_scheme(
            self.provider_type,
            self.surface_id.as_deref(),
            ProviderTransportKind::Llm,
        )
        .map_err(NetworkError::Internal)
    }

    /// #10055 AC2: reject surfaces this client's FIXED body cannot serve.
    ///
    /// Servable shapes, and why:
    /// - `AnthropicMessages` — `build_request_body`'s Anthropic arm + the
    ///   `content[0].text` parser.
    /// - `OpenAiChatCompletions` — the chat-completions arm + `choices[]` parser.
    /// - `OpenAiResponses` — Ollama's catalog shape. `ollama_chat_completions_url`
    ///   deliberately rewrites the URL back to `/v1/chat/completions`, so the
    ///   fixed body IS correct for it. Rejecting it would regress every
    ///   wizard-configured Ollama user (AC4).
    ///
    /// Everything else (Google's `generate_content`, the vision shapes, Bedrock)
    /// would be a malformed request, so it fails closed here instead of on the
    /// wire.
    fn ensure_servable_request_shape(&self) -> Result<(), NetworkError> {
        let shape = provider_specs::resolved_request_shape(
            self.provider_type,
            self.surface_id.as_deref(),
            ProviderTransportKind::Llm,
        )
        .map_err(NetworkError::Internal)?;

        if matches!(
            shape,
            ProviderRequestShape::AnthropicMessages
                | ProviderRequestShape::OpenAiChatCompletions
                | ProviderRequestShape::OpenAiResponses
        ) {
            return Ok(());
        }

        Err(NetworkError::Core(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: format!(
                "The suggestion analysis path cannot use provider surface {surface} \
                 (request shape {shape:?}). It sends a fixed chat-completions body; \
                 choose a chat-completions-compatible surface for analysis.",
                surface = self.surface_id.as_deref().unwrap_or("<default>"),
            ),
        }))
    }

    /// D5 iter-5: helper to sanitize a text fragment via the injected port.
    fn sanitize(&self, text: &str) -> String {
        self.pii_sanitizer
            .as_ref()
            .map(|s| s.sanitize_text(text, self.pii_level))
            .unwrap_or_else(|| text.to_string())
    }

    /// Unwrap the provider's HTTP response envelope to the inner LLM text content
    /// (ADR-023 Phase-2). Returns `""` on any parse/extract failure, so the
    /// caller's tolerant parsers then yield an empty result (degrade, never error).
    fn unwrap_llm_text(&self, response_text: &str) -> String {
        serde_json::from_str::<serde_json::Value>(response_text)
            .ok()
            .and_then(|json| extract_text(self.provider_type, &json).ok())
            .unwrap_or_default()
    }

    /// D7 helper: returns Err if breaker is Open, allowing the caller to
    /// short-circuit before constructing a request.
    fn check_breaker(&self) -> Result<(), CoreError> {
        if matches!(self.breaker.check(), CircuitState::Open { .. }) {
            Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::CircuitOpen,
                message: format!("circuit open for {}", self.endpoint),
            })
        } else {
            Ok(())
        }
    }

    /// D7 helper: record the breaker outcome based on initial HTTP send result.
    /// Called immediately after `.send().await`, before error-mapping, so the
    /// breaker sees the raw transport + status signal.
    fn record_breaker_outcome(&self, send_result: &Result<reqwest::Response, reqwest::Error>) {
        let signal = match send_result {
            Ok(resp) => classify_for_breaker(Some(resp.status().as_u16()), false),
            Err(_) => classify_for_breaker(None, true),
        };
        match signal {
            BreakerSignal::Success => self.breaker.record_success(),
            BreakerSignal::Failure => self.breaker.record_failure(),
            BreakerSignal::Neutral => {}
        }
    }

    /// D5 iter-5: sanitized candidate conversion. Apply `PiiSanitizer` to
    /// `content` and `reasoning` fields before returning the Suggestion.
    fn candidate_to_suggestion_sanitized(
        &self,
        candidate: responses::SuggestionCandidate,
    ) -> Suggestion {
        let mut s = candidate_to_suggestion(candidate);
        s.content = self.sanitize(&s.content);
        s.reasoning = s.reasoning.map(|r| self.sanitize(&r));
        s
    }

    /// Test-visible wrapper so tests can call build_request_body without
    /// constructing a full HTTP request.
    #[cfg(test)]
    pub(crate) fn build_request_body_pub(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> serde_json::Value {
        build_request_body(self.provider_type, &self.model, context_json, system_prompt)
    }

    /// Shared HTTP dispatch used by both `analyze` and `summarize_text`.
    ///
    /// Sends the pre-built JSON body, records the D7 breaker outcome, applies
    /// semantic HTTP status mapping (iter-54..59 pattern), and returns the raw
    /// response text on success.
    ///
    /// Credential resolution (`StoredSecret` / `ManagedOAuth`) happens here at
    /// request time so that OS-keychain lookups are never held across an await
    /// boundary at construction time. Resolution failure is mapped to a
    /// `NetworkError::Auth` — same semantic class as a 401 response — so the
    /// breaker is notified and the caller degrades consistently.
    async fn dispatch(
        &self,
        context_json: &str,
        system_prompt: &str,
        label: &str,
    ) -> Result<String, NetworkError> {
        // ADR-019 §3: Bedrock is intentionally unsupported. Reject in the shared
        // dispatch funnel — before resolving credentials or building a request —
        // so EVERY caller (analyze, summarize_text, extract_relations,
        // detect_contradictions) fails closed with the typed config code instead
        // of sending an OpenAI-shaped body to a Bedrock endpoint. Wrapped in
        // `NetworkError::Core` so the `?`/`From<NetworkError>` round-trip in the
        // callers preserves the exact `UnsupportedProviderBedrock` code.
        //
        // #10055 AC3: kept as its own check, BEFORE the generalized shape guard
        // below, so Bedrock keeps emitting `UnsupportedProviderBedrock` rather
        // than the generic unservable-shape code.
        if matches!(self.provider_type, AiProviderType::Bedrock) {
            return Err(NetworkError::Core(CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::UnsupportedProviderBedrock,
                message: "AWS Bedrock is intentionally unsupported in this build".into(),
            }));
        }

        // #10055 AC2: generalize the guard the Bedrock check above established.
        // This client sends a FIXED chat-completions/Anthropic-messages body and
        // parses a fixed envelope (the deliberate drift documented on
        // `ollama_chat_completions_url`). Sending that body to a surface whose
        // catalog `request_shape` is something else — e.g. Google's
        // `google_generate_content` — produced a malformed request and a
        // confusing downstream 4xx. Fail closed instead, with the surface named.
        self.ensure_servable_request_shape()?;

        // Resolve the bearer token at request time.  NoAuth yields an empty
        // string, ApiKey yields the inline key, StoredSecret/ManagedOAuth
        // performs the async lookup.
        let bearer_token = match self.credential.resolve_bearer_token().await {
            Ok(token) => token,
            Err(e) => {
                warn!("{label} credential resolution failed (treating as auth error): {e}");
                return Err(NetworkError::Auth(format!(
                    "{label} credential resolution failed: {e}"
                )));
            }
        };

        let body = build_request_body(self.provider_type, &self.model, context_json, system_prompt);
        let builder = self
            .http_client
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .json(&body);
        // #10055 AC1: resolve the auth scheme from the catalog (provider_type +
        // surface_id), the same call `RemoteLlmProvider` makes, instead of a
        // provider_type-only match whose catch-all arm sent Bearer to every
        // non-Anthropic surface.
        let builder = apply_auth_headers(builder, self.llm_auth_scheme()?, &bearer_token);

        let send_result = builder.send().await;
        // D7: record breaker outcome based on initial send result.
        self.record_breaker_outcome(&send_result);
        let response = send_result.map_err(|e| {
            // Iter-90: route timeouts through NetworkError::Timeout.
            if e.is_timeout() {
                NetworkError::Timeout {
                    timeout_ms: self.timeout_secs * 1000,
                }
            } else {
                NetworkError::Analysis(format!("{label} request failed: {e}"))
            }
        })?;

        let status = response.status();
        // #6939: cap the response body — a compromised/MITM analysis provider could
        // otherwise stream multi-GB and OOM the agent. Preserve the timeout split.
        let response_text = maekon_http_core::outbound::read_text_capped(
            response,
            maekon_http_core::outbound::MAX_AI_RESPONSE_BYTES,
        )
        .await
        .map_err(|e| match e {
            maekon_http_core::outbound::BodyReadError::Transport(err) if err.is_timeout() => {
                NetworkError::Timeout {
                    timeout_ms: self.timeout_secs * 1000,
                }
            }
            maekon_http_core::outbound::BodyReadError::Transport(err) => {
                NetworkError::Analysis(format!("Failed to read {label} response: {err}"))
            }
            maekon_http_core::outbound::BodyReadError::TooLarge { len, cap } => {
                NetworkError::Analysis(format!(
                    "{label} response exceeded cap {cap} bytes (len {len})"
                ))
            }
        })?;

        if !status.is_success() {
            warn!(status = %status, "{label} error response");
            let message = provider_error_message(label, status, Some(&response_text));
            // Semantic HTTP status mapping per iter-54..59.
            let net_err = match status.as_u16() {
                401 | 403 => NetworkError::Auth(message),
                408 | 504 => NetworkError::Timeout { timeout_ms: 0 },
                429 => NetworkError::RateLimited {
                    retry_after_secs: 60,
                },
                502 | 503 => NetworkError::ServiceUnavailable(message),
                _ => NetworkError::Analysis(message),
            };
            return Err(net_err);
        }

        Ok(response_text)
    }
}

#[async_trait]
impl AnalysisProvider for AnalysisClient {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        // D7: pre-flight breaker check.
        self.check_breaker()?;

        debug!(
            endpoint = %self.endpoint,
            model = %self.model,
            provider = ?self.provider_type,
            timeout = self.timeout_secs,
            "Calling analysis LLM API"
        );

        // ADR-019 §3 Bedrock rejection now lives in `dispatch` (shared by every
        // funnel), so analyze() inherits the guard without a local copy.
        let response_text = self
            .dispatch(context_json, system_prompt, "Analysis API")
            .await?;
        let response_json: serde_json::Value = serde_json::from_str(&response_text)
            .map_err(|e| NetworkError::Analysis(format!("Invalid JSON response: {e}")))?;

        let text = extract_text(self.provider_type, &response_json)?;
        let candidates = parse_candidates(&text)?;

        debug!(
            candidate_count = candidates.len(),
            "Parsed suggestion candidates"
        );

        let suggestions: Vec<Suggestion> = candidates
            .into_iter()
            .map(|c| self.candidate_to_suggestion_sanitized(c))
            .collect();

        Ok(suggestions)
    }

    /// Efficient single-completion call that returns raw text without JSON parsing.
    async fn summarize_text(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<String, CoreError> {
        // D7: pre-flight breaker check.
        self.check_breaker()?;

        debug!(
            endpoint = %self.endpoint,
            model = %self.model,
            provider = ?self.provider_type,
            "Calling summarize_text LLM API"
        );

        let response_text = self
            .dispatch(context_json, system_prompt, "Summarize API")
            .await?;
        let response_json: serde_json::Value = serde_json::from_str(&response_text)
            .map_err(|e| NetworkError::Analysis(format!("Invalid JSON response: {e}")))?;

        let text = extract_text(self.provider_type, &response_json)?;

        if text.trim().is_empty() {
            return Err(NetworkError::Analysis("Empty summary response".into()).into());
        }

        Ok(text.trim().to_string())
    }

    async fn extract_relations(
        &self,
        claims_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<RelationEdgeProposal>, CoreError> {
        // MG-PII-02 call-time gate (defense-in-depth on top of new_local_enrichment):
        // claim text must NEVER reach a non-loopback endpoint. Fail-closed → empty.
        if !maekon_http_core::outbound::host_is_loopback(&self.endpoint) {
            warn!(endpoint = %self.endpoint, "MG-PII-02: extract_relations egress blocked (non-loopback)");
            return Ok(Vec::new());
        }
        if self.check_breaker().is_err() {
            return Ok(Vec::new()); // breaker open → degrade, never Err (F1/AC6)
        }
        match self
            .dispatch(claims_json, system_prompt, "Relation API")
            .await
        {
            Ok(response_text) => Ok(parse_relation_proposals(
                &self.unwrap_llm_text(&response_text),
            )),
            Err(e) => {
                warn!("extract_relations dispatch failed (degrading to empty): {e}");
                Ok(Vec::new())
            }
        }
    }

    async fn detect_contradictions(
        &self,
        claims_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<ClaimStatusChange>, CoreError> {
        if !maekon_http_core::outbound::host_is_loopback(&self.endpoint) {
            warn!(endpoint = %self.endpoint, "MG-PII-02: detect_contradictions egress blocked (non-loopback)");
            return Ok(Vec::new());
        }
        if self.check_breaker().is_err() {
            return Ok(Vec::new());
        }
        match self
            .dispatch(claims_json, system_prompt, "Contradiction API")
            .await
        {
            Ok(response_text) => Ok(parse_status_changes(&self.unwrap_llm_text(&response_text))),
            Err(e) => {
                warn!("detect_contradictions dispatch failed (degrading to empty): {e}");
                Ok(Vec::new())
            }
        }
    }

    fn provider_name(&self) -> &str {
        &self.model
    }
}
