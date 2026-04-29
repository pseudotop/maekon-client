use oneshim_core::error::CoreError;

use oneshim_api_contracts::provider_specs::{
    subprocess_invocation_mode, surface_supports_capability, SubprocessInvocationMode,
    SurfaceCapabilityKind,
};

use super::auth_probe::auth_probe_mode_for_surface;
use super::llm_provider::{claude_llm_runtime, codex_llm_runtime, gemini_llm_runtime};
use super::ocr_provider::{claude_ocr_runtime, codex_ocr_runtime, gemini_ocr_runtime};
use super::{SubprocessCliAuthStatus, SubprocessInvocationRuntime};
use oneshim_api_contracts::provider_specs::SubprocessAuthProbeMode;

pub(super) fn invocation_runtime_for_mode(
    mode: SubprocessInvocationMode,
) -> SubprocessInvocationRuntime {
    match mode {
        SubprocessInvocationMode::CodexExecJson => SubprocessInvocationRuntime {
            llm_invoke: codex_llm_runtime,
            ocr_invoke: codex_ocr_runtime,
        },
        SubprocessInvocationMode::ClaudePrintJson => SubprocessInvocationRuntime {
            llm_invoke: claude_llm_runtime,
            ocr_invoke: claude_ocr_runtime,
        },
        SubprocessInvocationMode::GeminiCliPrompt => SubprocessInvocationRuntime {
            llm_invoke: gemini_llm_runtime,
            ocr_invoke: gemini_ocr_runtime,
        },
    }
}

pub(super) fn invocation_runtime_for_surface(
    surface_id: &str,
) -> Result<SubprocessInvocationRuntime, CoreError> {
    Ok(invocation_runtime_for_mode(invocation_mode_for_surface(
        surface_id,
    )?))
}

fn invocation_mode_for_surface(surface_id: &str) -> Result<SubprocessInvocationMode, CoreError> {
    // iter-150: every failure path from subprocess_invocation_mode (unknown
    // surface_id, missing subprocess_transport, invalid invocation_mode
    // string in the catalog) is a catalog/configuration issue — route to
    // Config.Invalid (`config.invalid`) rather than sharing Internal.Generic
    // with unrelated runtime faults.
    subprocess_invocation_mode(surface_id).map_err(|msg| CoreError::Config {
        code: oneshim_core::error_codes::ConfigCode::Invalid,
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
        .map(|mode| {
            let _ = invocation_runtime_for_mode(mode);
            true
        })
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
        SubprocessCliAuthStatus::Unauthenticated => false,
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
                code: oneshim_core::error_codes::ConfigCode::Invalid,
                ..
            }
        ));
    }
}
