//! Internal helper functions for suggestion Tauri commands.
//!
//! ADR-013 split from `suggestions/mod.rs`.

use crate::ipc_error::IpcError;

/// Canonical "Suggestions not available" error — suggestion manager missing.
pub(crate) fn suggestions_not_available() -> IpcError {
    IpcError::new("service.unavailable", "Suggestions not available")
}

/// Canonical "AI sessions not available" error — AI session manager missing.
pub(crate) fn ai_sessions_not_available() -> IpcError {
    IpcError::new("service.unavailable", "AI sessions not available")
}

/// Enqueue a failed feedback for background retry and persist to SQLite.
///
/// SQLite persist is the primary durability guarantee — it happens first.
/// The in-memory retry queue is then updated by awaiting the lock directly;
/// since the enqueue is a single fast push (no I/O, no long await), holding
/// the lock synchronously in the caller's async context is safe and eliminates
/// the pre-shutdown cancellation race that existed when a spawned task's
/// `JoinHandle` was discarded.
// pub(crate) so sibling module `feedback` can import this.
pub(crate) async fn enqueue_feedback_retry(
    mgr: &crate::suggestion_manager::SuggestionManager,
    suggestion_id: &str,
    feedback_type: maekon_core::models::suggestion::FeedbackType,
    comment: Option<String>,
) {
    let record = maekon_core::models::storage_records::PendingFeedbackRecord::new_for_insert(
        suggestion_id.to_string(),
        &feedback_type,
        comment.clone(),
        0,
        chrono::Utc::now(),
    );
    if let Err(e) = mgr.storage().save_pending_feedback(&record) {
        tracing::warn!(id = %suggestion_id, "failed to persist pending feedback: {e}");
    }
    // Await the lock directly — the critical section is a single VecDeque push,
    // so the lock is released immediately with no risk of contention.
    let evicted = mgr.retry_queue().lock().await.enqueue(
        maekon_suggestion::feedback_retry::PendingFeedback {
            suggestion_id: suggestion_id.to_string(),
            feedback_type,
            comment,
            attempts: 0,
            next_retry_at: chrono::Utc::now(),
        },
    );
    // If the bounded queue evicted an older pending retry, delete its durable row
    // so SQLite does not keep a pending-retry row the in-session maintenance loop
    // never drains (review4). The lock guard is already released here.
    if let Some(evicted) = evicted {
        if let Err(e) = mgr
            .storage()
            .delete_pending_feedback(&evicted.suggestion_id)
        {
            tracing::warn!(id = %evicted.suggestion_id, "failed to delete evicted pending feedback row: {e}");
        }
    }
}

// pub(crate) so sibling module `feedback` can import this.
pub(crate) fn feedback_type_for_action(
    action: &str,
) -> Result<maekon_core::models::suggestion::FeedbackType, IpcError> {
    use maekon_core::models::suggestion::FeedbackType;
    match action {
        "accept" => Ok(FeedbackType::Accepted),
        "reject" => Ok(FeedbackType::Rejected),
        "defer" => Ok(FeedbackType::Deferred),
        _ => Err(IpcError::new(
            "validation.invalid_arguments",
            format!("Unknown action: {action}. Use accept/reject/defer"),
        )),
    }
}

// pub(crate) so sibling module `feedback` can import this.
pub(crate) fn suggestion_not_found(suggestion_id: &str) -> IpcError {
    IpcError::new(
        "not_found.resource_missing",
        format!("Suggestion not found: {suggestion_id}"),
    )
}

/// Why a context-recovery request could not open a chat session for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverySessionUnavailable {
    /// No chat path is readiness-Ready (no provider set up, or it is blocked).
    ProviderNotReady,
    /// A chat path is held back only by the full-text consent that external
    /// chat requires (#12072).
    ExternalTextConsent,
    /// The ready path failed to start a session.
    ProviderUnavailable,
    /// Only a provider CLI could answer. Recovery keeps tools off and no CLI
    /// session can guarantee that (#12094), so the user has to open a Chat
    /// session, which recovery then reuses (#12712).
    CliNeedsOpenChat,
}

/// How close one chat path is to opening for a context-recovery request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatPathReadiness {
    Ready,
    /// Every requirement passes except the full-text consent.
    NeedsExternalTextConsent,
    NotReady,
}

