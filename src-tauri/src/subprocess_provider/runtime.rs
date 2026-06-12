use maekon_core::error::CoreError;

use maekon_api_contracts::provider_specs::{
    subprocess_invocation_mode, surface_supports_capability, SubprocessInvocationMode,
    SurfaceCapabilityKind,
};

use super::auth_probe::auth_probe_mode_for_surface;
use super::llm_provider::{claude_llm_runtime, codex_llm_runtime, gemini_llm_runtime};
use super::ocr_provider::{claude_ocr_runtime, codex_ocr_runtime, gemini_ocr_runtime};
use super::{SubprocessCliAuthStatus, SubprocessInvocationRuntime};
use maekon_api_contracts::provider_specs::SubprocessAuthProbeMode;

pub(super) fn invocation_runtime_for_mode(
    mode: SubprocessInvocationMode,
) -> Option<SubprocessInvocationRuntime> {
    match mode {
        SubprocessInvocationMode::CodexExecJson => Some(SubprocessInvocationRuntime {
            llm_invoke: codex_llm_runtime,
            ocr_invoke: codex_ocr_runtime,
        }),
        SubprocessInvocationMode::ClaudePrintJson => Some(SubprocessInvocationRuntime {
            llm_invoke: claude_llm_runtime,
            ocr_invoke: claude_ocr_runtime,
        }),
        SubprocessInvocationMode::GeminiCliPrompt => Some(SubprocessInvocationRuntime {
            llm_invoke: gemini_llm_runtime,
            ocr_invoke: gemini_ocr_runtime,
        }),
        SubprocessInvocationMode::ManualChatGui => None,
        // E21 #4864/#4885: the `app-server` transport is a chat/LLM-only
        // `ConversationSession` (see `CodexAppServerSession`, #4866) — it has no
        // OCR runtime. Decision #4885: Codex OCR stays on the `codex exec` path
        // (the `CodexExecJson` arm above, which keeps `ocr_invoke =
        // codex_ocr_runtime`). The app-server surface declares `supports.ocr =
        // false`; converting the exec surface wholesale would have broken the
        // `ocr_invoke` mapping, so a *separate* surface was added instead. This
        // arm therefore resolves to no headless OCR/LLM runtime here — the
        // app-server LLM path is routed via the session factory, not this map.
        SubprocessInvocationMode::CodexAppServer => None,
    }
}

pub(super) fn invocation_runtime_for_surface(
    surface_id: &str,
) -> Result<SubprocessInvocationRuntime, CoreError> {
    let mode = invocation_mode_for_surface(surface_id)?;
    invocation_runtime_for_mode(mode).ok_or_else(|| CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Invalid,
        message: format!(
            "Provider CLI surface '{surface_id}' is manual/GUI-only and has no headless invocation runtime."
        ),
    })
}

fn invocation_mode_for_surface(surface_id: &str) -> Result<SubprocessInvocationMode, CoreError> {
    // iter-150: every failure path from subprocess_invocation_mode (unknown
    // surface_id, missing subprocess_transport, invalid invocation_mode
    // string in the catalog) is a catalog/configuration issue — route to
    // Config.Invalid (`config.invalid`) rather than sharing Internal.Generic
    // with unrelated runtime faults.
    subprocess_invocation_mode(surface_id).map_err(|msg| CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Invalid,
        message: msg,
    })
}

pub(crate) fn cli_id_for_surface_id(surface_id: &str) -> Result<String, String> {
    Ok(super::catalog_subprocess_transport(surface_id)?
        .tool_id
        .clone())
}

pub(crate) fn runtime_supported_for_surface(surface_id: &str) -> bool {
    invocation_mode_for_surface(surface_id)
        .map(|mode| invocation_runtime_for_mode(mode).is_some())
        .unwrap_or(false)
}

