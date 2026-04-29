mod auth_probe;
mod llm_provider;
mod ocr_provider;
mod parsing;
pub(crate) mod runtime;
mod surface_selection;

use oneshim_core::error::CoreError;
use oneshim_core::ports::ocr_provider::OcrResult;
use serde::Deserialize;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;

use oneshim_api_contracts::provider_specs::{
    subprocess_transport as catalog_subprocess_transport, SurfaceCapabilityKind,
};

// ── Re-exports ────────────────────────────────────────────────

pub use llm_provider::SubprocessLlmProvider;
pub use ocr_provider::SubprocessOcrProvider;
pub(crate) use runtime::cli_id_for_surface_id;
pub(crate) use runtime::runtime_ready_for_surface;
pub(crate) use runtime::runtime_supported_for_surface;
#[allow(unused_imports)]
pub use surface_selection::{
    detect_known_cli_surfaces, preferred_cli_surface_for_capability,
    preferred_cli_surface_for_config, probe_for_surface_id, probe_known_cli_surfaces,
    select_cli_surface_for_capability, select_cli_surface_for_config,
};

// ── Shared constants ──────────────────────────────────────────

const DEFAULT_SUBPROCESS_TIMEOUT_SECS: u64 = 60;
const CLI_AUTH_PROBE_TIMEOUT_SECS: u64 = 2;
const ACTION_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "target_text": { "type": ["string", "null"] },
    "target_role": { "type": ["string", "null"] },
    "action_type": { "type": "string" },
    "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
  },
  "required": ["target_text", "target_role", "action_type", "confidence"],
  "additionalProperties": false
}"#;
const OCR_SCHEMA_JSON: &str = r#"{
  "type": "object",
  "properties": {
    "results": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "text": { "type": "string" },
          "x": { "type": "integer" },
          "y": { "type": "integer" },
          "width": { "type": "integer", "minimum": 0 },
          "height": { "type": "integer", "minimum": 0 },
          "confidence": { "type": "number", "minimum": 0.0, "maximum": 1.0 }
        },
        "required": ["text", "x", "y", "width", "height", "confidence"],
        "additionalProperties": false
      }
    }
  },
  "required": ["results"],
  "additionalProperties": false
}"#;

// ── Shared types ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSubprocessCli {
    pub surface_id: String,
    pub executable_path: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubprocessCliAuthStatus {
    Authenticated,
    Unauthenticated,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbedSubprocessCli {
    pub detected: DetectedSubprocessCli,
    pub auth_status: SubprocessCliAuthStatus,
    pub auth_detail: Option<String>,
}

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy)]
struct SubprocessInvocationRuntime {
    llm_invoke:
        for<'a> fn(&'a SubprocessLlmProvider, &'a str) -> BoxFuture<'a, Result<String, CoreError>>,
    ocr_invoke: for<'a> fn(
        &'a SubprocessOcrProvider,
        &'a Path,
        &'a Path,
    ) -> BoxFuture<'a, Result<String, CoreError>>,
}

#[derive(Debug, Clone, Deserialize)]
struct SubprocessOcrEnvelope {
    results: Vec<OcrResult>,
}

// ── Internal helper re-exports for submodules ─────────────────

pub(crate) use parsing::{
    append_model_flag, append_oneshot_flags, classify_subprocess_error, SubprocessKind,
};
#[allow(unused_imports)]
use parsing::{
    build_codex_ocr_prompt, build_intent_prompt, build_path_based_ocr_prompt, find_executable,
    is_gemini_json_flag_error, parse_interpreted_action_output, parse_ocr_output,
    truncate_for_error, write_subprocess_ocr_image,
};
use runtime::invocation_runtime_for_surface;

fn default_llm_model_for_surface(surface_id: &str) -> Result<Option<String>, String> {
    oneshim_api_contracts::provider_specs::default_surface_model(
        surface_id,
        SurfaceCapabilityKind::Llm,
    )
}

fn default_ocr_model_for_surface(surface_id: &str) -> Result<Option<String>, String> {
    oneshim_api_contracts::provider_specs::default_surface_model(
        surface_id,
        SurfaceCapabilityKind::Ocr,
    )
}

fn provider_name_for_surface_id(surface_id: &str) -> Result<String, String> {
    Ok(format!("subprocess-{}", cli_id_for_surface_id(surface_id)?))
}
