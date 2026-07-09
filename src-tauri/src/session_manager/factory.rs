//! Provider routing — create HttpApi, Subprocess, and LocalLlm sessions.

use std::sync::Arc;
use std::time::Instant;

use tracing::info;
// #5032: `warn!` is only emitted from the `analysis`-gated paths (codex
// app-server version-negotiation + LocalLlm Ollama probe/negotiation).
#[cfg(feature = "analysis")]
use tracing::warn;

use maekon_api_contracts::provider_specs::{self};
// #5032: these catalog/credential/trust imports are only used by the
// `analysis`-gated network-session builders (HttpApi + codex app-server).
#[cfg(feature = "analysis")]
use maekon_api_contracts::provider_specs::{ProviderTransportKind, SurfaceCapabilityKind};
use maekon_core::config::CodexAppServerRollout;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{SessionConfig, SessionState, SessionTransport};
use maekon_core::ports::conversation_session::ConversationSession;
#[cfg(feature = "analysis")]
use maekon_core::ports::credential_source::CredentialSource;

use crate::auditing_session::AuditingSession;
use crate::provider_adapters::GuardedConversationSession;
use crate::session_adapters::claude_session::ClaudeSubprocessSession;
use crate::session_adapters::subprocess_session::GenericSubprocessSession;
#[cfg(feature = "analysis")]
use crate::subprocess_provider::trust;
use crate::subprocess_provider::{
    probe_known_cli_surfaces, runtime_ready_for_surface, DetectedSubprocessCli, ProbedSubprocessCli,
};

// #5032: the codex app-server / HTTP-API / local-LLM conversation transports
// all live in `maekon-network`, which is only compiled when the `analysis`
// feature pulls in `dep:maekon-network`. Gating the imports + their dependent
// methods on `analysis` (with feature-disabled fallbacks below) is what lets
// this file compile under `--no-default-features`. No behaviour change when
// `analysis` is enabled.
#[cfg(feature = "analysis")]
use maekon_network::codex_app_server::{parse_user_agent_version, ClientInfo};
#[cfg(feature = "analysis")]
use maekon_network::codex_app_server_session::{CodexAppServerSession, ThreadConfig};
#[cfg(feature = "analysis")]
use maekon_network::http_api_session::{HttpApiSession, HttpApiSessionInit};
#[cfg(feature = "analysis")]
use maekon_network::local_llm_session::LocalLlmSession;

use super::{ManagedSession, SessionManagerImpl};

/// Tool definitions extracted from the context assembler (if any).
pub(super) type DefaultTools = Option<Vec<maekon_core::models::ai_session::ToolDefinition>>;

// ── LocalLlmTarget ────────────────────────────────────────────────────────────
//
// Defined unconditionally (no `#[cfg(feature = "analysis")]`). The type +
// resolver + builder are needed in session_wiring.rs which has no feature gate;
// maekon-api-contracts is already a non-optional dep (used at factory.rs:12).
// Only the *consumption* inside `create_local_llm_session` remains
// analysis-gated (that fn uses maekon-network types). Matches Decision 1.

/// Resolved Ollama base URL + optional default model from config/catalog.
/// Produced by [`resolve_local_llm_target`] at composition time (session_wiring.rs)
/// so `create_local_llm_session` never needs raw `AppConfig` access.
///
/// Fields are populated unconditionally (`session_wiring.rs` calls
/// `with_local_llm_target` regardless of feature flags) but read only by
/// `create_local_llm_session`, which is `#[cfg(feature = "analysis")]` — under
/// `--no-default-features` nothing reads them (#7743 ctd-W3 A2b follow-up).
/// Kept unconditional per the module comment above (Decision 1): hard-gating
/// the struct would also require gating its tests below, several of which
/// exercise `resolve_local_llm_target` directly and should keep running
/// regardless of `analysis`.
#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
#[derive(Debug, Clone)]
pub(crate) struct LocalLlmTarget {
    /// Scheme + host + port (no trailing path). E.g. `http://localhost:11434`
    /// or `http://192.168.0.10:11434` for a custom LAN host.
    pub(crate) base_url: String,
    /// Default model read from llm_api.model or catalog (qwen3:8b). `None`
    /// signals `create_local_llm_session` to use the catalog resolver.
    pub(crate) default_model: Option<String>,
}