/// Classifies one chat capability's readiness. A path that would be Ready once
/// the full-text consent is granted says so, so the panel can name that
/// permission instead of sending the user to provider setup.
pub(crate) fn chat_path_readiness(
    item: Option<&maekon_core::ai_readiness::AiCapabilityReadiness>,
) -> ChatPathReadiness {
    use maekon_core::ai_readiness::{evaluate_ai_readiness, AiConsentField, AiReadinessStatus};

    let Some(item) = item else {
        return ChatPathReadiness::NotReady;
    };
    if item.status == AiReadinessStatus::Ready {
        return ChatPathReadiness::Ready;
    }
    let only_full_text_missing = item
        .dimensions
        .consent
        .iter()
        .all(|consent| consent.granted || consent.field == AiConsentField::FullTextExtraction);
    let mut granted = item.dimensions.clone();
    for consent in &mut granted.consent {
        consent.granted = true;
    }
    if only_full_text_missing
        && evaluate_ai_readiness(item.capability_id, granted).status == AiReadinessStatus::Ready
    {
        ChatPathReadiness::NeedsExternalTextConsent
    } else {
        ChatPathReadiness::NotReady
    }
}

/// System prompt of a session that context recovery opens for itself. Fixed,
/// so the session context assembler does not add the wider activity summary a
/// Chat session carries: the request is about the current screen only.
pub(crate) const RECOVERY_SYSTEM_PROMPT: &str =
    "You are Maekon's context recovery assistant. Answer only what the user message asks, in the format it asks for.";

/// Chat path a context-recovery request may open for itself when no chat
/// session is running (#12521). It follows the Chat page's create gate and
/// order (`bestReadyChatTransport` in `features/aiReadiness.ts`) except for the
/// provider CLI: only a readiness-Ready capability qualifies, and an HTTP
/// session needs a named provider surface. Readiness already requires the
/// full-text consent for an HTTP API (#12072), so a path missing only that
/// consent is reported as such. Tools stay off: recovery sends one prompt and
/// reads one answer.
///
/// A provider CLI never opens here. The session factory refuses a CLI session
/// with tools off before spawning it, because no CLI can prove that its
/// inherited MCP servers, plugins and hooks are off (#12094). A ready CLI only
/// changes what the user is told when no other path can answer (#12712).
pub(crate) fn recovery_session_config(
    readiness: impl Fn(maekon_core::ai_readiness::AiCapabilityId) -> ChatPathReadiness,
    llm_api: Option<&maekon_core::config::ExternalApiEndpoint>,
) -> Result<maekon_core::models::ai_session::SessionConfig, RecoverySessionUnavailable> {
    use maekon_core::ai_readiness::AiCapabilityId;
    use maekon_core::models::ai_session::{SessionConfig, SessionTransport};

    let named_http = llm_api.filter(|endpoint| endpoint.surface_id.is_some());
    let cli = readiness(AiCapabilityId::ChatSubprocess);
    let paths = [
        (AiCapabilityId::ChatHttpApi, SessionTransport::HttpApi),
        (AiCapabilityId::ChatLocalLlm, SessionTransport::LocalLlm),
    ]
    .map(|(capability, transport)| {
        if transport == SessionTransport::HttpApi && named_http.is_none() {
            (transport, ChatPathReadiness::NotReady)
        } else {
            (transport, readiness(capability))
        }
    });
    let Some(&(transport, _)) = paths
        .iter()
        .find(|(_, state)| *state == ChatPathReadiness::Ready)
    else {
        let consent_only = paths
            .iter()
            .any(|(_, state)| *state == ChatPathReadiness::NeedsExternalTextConsent);
        return Err(if consent_only {
            RecoverySessionUnavailable::ExternalTextConsent
        } else if cli != ChatPathReadiness::NotReady {
            // Granting the consent would not help either: the CLI still
            // cannot open here, and a Chat session asks for it itself.
            RecoverySessionUnavailable::CliNeedsOpenChat
        } else {
            RecoverySessionUnavailable::ProviderNotReady
        });
    };
    let http = named_http.filter(|_| transport == SessionTransport::HttpApi);
    Ok(SessionConfig {
        transport,
        surface_id: http.and_then(|endpoint| endpoint.surface_id.clone()),
        model: http.and_then(|endpoint| endpoint.model.clone()),
        system_prompt: Some(RECOVERY_SYSTEM_PROMPT.to_string()),
        tools_enabled: false,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    })
}