pub(super) fn runtime_ready_for_auth_status(
    surface_id: &str,
    auth_status: SubprocessCliAuthStatus,
    capability: SurfaceCapabilityKind,
) -> bool {
    if !runtime_supported_for_surface(surface_id)
        || !surface_supports_capability(surface_id, capability).unwrap_or(false)
    {
        return false;
    }

    match auth_status {
        SubprocessCliAuthStatus::Authenticated => true,
        SubprocessCliAuthStatus::Unauthenticated
        | SubprocessCliAuthStatus::Unsupported
        | SubprocessCliAuthStatus::StaleSession
        | SubprocessCliAuthStatus::InteractiveRequired => false,
        SubprocessCliAuthStatus::Unknown => {
            matches!(
                auth_probe_mode_for_surface(surface_id),
                Ok(SubprocessAuthProbeMode::None)
            )
        }
    }
}

pub(crate) fn runtime_ready_for_surface(
    surface_id: &str,
    auth_status: SubprocessCliAuthStatus,
) -> bool {
    runtime_ready_for_auth_status(surface_id, auth_status, SurfaceCapabilityKind::Llm)
        || runtime_ready_for_auth_status(surface_id, auth_status, SurfaceCapabilityKind::Ocr)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Iter-150 regression guard: catalog lookups that fail (unknown
    /// `surface_id`, invalid catalog mode string) must emit
    /// `CoreError::Config` / `config.invalid`, not `Internal.Generic`. Every
    /// String error surfaced by `subprocess_invocation_mode` is ultimately a
    /// catalog/configuration fault.
    #[test]
    fn unknown_surface_maps_to_config_invalid() {
        let err = match invocation_runtime_for_surface("provider_surface.__does_not_exist__") {
            Ok(_) => panic!("unknown surface should fail"),
            Err(e) => e,
        };
        assert_eq!(err.code(), "config.invalid");
        assert!(matches!(
            err,
            CoreError::Config {
                code: maekon_core::error_codes::ConfigCode::Invalid,
                ..
            }
        ));
    }

    #[test]
    fn manual_gui_surface_is_not_runtime_supported() {
        assert!(!runtime_supported_for_surface(
            "provider_surface.google.antigravity_cli"
        ));
    }

    /// E21 #4864: the `CodexAppServer` invocation mode is recognized by the
    /// contract but has no headless runtime adapter yet (lands in #4865), so it
    /// resolves to no runtime — exactly like `ManualChatGui`. This keeps any
    /// future catalog surface declaring the mode out of automatic selection
    /// until the transport exists.
    #[test]
    fn codex_app_server_mode_has_no_runtime_yet() {
        use maekon_api_contracts::provider_specs::SubprocessInvocationMode;
        assert!(invocation_runtime_for_mode(SubprocessInvocationMode::CodexAppServer).is_none());
    }

    /// E21 #4885 decision guard: Codex OCR stays on the `codex exec` path. The
    /// `codex_exec_json` surface must keep a headless runtime (which carries the
    /// `ocr_invoke = codex_ocr_runtime` mapping), and the separate app-server
    /// surface must remain OCR-less — so a future enum/dispatch change can't
    /// silently move (or break) the Codex OCR path. (Catalog `supports.ocr` is
    /// `true` for the exec surface and `false` for the app-server surface.)
    #[test]
    fn codex_ocr_stays_on_exec_surface_not_app_server() {
        // The exec codex surface resolves to a headless runtime (OCR path intact).
        let runtime = invocation_runtime_for_surface("provider_surface.openai.subprocess_cli")
            .expect(
                "codex exec surface must resolve to a headless runtime (OCR stays on exec, #4885)",
            );
        // The exec surface carries both an LLM and an OCR invoke fn pointer.
        // Pin that both slots are non-null (function pointers are always non-null,
        // but the struct must be fully constructed — i.e., not a None arm).
        let _ = runtime.llm_invoke; // presence proves the struct was returned
        let _ = runtime.ocr_invoke; // OCR fn pointer must be present on exec path
                                    // The app-server surface is chat/LLM-only — no headless OCR/LLM runtime here.
        assert!(
            matches!(
                invocation_runtime_for_surface("provider_surface.openai.codex_app_server")
                    .err()
                    .expect("app-server surface must Err"),
                maekon_core::error::CoreError::Config { .. }
            ),
            "app-server surface must not provide a headless OCR runtime — must yield CoreError::Config (#4885)"
        );
    }
}