/// Derive the Ollama base URL + default model from `AiProviderConfig`.
///
/// Resolution order:
/// 1. `llm_api` present AND `provider_type == Ollama` → strip the known LLM-
///    transport suffix (`/v1/responses` or `/v1/chat/completions`) from the
///    configured endpoint and keep scheme/host/port + any custom path prefix,
///    mirroring `probe_url_for_endpoint`'s suffix-strip technique
///    (feature_capabilities.rs:674-687). Unknown suffix → origin-only fallback
///    (scheme://host:port), per Decision 5.
/// 2. `llm_api` absent or provider is not Ollama → catalog base: strip the
///    availability-probe path (`/api/version`) from the catalog probe URL.
///    If the catalog lookup itself fails → literal `http://localhost:11434`.
///
/// Never panics; session creation is never allowed to abort at wiring time.
pub(crate) fn resolve_local_llm_target(
    ai: &maekon_core::config::AiProviderConfig,
) -> LocalLlmTarget {
    use maekon_core::config::AiProviderType;

    const OLLAMA_SURFACE: &str = "provider_surface.ollama.local_http";
    const LLM_SUFFIXES: &[&str] = &["/v1/responses", "/v1/chat/completions"];
    const CATALOG_FALLBACK: &str = "http://localhost:11434";

    // Helper: strip scheme://host:port from a URL string.
    fn origin_only(url: &str) -> Option<String> {
        let parsed = reqwest::Url::parse(url).ok()?;
        let scheme = parsed.scheme();
        let host = parsed.host_str()?;
        let port = parsed.port();
        Some(match port {
            Some(p) => format!("{scheme}://{host}:{p}"),
            None => format!("{scheme}://{host}"),
        })
    }

    // Helper: strip a known suffix from the path, keeping any custom prefix,
    // then re-assemble scheme://host:port + prefix (no trailing slash).
    fn strip_llm_suffix(endpoint: &str) -> Option<String> {
        let parsed = reqwest::Url::parse(endpoint).ok()?;
        let path = parsed.path();
        for suffix in LLM_SUFFIXES {
            if let Some(prefix_path) = path.strip_suffix(suffix) {
                let scheme = parsed.scheme();
                let host = parsed.host_str()?;
                let port = parsed.port();
                let base_authority = match port {
                    Some(p) => format!("{scheme}://{host}:{p}"),
                    None => format!("{scheme}://{host}"),
                };
                let result = if prefix_path.is_empty() || prefix_path == "/" {
                    base_authority
                } else {
                    format!("{base_authority}{prefix_path}")
                };
                return Some(result.trim_end_matches('/').to_string());
            }
        }
        None
    }

    // Case 1: Ollama llm_api configured.
    if let Some(ref api) = ai.llm_api {
        if matches!(api.provider_type, AiProviderType::Ollama) {
            let base_url = strip_llm_suffix(&api.endpoint)
                .or_else(|| origin_only(&api.endpoint))
                .unwrap_or_else(|| CATALOG_FALLBACK.to_string());
            let default_model = api.model.clone();
            return LocalLlmTarget {
                base_url,
                default_model,
            };
        }
    }

    // Case 2: no Ollama llm_api — derive from catalog probe URL.
    let base_url = provider_specs::availability_probe(OLLAMA_SURFACE)
        .ok()
        .flatten()
        .and_then(|probe| {
            // Strip "/api/version" from the probe URL to get the base.
            let parsed = reqwest::Url::parse(&probe.url).ok()?;
            origin_only(parsed.as_str())
        })
        .unwrap_or_else(|| CATALOG_FALLBACK.to_string());

    LocalLlmTarget {
        base_url,
        default_model: None,
    }
}

// ── Ollama model negotiation ──────────────────────────────────────────────────
//
// Pure negotiation logic extracted so it can be unit-tested without any
// network involvement.  The async probe/list helpers in
// `maekon_network::ollama_discovery` call the real Ollama daemon; this
// function consumes their outputs.

/// Outcome of model negotiation reported back to the caller.
///
/// Consumed only inside `create_local_llm_session`, `#[cfg(feature =
/// "analysis")]` — under `--no-default-features` nothing constructs or
/// matches on this, but it stays unconditional (rather than hard-gated) so
/// the pure-logic unit tests below — which cover this type directly and are
/// not otherwise `analysis`-gated — keep running regardless of feature
/// flags (#7743 ctd-W3 A2b follow-up).
#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NegotiationOutcome {
    /// The requested model was found in the installed list (exact or tag-
    /// stripped match).  No change was made.
    ExactMatch,
    /// The requested model was not installed; a model from the same name
    /// family was selected instead.  `used` is the family member chosen.
    FamilyFallback { used: String },
    /// The requested model is not installed and no family member was found.
    /// The original model is preserved with a warning.  `available` carries
    /// the full installed list for log / UI purposes.
    NotInstalled { available: Vec<String> },
    /// The installed-model list was unavailable (`None`).  Negotiation was
    /// skipped; the original model is preserved unchanged.
    ListUnavailable,
    /// The caller supplied `explicit: true`, meaning the user explicitly named
    /// this model in `SessionConfig.model`.  Negotiation is skipped out of
    /// respect for the user's choice; a warn is still emitted when the model
    /// is not installed.
    ExplicitOverride,
}

/// Negotiate the best available Ollama model given the installed list.
///
/// # Arguments
/// * `requested` — the model name resolved through the default-chain (may
///   include a tag, e.g. `"qwen3:8b"`).
/// * `explicit` — `true` when `SessionConfig.model` was `Some` (user
///   explicitly named the model).  Negotiation is skipped.
/// * `installed` — the list returned by `list_installed_models`, or `None`
///   when the API was unreachable.
///
/// # Returns
/// A `(String, NegotiationOutcome)` pair.  The `String` is the model name to
/// use for the session; it equals `requested` unless `FamilyFallback` fired.
///
/// Family matching: the *base name* is the part before the first `:`.  A
/// family member is any installed model whose base name equals the requested
/// base name.  The first matching family member (sorted by the OS order of
/// `/api/tags`) is selected — no arbitrary cross-family selection is made to
/// prevent accidentally using an embedding or code model for chat.
///
/// See `NegotiationOutcome`'s doc comment: consumed only from the
/// `analysis`-gated `create_local_llm_session`, but kept unconditional so
/// its unit tests below run regardless of feature flags.
#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
pub(crate) fn negotiate_local_llm_model(
    requested: &str,
    explicit: bool,
    installed: Option<&[String]>,
) -> (String, NegotiationOutcome) {
    // User explicitly named the model — respect it, no negotiation.
    if explicit {
        return (requested.to_string(), NegotiationOutcome::ExplicitOverride);
    }

    let Some(installed) = installed else {
        return (requested.to_string(), NegotiationOutcome::ListUnavailable);
    };

    // Exact match: the requested model is installed as-is.
    if installed.iter().any(|m| m == requested) {
        return (requested.to_string(), NegotiationOutcome::ExactMatch);
    }

    // Tag-stripped match: "qwen3" matches "qwen3:latest", "qwen3:8b", etc.
    // and "qwen3:8b" (requested) should match "qwen3:8b" (already covered
    // above) OR "qwen3" without tag.
    let requested_base = requested.split(':').next().unwrap_or(requested);
    if installed.iter().any(|m| m.as_str() == requested_base) {
        return (requested.to_string(), NegotiationOutcome::ExactMatch);
    }

    // Family fallback: find any installed model with the same base name.
    if let Some(family_member) = installed
        .iter()
        .find(|m| m.split(':').next().unwrap_or(m.as_str()) == requested_base)
    {
        return (
            family_member.clone(),
            NegotiationOutcome::FamilyFallback {
                used: family_member.clone(),
            },
        );
    }

    // Not installed and no family member found.
    (
        requested.to_string(),
        NegotiationOutcome::NotInstalled {
            available: installed.to_vec(),
        },
    )
}