/// Closes a session that context recovery opened for itself once the request
/// ends, whichever return path it takes (#12521). The session is internal, so
/// it never appears in the Chat list, and it answers one prompt, so it does
/// not wait for the reaper.
pub(crate) struct RecoverySessionGuard {
    manager: std::sync::Arc<crate::session_manager::SessionManagerImpl>,
    session_id: String,
}

impl Drop for RecoverySessionGuard {
    fn drop(&mut self) {
        let manager = std::sync::Arc::clone(&self.manager);
        let session_id = std::mem::take(&mut self.session_id);
        tauri::async_runtime::spawn(async move {
            if let Err(error) = manager
                .kill_session_with_reason(&session_id, "context recovery finished")
                .await
            {
                tracing::warn!(%error, "failed to close the context recovery session");
            }
        });
    }
}

/// Opens a chat session for one explicit context-recovery request (#12521).
pub(crate) async fn open_recovery_session(
    manager: &std::sync::Arc<crate::session_manager::SessionManagerImpl>,
    feature_state: tauri::State<'_, crate::feature_capabilities::FeatureCapabilityState>,
    app_state: tauri::State<'_, crate::runtime_state::AppState>,
    config_state: tauri::State<'_, crate::runtime_state::ConfigRuntimeState>,
) -> Result<
    (
        std::sync::Arc<dyn maekon_core::ports::conversation_session::ConversationSession>,
        RecoverySessionGuard,
    ),
    RecoverySessionUnavailable,
> {
    let config = config_state.config_manager().get();
    let readiness = crate::commands::system::feature_capability_snapshot(
        &feature_state,
        &app_state,
        &config_state,
        Some(std::sync::Arc::clone(manager)),
    )
    .await
    .ai_readiness
    .ok_or(RecoverySessionUnavailable::ProviderNotReady)?;
    let session_config = recovery_session_config(
        |capability| chat_path_readiness(readiness.find(capability)),
        config.ai_provider.llm_api.as_ref(),
    )?;
    let session = manager
        .create_internal_session(session_config)
        .await
        .map_err(|_| RecoverySessionUnavailable::ProviderUnavailable)?;
    let guard = RecoverySessionGuard {
        manager: std::sync::Arc::clone(manager),
        session_id: session.session_id().to_string(),
    };
    Ok((session, guard))
}

#[cfg(test)]
mod recovery_session_tests {
    use super::{
        chat_path_readiness, recovery_session_config, ChatPathReadiness,
        RecoverySessionUnavailable, RECOVERY_SYSTEM_PROMPT,
    };
    use maekon_core::ai_readiness::{
        evaluate_ai_readiness, AiCapabilityId, AiConsentField, AiConsentReadiness,
        AiInvocationGuardState, AiModelAvailability, AiProviderAuthReadiness, AiProviderDetection,
        AiProviderInvocationReadiness, AiReadinessDimensions, AiRuntimeApplyRequirement,
    };
    use maekon_core::config::{AiAccessMode, AiProviderType, ExternalApiEndpoint};
    use maekon_core::models::ai_session::SessionTransport;
    use ChatPathReadiness::{NeedsExternalTextConsent, NotReady, Ready};

    const SURFACE: &str = "provider_surface.example.http";