/// Which conversation adapter handles a subprocess surface (B3 SSOT — E21
/// #4864): keyed by catalog invocation mode rather than a surface_id string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SubprocessAdapterKind {
    Claude,
    Generic,
    /// Codex `app-server` JSON-RPC session (#4866) — long-lived process + thread.
    AppServer,
}

/// `ClaudePrintJson` uses the Claude stream-json adapter; every other (or
/// unresolved) mode uses the generic adapter, which dispatches by mode itself
/// and errors for modes without a session implementation.
pub(super) fn subprocess_adapter_kind(
    mode: Option<provider_specs::SubprocessInvocationMode>,
) -> SubprocessAdapterKind {
    match mode {
        Some(provider_specs::SubprocessInvocationMode::ClaudePrintJson) => {
            SubprocessAdapterKind::Claude
        }
        Some(provider_specs::SubprocessInvocationMode::CodexAppServer) => {
            SubprocessAdapterKind::AppServer
        }
        _ => SubprocessAdapterKind::Generic,
    }
}

/// Whether the Codex `app-server` transport should be attempted for this rollout
/// stage (E21 #4871). `Off` → never (use `codex exec`); opt-in/default → attempt
/// app-server (with graceful fallback to exec handled by the caller).
pub(super) fn codex_should_attempt_app_server(rollout: CodexAppServerRollout) -> bool {
    !matches!(rollout, CodexAppServerRollout::Off)
}

/// Whether a catalog surface is the Codex `codex exec` one-shot path — the
/// fallback target for the app-server transport (E21 #4871).
pub(super) fn is_codex_exec_surface(surface_id: &str) -> bool {
    matches!(
        provider_specs::subprocess_invocation_mode(surface_id).ok(),
        Some(provider_specs::SubprocessInvocationMode::CodexExecJson)
    )
}

/// Find the detected `codex exec` surface among probed CLIs — the graceful
/// fallback for the app-server transport (E21 #4871). Both codex surfaces map
/// to the same `codex` binary, so when the app-server surface is detected its
/// exec sibling normally is too.
fn find_codex_exec_surface(probed: &[ProbedSubprocessCli]) -> Option<DetectedSubprocessCli> {
    probed
        .iter()
        .find(|p| is_codex_exec_surface(&p.detected.surface_id))
        .map(|p| p.detected.clone())
}

impl SessionManagerImpl {
    /// Create a session by dispatching on transport type.
    /// Called from the `SessionManager::create_session` trait impl.
    pub(super) async fn create_session_impl(
        &self,
        mut config: SessionConfig,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        // Auto-generate system prompt from context if not provided and lift any
        // context-assembler tool definitions into session defaults when enabled.
        let mut default_tools: DefaultTools = None;
        if let Some(ref assembler) = self.context_assembler {
            if config.system_prompt.is_none() || config.tools_enabled {
                let message = assembler.build_system_message().await;
                if config.system_prompt.is_none() {
                    config.system_prompt = Some(message.content);
                }
                if config.tools_enabled {
                    default_tools = message.tools.filter(|tools| !tools.is_empty());
                }
            }
        }

        match config.transport {
            SessionTransport::Subprocess => {
                self.create_subprocess_session(&config, &default_tools)
                    .await
            }
            SessionTransport::HttpApi => {
                self.create_http_api_session(&config, &default_tools).await
            }
            SessionTransport::LocalLlm => {
                self.create_local_llm_session(&config, &default_tools).await
            }
        }
    }

    /// Wrap a freshly-created inner session with the privacy guard and the
    /// audit decorator. The guard self-gates on `inner.is_external()`, so local
    /// sessions pass through unsanitized; external sessions get their content
    /// sanitized before transmission. Order: `AuditingSession(Guarded(inner))`
    /// — audit outermost, guard closest to the transport (E21 #4882/#4883).
    ///
    /// Fail-closed invariant (E21 #4869): an external session MUST NOT be
    /// created without a privacy guard. If `privacy_guard` is absent, a local
    /// session still passes through (its data never leaves the device), but an
    /// external one is refused rather than egressing chat content unsanitized.
    pub(super) fn decorate_session(
        &self,
        inner: Arc<dyn ConversationSession>,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let guarded: Arc<dyn ConversationSession> = match self.privacy_guard.clone() {
            Some(guard) => Arc::new(GuardedConversationSession::new(inner, guard)),
            None if inner.is_external() => {
                return Err(CoreError::PolicyDenied {
                    code: maekon_core::error_codes::PolicyCode::Denied,
                    message: format!(
                        "external session '{}' refused: privacy guard not configured (#4869 fail-closed)",
                        inner.provider_name()
                    ),
                });
            }
            None => {
                crate::provider_adapters::ensure_non_external_endpoints_are_loopback(
                    inner.provider_name(),
                    inner.egress_endpoint_urls(),
                )?;
                inner
            }
        };
        Ok(Arc::new(AuditingSession::new(guarded, self.audit.clone())))
    }

    async fn create_subprocess_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        // F-RR-C24-02: `probe_known_cli_surfaces` is a synchronous blocking
        // function — it spawns child processes and contains a
        // `thread::sleep(50ms)` polling loop inside
        // `run_probe_command_with_timeout`.  Calling it bare in an async fn
        // stalls the Tokio executor thread for the duration of all CLI probes.
        // Move work onto a blocking thread, matching the pattern in
        // `feature_capabilities.rs:72`.
        let probed_surfaces = tokio::task::spawn_blocking(probe_known_cli_surfaces)
            .await
            .map_err(|join_err| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("probe_known_cli_surfaces panicked: {join_err}"),
            })?;
        let surface = if let Some(requested_surface_id) = config.surface_id.as_deref() {
            probed_surfaces
                .iter()
                .find(|surface| {
                    surface
                        .detected
                        .surface_id
                        .eq_ignore_ascii_case(requested_surface_id)
                })
                .map(|surface| surface.detected.clone())
                .ok_or_else(|| {
                    // Iter-94: NotFound semantically (the requested subprocess
                    // CLI tool is not installed / not detected); wire code
                    // `not_found.resource_missing` helps telemetry distinguish
                    // missing-tool from internal runtime failure.
                    CoreError::NotFound {
                        code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                        resource_type: "subprocess_cli_surface".to_string(),
                        id: requested_surface_id.to_string(),
                    }
                })?
        } else {
            probed_surfaces
                .iter()
                .find(|surface| {
                    runtime_ready_for_surface(&surface.detected.surface_id, surface.auth_status)
                })
                .map(|surface| surface.detected.clone())
                .or_else(|| {
                    probed_surfaces
                        .first()
                        .map(|surface| surface.detected.clone())
                })
                .ok_or_else(|| {
                    // Iter-94: no subprocess CLI detected — surface-level
                    // NotFound. Distinguishes "user hasn't installed any
                    // supported CLI" from generic runtime failure in logs.
                    CoreError::NotFound {
                        code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                        resource_type: "subprocess_cli_surface".to_string(),
                        id: "any".to_string(),
                    }
                })?
        };

        // B3 SSOT: choose the conversation adapter by catalog invocation mode,
        // not by a hard-coded surface_id string. `ClaudePrintJson` → the Claude
        // stream-json adapter; every other mode → the generic adapter (which
        // itself dispatches by mode and errors for unsupported ones).
        let mode = provider_specs::subprocess_invocation_mode(&surface.surface_id).ok();
        let inner: Arc<dyn ConversationSession> = match subprocess_adapter_kind(mode) {
            SubprocessAdapterKind::Claude => Arc::new(ClaudeSubprocessSession::new(
                surface,
                config,
                self.config.clone(),
                default_tools.clone(),
            )),
            SubprocessAdapterKind::Generic => Arc::new(GenericSubprocessSession::new(
                surface,
                config,
                self.config.clone(),
                default_tools.clone(),
            )),
            SubprocessAdapterKind::AppServer => {
                // E21 #4871: gate app-server behind the rollout flag and
                // gracefully fall back to `codex exec` on any failure.
                self.create_codex_session_with_fallback(
                    &surface,
                    &probed_surfaces,
                    config,
                    default_tools,
                )
                .await?
            }
        };

        let wrapped: Arc<dyn ConversationSession> = self.decorate_session(inner)?;

        let session_id = wrapped.session_id().to_string();
        info!(session_id = %session_id, "created subprocess session");

        let managed = ManagedSession {
            session: wrapped.clone(),
            state: SessionState::Active,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        self.admit_session(session_id, managed).await?;

        Ok(wrapped)
    }

    /// Create a Codex conversation session honoring the app-server rollout flag
    /// with graceful fallback to `codex exec` (E21 #4871). The returned inner
    /// session is still wrapped by `decorate_session` (so the privacy guard
    /// applies on every path, including the exec fallback — review B1).
    /// `pub(super)` so the E21 #4863 wired test can assert the full graceful
    /// degrade chain (probe-fail → exec fallback) end to end.
    pub(super) async fn create_codex_session_with_fallback(
        &self,
        app_server_surface: &DetectedSubprocessCli,
        probed: &[ProbedSubprocessCli],
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        // Flag off → skip app-server entirely, use the exec sibling surface.
        if !codex_should_attempt_app_server(self.codex_app_server_rollout) {
            return self.build_codex_exec_session(probed, config, default_tools, "rollout_off");
        }

        // #5032: the app-server JSON-RPC transport requires `maekon-network`
        // (`analysis`). When it is not compiled in, degrade to the always-
        // available `codex exec` subprocess path — the SAME graceful fallback the
        // `analysis` build takes on any app-server failure (#4871), so the user's
        // chat behaviour is unchanged.
        #[cfg(not(feature = "analysis"))]
        {
            let _ = app_server_surface;
            return self.build_codex_exec_session(
                probed,
                config,
                default_tools,
                "app_server_unavailable",
            );
        }

        // Attempt app-server; on any failure (handshake / unsupported version /
        // crash / timeout) degrade to exec rather than breaking the user's chat.
        #[cfg(feature = "analysis")]
        match self
            .connect_codex_app_server(app_server_surface, config)
            .await
        {
            Ok(session) => Ok(session),
            Err(err) => {
                warn!(
                    err.code = %err.code(),
                    "codex app-server transport failed; falling back to codex exec (#4871): {err}"
                );
                self.build_codex_exec_session(probed, config, default_tools, "app_server_failed")
            }
        }
    }

    /// Connect the `codex app-server` JSON-RPC transport and open a thread
    /// (#4866). Args come from the catalog transport spec; R6 sandbox clamping
    /// is enforced inside the adapter's `effective_sandbox`. `pub(super)` so the
    /// E21 #4863 wired tests can drive this exact spawn chokepoint (binary trust,
    /// the `--version` probe, and version negotiation) through a fake `codex`
    /// binary — proving the checks run on the real path, not in isolation
    /// (dead-writer ban).
    #[cfg(feature = "analysis")]
    pub(super) async fn connect_codex_app_server(
        &self,
        surface: &DetectedSubprocessCli,
        config: &SessionConfig,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let transport =
            provider_specs::subprocess_transport(&surface.surface_id).map_err(|msg| {
                CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Invalid,
                    message: format!("app-server transport spec unavailable: {msg}"),
                }
            })?;

        // ── E21 #4863 R7: binary-trust verification BEFORE spawning ──────────
        // (a) install-path allowlist — WARN-AND-PROCEED telemetry (NOT a gate);
        // (b) `codex --version` probe — the ONLY hard branch (a binary that
        //     cannot answer `--version` is not a trustworthy app-server, so we
        //     return Err here and the #4871 arm degrades to `codex exec`).
        // Both branches keep the graceful tolerate-and-fallback invariant: the
        // only `Err` routes through `create_codex_session_with_fallback`, never
        // an abort of the user's chat.
        let trust =
            trust::install_path_trust(&surface.executable_path, &trust::default_allowed_roots());
        // Source the probe args from the catalog compatibility spec; default to
        // `["--version"]` when the spec is absent (graceful, never an error).
        let probe_args: Vec<String> = provider_specs::subprocess_compatibility(&surface.surface_id)
            .ok()
            .map(|compat| compat.version_probe_command.clone())
            .filter(|args| !args.is_empty())
            .unwrap_or_else(|| vec!["--version".to_string()]);
        let probed_version = Self::probe_codex_version(&surface.executable_path, &probe_args).await;
        // Log the file name only — the full install path embeds the OS username
        // (e.g. /Users/<user>/.codex/...), a PII leak in a monitoring product.
        let exe_name = surface
            .executable_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("codex");
        match &probed_version {
            Some(version) => {
                if matches!(trust, trust::BinaryTrust::TrustedPath) {
                    info!(
                        codex_version = %version,
                        exe = %exe_name,
                        "codex binary trusted (#4863 R7)"
                    );
                } else {
                    // Out-of-allowlist binary that still answers --version. Default
                    // mode is WARN-AND-PROCEED — this is audit telemetry (e.g. a
                    // PATH-hijacked codex is visible), NOT a hard block. Hard-block
                    // is a deferred opt-in managed mode.
                    warn!(
                        codex_version = %version,
                        exe = %exe_name,
                        "codex binary outside install allowlist; proceeding (#4863 R7 telemetry)"
                    );
                }
            }
            None => {
                // A binary that cannot answer `--version` (spawn error / timeout /
                // no version token) is not a trustworthy app-server. This is the
                // ONLY hard branch: return Err → create_codex_session_with_fallback
                // degrades to `codex exec` (#4871) — the chat is never aborted.
                return Err(CoreError::InvalidArguments {
                    code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                    message: format!(
                        "codex_version_unverified: '{}' did not answer {:?}",
                        exe_name, probe_args
                    ),
                });
            }
        }

        let mut command = tokio::process::Command::new(&surface.executable_path);
        command.args(&transport.app_server_args);
        let client_info = ClientInfo {
            name: "maekon".to_string(),
            title: "Maekon".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        // I3 (#4866): pass conversation-thread params through to `thread/start`.
        let thread = ThreadConfig {
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "gpt-5.4".to_string()),
            cwd: config.cwd.clone(),
            sandbox_policy: config.sandbox_policy.clone(),
            approval_policy: config.approval_policy.clone(),
        };
        // E21 #4870: wire the FAIL-CLOSED approval loop when a decider is present
        // (policy auto-decision + audit + DefaultDenyUiHook). `None` keeps the
        // plain #4866 behavior (inbound approval requests drained-and-dropped).
        let session = CodexAppServerSession::connect_with_approval(
            command,
            &client_info,
            thread,
            self.codex_approval_decider.clone(),
        )
        .await?;

        // ── E21 #4863 version negotiation (INFORM-ONLY, post-connect) ────────
        // The `initialize` result carries NO protocolVersion — only a userAgent
        // (I1 correction). Extract its version token and cross-check it against
        // the catalog's prose minimum + known-bad denylist, and against the
        // pre-spawn `--version` probe output. EVERY branch is warn!/info! +
        // PROCEED: there is NO `return Err` on any version comparison, so version
        // drift can NEVER break a working session (ADR-025 / #4871). Numeric
        // semver gating is impossible against the prose `minimum_supported_version`
        // and is deferred to a follow-up that adds a comparable catalog field.
        Self::log_version_negotiation(&surface.surface_id, &session, probed_version.as_deref());

        Ok(Arc::new(session))
    }

    /// Spawn the catalog `version_probe_command` (e.g. `codex --version`) and
    /// leniently extract a version token from its stdout (E21 #4863 R7 + the
    /// `codex --version` parallel check — a SINGLE wired reader shared by the
    /// trust gate and the version log). Bounded by a 4s timeout. Returns `None`
    /// on spawn error, timeout, non-zero exit with no parseable token, or absent
    /// version token — the caller treats `None` as "unprobeable real-codex" and
    /// degrades to `codex exec` (the only hard branch).
    #[cfg(feature = "analysis")]
    async fn probe_codex_version(exe: &std::path::Path, probe_args: &[String]) -> Option<String> {
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            tokio::process::Command::new(exe)
                .args(probe_args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output(),
        )
        .await
        .ok()? // timeout → None
        .ok()?; // spawn/io error → None
                // Many CLIs print the version to stdout; some to stderr. We suppressed
                // stderr above to keep the probe quiet, so read stdout. Combine lines
                // and reuse the same lenient extractor as the userAgent path.
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_user_agent_version(&stdout)
    }

    /// Emit the INFORM-ONLY version-negotiation log for a freshly-connected
    /// app-server session (E21 #4863). Reads the live `userAgent` via the session
    /// passthrough accessor, extracts its version, and cross-checks it against the
    /// catalog denylist + the pre-spawn `--version` probe. NEVER gates — all
    /// branches warn!/info! + proceed.
    #[cfg(feature = "analysis")]
    fn log_version_negotiation(
        surface_id: &str,
        session: &CodexAppServerSession,
        probed_version: Option<&str>,
    ) {
        let user_agent = session.server_info().user_agent.clone();
        let ua_version = user_agent.as_deref().and_then(parse_user_agent_version);

        // Cross-check the live userAgent version against the catalog known-bad
        // denylist (prose substrings) → warn only.
        if let (Some(ua_ver), Ok(compat)) = (
            ua_version.as_deref(),
            provider_specs::subprocess_compatibility(surface_id),
        ) {
            for bad in &compat.known_bad_versions {
                if !bad.is_empty() && ua_ver.contains(bad.as_str()) {
                    warn!(
                        useragent_version = %ua_ver,
                        known_bad = %bad,
                        "codex app-server reports a known-bad version; proceeding (#4863 inform-only)"
                    );
                }
            }
        }

        // userAgent (live, post-connect) vs --version (pre-spawn binary) → warn on
        // mismatch, proceed regardless.
        match (ua_version.as_deref(), probed_version) {
            (Some(ua_ver), Some(probe_ver)) if ua_ver != probe_ver => {
                warn!(
                    useragent_version = %ua_ver,
                    probe_version = %probe_ver,
                    "codex userAgent vs --version mismatch; proceeding (#4863 inform-only)"
                );
            }
            (Some(ua_ver), _) => {
                info!(
                    useragent_version = %ua_ver,
                    probe_version = probed_version.unwrap_or("unknown"),
                    "codex app-server version negotiated (#4863 inform-only)"
                );
            }
            (None, _) => {
                // No parseable userAgent version — the prose minimum can't be
                // numerically compared anyway. Log the raw userAgent for triage.
                info!(
                    user_agent = user_agent.as_deref().unwrap_or("<absent>"),
                    "codex app-server userAgent carries no parseable version token (#4863 inform-only)"
                );
            }
        }
    }

    /// Build the `codex exec` one-shot session on the exec sibling surface — the
    /// graceful fallback / flag-off path (E21 #4871).
    pub(super) fn build_codex_exec_session(
        &self,
        probed: &[ProbedSubprocessCli],
        config: &SessionConfig,
        default_tools: &DefaultTools,
        reason: &str,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let exec_surface = find_codex_exec_surface(probed).ok_or_else(|| CoreError::NotFound {
            code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "codex_exec_surface".to_string(),
            id: "provider_surface.openai.subprocess_cli".to_string(),
        })?;
        info!(
            reason,
            surface = %exec_surface.surface_id,
            "using codex exec path (#4871)"
        );
        Ok(Arc::new(GenericSubprocessSession::new(
            exec_surface,
            config,
            self.config.clone(),
            default_tools.clone(),
        )))
    }

    #[cfg(feature = "analysis")]
    async fn create_http_api_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let surface_id =
            config
                .surface_id
                .as_deref()
                .ok_or_else(|| CoreError::InvalidArguments {
                    code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                    message: "surface_id is required for HttpApi sessions".to_string(),
                })?;

        // Resolve surface spec from the provider catalog.
        // Iter-94: catalog lookup miss = NotFound (the surface id references
        // an entry that doesn't exist in the catalog); not an internal error.
        let surface_spec = provider_specs::provider_surface_spec(surface_id).map_err(|msg| {
            CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "provider_surface".to_string(),
                id: format!("{surface_id}: {msg}"),
            }
        })?;
        // Iter-94: unknown provider_type = invalid config (the vendor_id
        // doesn't map to any known provider type).
        let provider_type = maekon_core::provider_surface::provider_type_from_vendor_id(
            &surface_spec.provider_type,
        )
        .ok_or_else(|| CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: format!(
                "unknown provider_type for vendor '{}'",
                surface_spec.vendor_id
            ),
        })?;

        // Resolve the LLM transport endpoint from the catalog.
        // Iter-94: transport catalog miss = NotFound.
        let transport_spec = provider_specs::resolved_transport_spec(
            provider_type,
            Some(surface_id),
            ProviderTransportKind::Llm,
        )
        .map_err(|msg| CoreError::NotFound {
            code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "provider_transport".to_string(),
            id: format!("{surface_id}/llm: {msg}"),
        })?;

        // Model: explicit > catalog default > error.
        let model = config
            .model
            .clone()
            .or_else(|| {
                provider_specs::resolved_default_model(
                    provider_type,
                    Some(surface_id),
                    SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .ok_or_else(|| CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: format!(
                    "no model specified and surface '{surface_id}' has no default LLM model"
                ),
            })?;

        // Build credential source from the secret store.
        let credential = CredentialSource::from_api_key_endpoint(
            &maekon_core::config::ExternalApiEndpoint {
                endpoint: transport_spec.url.clone(),
                api_key: String::new(),
                model: Some(model.clone()),
                timeout_secs: 30,
                provider_type,
                surface_id: Some(surface_id.to_string()),
                credential: None,
            },
            self.secret_store.clone(),
        )
        .or_else(|_| {
            // Fallback: no-auth surfaces (e.g. Ollama local_http).
            if maekon_core::provider_surface::provider_surface_uses_no_auth(surface_id) {
                Ok(CredentialSource::NoAuth)
            } else {
                Err(CoreError::Auth {
                    code: maekon_core::error_codes::AuthCode::Failed,
                    message: format!("no credential available for surface '{surface_id}'"),
                })
            }
        })?;

        let inner: Arc<dyn ConversationSession> =
            Arc::new(HttpApiSession::new(HttpApiSessionInit {
                surface_id: surface_id.to_string(),
                model,
                endpoint: transport_spec.url.clone(),
                credential,
                provider_type,
                system_prompt: config.system_prompt.clone(),
                config: self.config.clone(),
                default_tools: default_tools.clone(),
                // D7 (#4812 / E20-20): shared workspace-wide registry from the
                // composition root (iter-011 consolidation done) — same Arc as the
                // other network adapters targeting the same endpoint.
                breaker_registry: self.breaker_registry.clone(),
            }));

        let wrapped: Arc<dyn ConversationSession> = self.decorate_session(inner)?;

        let session_id = wrapped.session_id().to_string();
        info!(session_id = %session_id, surface_id = %surface_id, "created HttpApi session");

        let managed = ManagedSession {
            session: wrapped.clone(),
            state: SessionState::Active,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        self.admit_session(session_id, managed).await?;

        Ok(wrapped)
    }

    #[cfg(feature = "analysis")]
    async fn create_local_llm_session(
        &self,
        config: &SessionConfig,
        default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let _ = default_tools; // LocalLlm does not use tool definitions yet.

        // Resolve Ollama base URL from the pre-computed target (wired at
        // composition time by session_wiring.rs via `with_local_llm_target`).
        // Falls back to the catalog-derived default when no target was wired
        // (e.g. standalone tests or no composition root), which is still loopback.
        let target = self.local_llm_target.clone().unwrap_or_else(|| {
            resolve_local_llm_target(
                // Build a minimal default so we always get a loopback base.
                &maekon_core::config::AiProviderConfig::default(),
            )
        });
        let base_url = target.base_url;

        // Whether the user explicitly named a model in SessionConfig.
        let explicit_model = config.model.is_some();

        // Model precedence: SessionConfig.model > llm_api.model (target) >
        // catalog resolved_default_model(Ollama) > "qwen3:8b" literal fallback.
        let resolved_model = config
            .model
            .clone()
            .or(target.default_model)
            .or_else(|| {
                provider_specs::resolved_default_model(
                    maekon_core::config::AiProviderType::Ollama,
                    None,
                    maekon_api_contracts::provider_specs::SurfaceCapabilityKind::Llm,
                )
                .ok()
                .flatten()
            })
            .unwrap_or_else(|| "qwen3:8b".to_string());

        // ── Step 1: Ollama identity probe (C3 #5732 FLAG-2) ──────────────────
        //
        // GET {base}/api/version to confirm the endpoint is an Ollama daemon.
        // Bounded to 2 s; fail-open — session creation proceeds on every result.
        // One-time cost per session creation call.
        let probe_timeout = std::time::Duration::from_secs(2);
        let probe = maekon_network::ollama_discovery::probe_ollama(&base_url, probe_timeout).await;
        match &probe {
            maekon_network::ollama_discovery::OllamaProbe::Ollama { version } => {
                info!(
                    ollama_version = %version,
                    base_url = %base_url,
                    "Ollama daemon confirmed"
                );
            }
            maekon_network::ollama_discovery::OllamaProbe::NotOllama => {
                warn!(
                    base_url = %base_url,
                    "endpoint responds but does not look like Ollama — native /api/chat will \
                     likely fail; LM Studio/vLLM users should use the HTTP API chat path instead \
                     (SessionTransport::HttpApi with an OpenAI-compatible surface)"
                );
            }
            maekon_network::ollama_discovery::OllamaProbe::Unreachable => {
                warn!(
                    base_url = %base_url,
                    "Ollama daemon appears unreachable — is `ollama serve` running? \
                     Session will be created; errors will surface on first message."
                );
            }
        }

        // ── Step 2: model negotiation (C2 #5727) ─────────────────────────────
        //
        // Only when the probe confirmed Ollama: fetch the installed model list
        // and negotiate the best available model within the same family.
        // Skipped for non-Ollama / unreachable endpoints (fail-open).
        let final_model = if matches!(
            probe,
            maekon_network::ollama_discovery::OllamaProbe::Ollama { .. }
        ) {
            let installed =
                maekon_network::ollama_discovery::list_installed_models(&base_url, probe_timeout)
                    .await;
            let (negotiated, outcome) =
                negotiate_local_llm_model(&resolved_model, explicit_model, installed.as_deref());
            match &outcome {
                NegotiationOutcome::ExactMatch => {
                    info!(model = %negotiated, "Ollama model verified installed");
                }
                NegotiationOutcome::FamilyFallback { used } => {
                    info!(
                        requested = %resolved_model,
                        selected = %used,
                        "requested Ollama model not installed; using installed family member"
                    );
                }
                NegotiationOutcome::NotInstalled { available } => {
                    warn!(
                        model = %negotiated,
                        available = ?available,
                        "model is not installed — run `ollama pull {}`; \
                         session will fail on first message",
                        negotiated
                    );
                }
                NegotiationOutcome::ListUnavailable => {
                    info!(
                        model = %negotiated,
                        "could not fetch installed model list; proceeding with resolved model"
                    );
                }
                NegotiationOutcome::ExplicitOverride => {
                    // User explicitly chose; log only when not installed.
                    // The NotInstalled arm below handles that.
                }
            }
            negotiated
        } else {
            resolved_model
        };

        let session_id = uuid::Uuid::new_v4().to_string();

        let inner: Arc<dyn ConversationSession> = Arc::new(LocalLlmSession::new(
            session_id.clone(),
            final_model,
            base_url,
            config.system_prompt.clone(),
            self.config.clone(),
        ));

        let wrapped: Arc<dyn ConversationSession> = self.decorate_session(inner)?;

        let session_id = wrapped.session_id().to_string();
        info!(session_id = %session_id, "created LocalLlm (Ollama) session");

        let managed = ManagedSession {
            session: wrapped.clone(),
            state: SessionState::Active,
            created_at: Instant::now(),
            last_active: Instant::now(),
            retry_count: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
        };
        self.admit_session(session_id, managed).await?;

        Ok(wrapped)
    }

    // #5032: `not(analysis)` fallbacks for the two network-only conversation
    // transports. The `HttpApi` / `LocalLlm` sessions are implemented entirely in
    // `maekon-network` (only compiled under `analysis`), so when that feature is
    // off these transports cannot be served by this build. They return
    // `ServiceUnavailable` — the same disabled-feature shape used elsewhere
    // (e.g. `ocr_resolver`, `sync_setup`) — so `create_session_impl` still
    // compiles and dispatches under every feature-matrix cell. No behaviour
    // change when `analysis` is enabled (the real builders above are used).
    #[cfg(not(feature = "analysis"))]
    async fn create_http_api_session(
        &self,
        _config: &SessionConfig,
        _default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "HttpApi conversation sessions require the 'analysis' feature \
                      (maekon-network), which is not compiled in this build"
                .to_string(),
        })
    }

    #[cfg(not(feature = "analysis"))]
    async fn create_local_llm_session(
        &self,
        _config: &SessionConfig,
        _default_tools: &DefaultTools,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "LocalLlm conversation sessions require the 'analysis' feature \
                      (maekon-network), which is not compiled in this build"
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        negotiate_local_llm_model, subprocess_adapter_kind, NegotiationOutcome,
        SubprocessAdapterKind,
    };
    use maekon_api_contracts::provider_specs::SubprocessInvocationMode as Mode;

    // ── negotiate_local_llm_model ─────────────────────────────────────────────

    #[test]
    fn negotiate_exact_match_returns_exact_match() {
        let installed = vec!["qwen3:8b".to_string(), "llama3:latest".to_string()];
        let (model, outcome) = negotiate_local_llm_model("qwen3:8b", false, Some(&installed));
        assert_eq!(model, "qwen3:8b");
        assert_eq!(outcome, NegotiationOutcome::ExactMatch);
    }

    #[test]
    fn negotiate_tag_stripped_match_returns_exact_match() {
        // "qwen3" (no tag) installed; requested is "qwen3" — base-name match.
        let installed = vec!["qwen3".to_string()];
        let (model, outcome) = negotiate_local_llm_model("qwen3", false, Some(&installed));
        assert_eq!(model, "qwen3");
        assert_eq!(outcome, NegotiationOutcome::ExactMatch);
    }

    #[test]
    fn negotiate_family_fallback_selects_installed_family_member() {
        // qwen3:8b not installed; qwen3:4b is — same family.
        let installed = vec!["llama3:latest".to_string(), "qwen3:4b".to_string()];
        let (model, outcome) = negotiate_local_llm_model("qwen3:8b", false, Some(&installed));
        assert_eq!(model, "qwen3:4b");
        assert_eq!(
            outcome,
            NegotiationOutcome::FamilyFallback {
                used: "qwen3:4b".to_string()
            }
        );
    }

    #[test]
    fn negotiate_not_installed_preserves_original_when_no_family() {
        let installed = vec!["llama3:latest".to_string(), "mistral:7b".to_string()];
        let (model, outcome) = negotiate_local_llm_model("qwen3:8b", false, Some(&installed));
        assert_eq!(model, "qwen3:8b");
        assert_eq!(
            outcome,
            NegotiationOutcome::NotInstalled {
                available: installed.clone()
            }
        );
    }

    #[test]
    fn negotiate_explicit_override_skips_negotiation() {
        // Even if qwen3:8b is not installed, explicit=true → no change.
        let installed = vec!["llama3:latest".to_string()];
        let (model, outcome) = negotiate_local_llm_model("qwen3:8b", true, Some(&installed));
        assert_eq!(model, "qwen3:8b");
        assert_eq!(outcome, NegotiationOutcome::ExplicitOverride);
    }

    #[test]
    fn negotiate_list_unavailable_skips_negotiation() {
        let (model, outcome) = negotiate_local_llm_model("qwen3:8b", false, None);
        assert_eq!(model, "qwen3:8b");
        assert_eq!(outcome, NegotiationOutcome::ListUnavailable);
    }

    #[test]
    fn negotiate_empty_installed_list_returns_not_installed() {
        let (model, outcome) = negotiate_local_llm_model("qwen3:8b", false, Some(&[]));
        assert_eq!(model, "qwen3:8b");
        assert_eq!(
            outcome,
            NegotiationOutcome::NotInstalled { available: vec![] }
        );
    }

    // ── subprocess_adapter_kind ───────────────────────────────────────────────

    #[test]
    fn claude_mode_routes_to_claude_adapter() {
        assert_eq!(
            subprocess_adapter_kind(Some(Mode::ClaudePrintJson)),
            SubprocessAdapterKind::Claude
        );
    }

    #[test]
    fn codex_app_server_mode_routes_to_app_server_adapter() {
        assert_eq!(
            subprocess_adapter_kind(Some(Mode::CodexAppServer)),
            SubprocessAdapterKind::AppServer
        );
    }

    #[test]
    fn other_and_unresolved_modes_route_to_generic_adapter() {
        for mode in [
            Mode::CodexExecJson,
            Mode::GeminiCliPrompt,
            Mode::ManualChatGui,
        ] {
            assert_eq!(
                subprocess_adapter_kind(Some(mode)),
                SubprocessAdapterKind::Generic,
                "mode {mode:?} must route to the generic adapter"
            );
        }
        // Unresolved surface (catalog miss) defaults to generic, preserving the
        // prior else-branch behavior.
        assert_eq!(
            subprocess_adapter_kind(None),
            SubprocessAdapterKind::Generic
        );
    }
}