    fn http_endpoint(surface_id: Option<&str>) -> ExternalApiEndpoint {
        ExternalApiEndpoint {
            endpoint: "https://api.example.test/v1/responses".to_string(),
            api_key: String::new(),
            model: Some("model-x".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::Ollama,
            surface_id: surface_id.map(str::to_string),
            credential: None,
        }
    }

    fn only(
        id: AiCapabilityId,
        state: ChatPathReadiness,
    ) -> impl Fn(AiCapabilityId) -> ChatPathReadiness {
        move |capability| if capability == id { state } else { NotReady }
    }

    fn pick(
        readiness: impl Fn(AiCapabilityId) -> ChatPathReadiness,
        llm_api: Option<&ExternalApiEndpoint>,
    ) -> Result<SessionTransport, RecoverySessionUnavailable> {
        recovery_session_config(readiness, llm_api).map(|config| config.transport)
    }

    #[test]
    fn no_ready_chat_path_opens_nothing() {
        let named = http_endpoint(Some(SURFACE));
        assert_eq!(
            pick(|_| NotReady, Some(&named)),
            Err(RecoverySessionUnavailable::ProviderNotReady)
        );
    }

    /// #12712: the session factory refuses a CLI session with tools off
    /// (#12094), so a ready CLI alone points the user at Chat instead of
    /// failing as if the provider were offline.
    #[test]
    fn ready_cli_alone_asks_for_an_open_chat_instead_of_a_session() {
        let named = http_endpoint(Some(SURFACE));
        assert_eq!(
            pick(only(AiCapabilityId::ChatSubprocess, Ready), Some(&named)),
            Err(RecoverySessionUnavailable::CliNeedsOpenChat)
        );
        assert_eq!(
            pick(only(AiCapabilityId::ChatSubprocess, Ready), None),
            Err(RecoverySessionUnavailable::CliNeedsOpenChat)
        );
    }

    /// #12712: every config recovery returns must pass the tool policy the
    /// session factory applies before it spawns a CLI (#12094). Re-adding the
    /// CLI path makes a ready CLI produce a config this policy refuses.
    #[test]
    fn no_recovery_config_is_refused_by_the_subprocess_tool_policy() {
        use crate::session_adapters::invocation_policy::InvocationPolicy;

        let named = http_endpoint(Some(SURFACE));
        let unnamed = http_endpoint(None);
        let states = [Ready, NeedsExternalTextConsent, NotReady];
        let mut opened = 0;
        for cli in states {
            for http in states {
                for local in states {
                    let readiness = |id| match id {
                        AiCapabilityId::ChatSubprocess => cli,
                        AiCapabilityId::ChatHttpApi => http,
                        AiCapabilityId::ChatLocalLlm => local,
                        _ => NotReady,
                    };
                    for llm_api in [Some(&named), Some(&unnamed), None] {
                        let Ok(config) = recovery_session_config(readiness, llm_api) else {
                            continue;
                        };
                        opened += 1;
                        assert!(!config.tools_enabled);
                        if config.transport == SessionTransport::Subprocess {
                            if let Err(error) = InvocationPolicy::require_tools_enabled(&config) {
                                panic!(
                                    "recovery opened a CLI session the factory refuses: {error} \
                                     (cli={cli:?}, http={http:?}, local={local:?})"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(opened > 0, "the sweep must reach configs that open");
    }

    #[test]
    fn ready_http_names_its_surface_and_model_or_opens_nothing() {
        let http_ready = only(AiCapabilityId::ChatHttpApi, Ready);
        let named = http_endpoint(Some(SURFACE));
        let config =
            recovery_session_config(&http_ready, Some(&named)).expect("the ready HTTP path opens");
        assert_eq!(config.transport, SessionTransport::HttpApi);
        assert_eq!(config.surface_id.as_deref(), Some(SURFACE));
        assert_eq!(config.model.as_deref(), Some("model-x"));
        assert!(!config.tools_enabled);
        let not_ready = Err(RecoverySessionUnavailable::ProviderNotReady);
        assert_eq!(pick(&http_ready, Some(&http_endpoint(None))), not_ready);
        assert_eq!(pick(&http_ready, None), not_ready);
    }

    #[test]
    fn follows_the_chat_page_order_after_the_cli() {
        let named = http_endpoint(Some(SURFACE));
        let unnamed = http_endpoint(None);
        let no_cli = |id| {
            if id == AiCapabilityId::ChatSubprocess {
                NotReady
            } else {
                Ready
            }
        };
        // A ready CLI does not shadow a ready HTTP or local path (#12712).
        assert_eq!(pick(|_| Ready, Some(&named)), Ok(SessionTransport::HttpApi));
        assert_eq!(
            pick(|_| Ready, Some(&unnamed)),
            Ok(SessionTransport::LocalLlm)
        );
        assert_eq!(pick(no_cli, Some(&named)), Ok(SessionTransport::HttpApi));
        assert_eq!(pick(no_cli, Some(&unnamed)), Ok(SessionTransport::LocalLlm));
    }

    #[test]
    fn a_path_missing_only_the_consent_names_that_consent() {
        let named = http_endpoint(Some(SURFACE));
        let http_needs_consent = only(AiCapabilityId::ChatHttpApi, NeedsExternalTextConsent);
        assert_eq!(
            pick(&http_needs_consent, Some(&named)),
            Err(RecoverySessionUnavailable::ExternalTextConsent)
        );
        // Granting the consent opens the HTTP path, so it is named before the
        // CLI, which would still need an open Chat.
        let http_consent_cli_ready = |id| match id {
            AiCapabilityId::ChatSubprocess => Ready,
            AiCapabilityId::ChatHttpApi => NeedsExternalTextConsent,
            _ => NotReady,
        };
        assert_eq!(
            pick(http_consent_cli_ready, Some(&named)),
            Err(RecoverySessionUnavailable::ExternalTextConsent)
        );
        // A CLI missing only the consent would still not open here (#12712).
        let cli_needs_consent = only(AiCapabilityId::ChatSubprocess, NeedsExternalTextConsent);
        assert_eq!(
            pick(&cli_needs_consent, None),
            Err(RecoverySessionUnavailable::CliNeedsOpenChat)
        );
        // A ready path still answers.
        let local_ready = |id| match id {
            AiCapabilityId::ChatSubprocess => NeedsExternalTextConsent,
            AiCapabilityId::ChatLocalLlm => Ready,
            _ => NotReady,
        };
        assert_eq!(pick(local_ready, None), Ok(SessionTransport::LocalLlm));
    }

    #[test]
    fn a_fixed_system_prompt_keeps_the_activity_summary_out() {
        // `create_session_impl` fills a missing system prompt from the session
        // context assembler: recent apps, window titles, and regime.
        let named = http_endpoint(Some(SURFACE));
        let config = recovery_session_config(|_| Ready, Some(&named)).expect("a ready path opens");
        assert_eq!(
            config.system_prompt.as_deref(),
            Some(RECOVERY_SYSTEM_PROMPT)
        );
    }

    fn cli_dimensions(full_text_granted: bool) -> AiReadinessDimensions {
        AiReadinessDimensions {
            compiled_capability: true,
            selected_access_mode: AiAccessMode::ProviderSubscriptionCli,
            access_mode_compatible: true,
            endpoint_or_profile_configured: true,
            provider_detection: AiProviderDetection::Detected,
            provider_auth: AiProviderAuthReadiness::Ready,
            provider_invocation: AiProviderInvocationReadiness::Ready,
            model_availability: AiModelAvailability::NotRequired,
            runtime_flag_enabled: true,
            consent: vec![AiConsentReadiness {
                field: AiConsentField::FullTextExtraction,
                granted: full_text_granted,
            }],
            apply_requirement: AiRuntimeApplyRequirement::Restart,
            apply_pending: false,
            privacy_gate: AiInvocationGuardState::EnforcedAtInvocation,
            egress_gate: AiInvocationGuardState::EnforcedAtInvocation,
            budget_gate: AiInvocationGuardState::EnforcedAtInvocation,
            audit_gate: AiInvocationGuardState::EnforcedAtInvocation,
        }
    }

    #[test]
    fn readiness_blocked_only_by_the_full_text_consent_is_told_apart() {
        let classify = |dimensions| {
            let item = evaluate_ai_readiness(AiCapabilityId::ChatSubprocess, dimensions);
            chat_path_readiness(Some(&item))
        };
        assert_eq!(classify(cli_dimensions(true)), Ready);
        assert_eq!(classify(cli_dimensions(false)), NeedsExternalTextConsent);
        // Missing the consent and something else is not a consent-only block.
        let mut unauthenticated = cli_dimensions(false);
        unauthenticated.provider_auth = AiProviderAuthReadiness::Required;
        assert_eq!(classify(unauthenticated), NotReady);
        // Another missing consent is not the full-text consent's to name.
        let mut also_ocr = cli_dimensions(false);
        also_ocr.consent.push(AiConsentReadiness {
            field: AiConsentField::OcrProcessing,
            granted: false,
        });
        assert_eq!(classify(also_ocr), NotReady);
        assert_eq!(chat_path_readiness(None), NotReady);
    }
}
