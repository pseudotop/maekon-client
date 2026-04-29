use oneshim_core::error::CoreError;
use oneshim_core::ports::llm_provider::{InterpretedAction, ScreenContext, SkillContext};
use oneshim_core::ports::ocr_provider::OcrResult;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use super::{
    catalog_subprocess_transport, SubprocessOcrEnvelope, ACTION_SCHEMA_JSON, OCR_SCHEMA_JSON,
};

pub(super) fn build_intent_prompt(
    screen_context: &ScreenContext,
    intent_hint: &str,
    skill_ctx: &SkillContext,
) -> Result<String, CoreError> {
    let screen_context_json = serde_json::to_string_pretty(screen_context)?;
    let available_skills = if skill_ctx.available_skills.is_empty() {
        "[]".to_string()
    } else {
        serde_json::to_string(
            &skill_ctx
                .available_skills
                .iter()
                .map(|skill| skill.name.clone())
                .collect::<Vec<_>>(),
        )?
    };
    let active_skill = skill_ctx
        .active_skill_body
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("(none)");

    Ok(format!(
        "You are Maekon's subprocess-backed UI intent planner.\n\
Return only compact JSON matching this schema:\n{schema}\n\n\
Rules:\n\
- action_type must be one of: click, type, hotkey, wait, activate.\n\
- confidence must be a number between 0.0 and 1.0.\n\
- target_text should be the visible text to target when known, otherwise null.\n\
- target_role should be a concise accessibility-style role when known, otherwise null.\n\
- Do not include markdown, commentary, or code fences.\n\n\
Available skill names: {available_skills}\n\
Active skill body:\n{active_skill}\n\n\
Intent hint:\n{intent_hint}\n\n\
Screen context JSON:\n{screen_context_json}",
        schema = ACTION_SCHEMA_JSON,
        available_skills = available_skills,
        active_skill = active_skill,
        intent_hint = intent_hint.trim(),
        screen_context_json = screen_context_json
    ))
}

pub(super) fn build_codex_ocr_prompt(model: &str) -> String {
    format!(
        "You are Maekon's subprocess-backed OCR extractor.\n\
Use the attached image as the only source of truth.\n\
Return strict JSON matching this schema:\n{schema}\n\n\
Rules:\n\
- Include every visible text region that matters for UI interaction.\n\
- Use x/y/width/height when they are reasonably inferable from the image.\n\
- If geometry is uncertain, use 0 for coordinates and size.\n\
- confidence must be a number between 0.0 and 1.0.\n\
- Do not include markdown, commentary, or code fences.\n\
- The selected model is '{model}'.",
        schema = OCR_SCHEMA_JSON,
        model = model
    )
}

pub(super) fn build_path_based_ocr_prompt(image_path: &Path, model: &str) -> String {
    format!(
        "You are Maekon's subprocess-backed OCR extractor.\n\
Read the local image file at this path:\n{image_path}\n\n\
Return strict JSON matching this schema:\n{schema}\n\n\
Rules:\n\
- Include every visible text region that matters for UI interaction.\n\
- Use x/y/width/height when they are reasonably inferable from the image.\n\
- If geometry is uncertain, use 0 for coordinates and size.\n\
- confidence must be a number between 0.0 and 1.0.\n\
- Do not include markdown, commentary, or code fences.\n\
- The selected model is '{model}'.",
        image_path = image_path.display(),
        schema = OCR_SCHEMA_JSON,
        model = model
    )
}

pub(super) fn write_subprocess_ocr_image(
    workdir: &Path,
    image: &[u8],
    image_format: &str,
) -> Result<PathBuf, CoreError> {
    let extension = match image_format.trim().to_ascii_lowercase().as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        "gif" => "gif",
        "bmp" => "bmp",
        _ => "bin",
    };
    let path = workdir.join(format!("ocr-input.{extension}"));
    std::fs::write(&path, image).map_err(|err| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!("Failed to write subprocess OCR image input: {err}"),
    })?;
    Ok(path)
}

pub(super) fn parse_ocr_output(raw: &str) -> Result<Vec<OcrResult>, CoreError> {
    let normalized = raw.trim();
    // Iter-93: both the empty-response and unparseable-output paths are OCR
    // provider failures — route through CoreError::OcrError so the wire code
    // is `provider.ocr_failed`, not the generic `internal.generic` fallback.
    // Telemetry and retry logic can then distinguish "our parse layer broke"
    // from "the OCR subprocess misbehaved" (this branch).
    if normalized.is_empty() {
        return Err(CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: "Subprocess CLI returned an empty OCR response.".to_string(),
        });
    }

    if let Ok(envelope) = serde_json::from_str::<SubprocessOcrEnvelope>(normalized) {
        return Ok(normalize_ocr_results(envelope.results));
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(normalized) {
        if let Some(results) = parse_ocr_value(&value) {
            return Ok(normalize_ocr_results(results));
        }
    }

    if let Some(fragment) = extract_json_object_fragment(normalized) {
        if let Ok(envelope) = serde_json::from_str::<SubprocessOcrEnvelope>(&fragment) {
            return Ok(normalize_ocr_results(envelope.results));
        }
    }

    Err(CoreError::OcrError {
        code: oneshim_core::error_codes::ProviderCode::OcrFailed,
        message: format!(
            "Subprocess CLI returned non-JSON OCR output: {}",
            truncate_for_error(normalized)
        ),
    })
}

fn parse_ocr_value(value: &serde_json::Value) -> Option<Vec<OcrResult>> {
    if let Ok(envelope) = serde_json::from_value::<SubprocessOcrEnvelope>(value.clone()) {
        return Some(envelope.results);
    }

    match value {
        serde_json::Value::Object(map) => {
            for key in ["result", "response", "content", "message", "data"] {
                if let Some(nested) = map.get(key) {
                    if let Some(results) = parse_ocr_value(nested) {
                        return Some(results);
                    }
                }
            }
            None
        }
        serde_json::Value::String(text) => serde_json::from_str::<SubprocessOcrEnvelope>(text)
            .ok()
            .map(|envelope| envelope.results)
            .or_else(|| {
                extract_json_object_fragment(text).and_then(|fragment| {
                    serde_json::from_str::<SubprocessOcrEnvelope>(&fragment)
                        .ok()
                        .map(|envelope| envelope.results)
                })
            }),
        serde_json::Value::Array(items) => items.iter().find_map(parse_ocr_value),
        _ => None,
    }
}

fn normalize_ocr_results(results: Vec<OcrResult>) -> Vec<OcrResult> {
    results
        .into_iter()
        .filter_map(|mut result| {
            if result.text.trim().is_empty() {
                return None;
            }
            result.confidence = result.confidence.clamp(0.0, 1.0);
            Some(result)
        })
        .collect()
}

pub(super) fn parse_interpreted_action_output(raw: &str) -> Result<InterpretedAction, CoreError> {
    let normalized = raw.trim();
    // Iter-93: LLM subprocess returning empty / non-JSON intent output is a
    // provider failure (LLM misbehaved), not an internal-code failure. Route
    // through CoreError::Analysis so wire code is `provider.analysis_failed`
    // consistent with the AnalysisClient and RemoteLlmProvider surfaces.
    if normalized.is_empty() {
        return Err(CoreError::Analysis {
            code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
            message: "Subprocess CLI returned an empty response.".to_string(),
        });
    }

    if let Ok(action) = serde_json::from_str::<InterpretedAction>(normalized) {
        return Ok(clamp_confidence(action));
    }

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(normalized) {
        if let Some(action) = parse_interpreted_action_value(&value) {
            return Ok(clamp_confidence(action));
        }
    }

    if let Some(fragment) = extract_json_object_fragment(normalized) {
        if let Ok(action) = serde_json::from_str::<InterpretedAction>(&fragment) {
            return Ok(clamp_confidence(action));
        }
    }

    Err(CoreError::Analysis {
        code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
        message: format!(
            "Subprocess CLI returned non-JSON intent output: {}",
            truncate_for_error(normalized)
        ),
    })
}

fn parse_interpreted_action_value(value: &serde_json::Value) -> Option<InterpretedAction> {
    if let Ok(action) = serde_json::from_value::<InterpretedAction>(value.clone()) {
        return Some(action);
    }

    match value {
        serde_json::Value::Object(map) => {
            for key in ["result", "response", "content", "message"] {
                if let Some(nested) = map.get(key) {
                    if let Some(action) = parse_interpreted_action_value(nested) {
                        return Some(action);
                    }
                }
            }
            None
        }
        serde_json::Value::String(text) => serde_json::from_str::<InterpretedAction>(text).ok(),
        serde_json::Value::Array(items) => items.iter().find_map(parse_interpreted_action_value),
        _ => None,
    }
}

fn clamp_confidence(mut action: InterpretedAction) -> InterpretedAction {
    action.confidence = action.confidence.clamp(0.0, 1.0);
    action
}

pub(super) fn extract_json_object_fragment(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(raw[start..=end].to_string())
}

pub(super) fn truncate_for_error(value: &str) -> String {
    const MAX_LEN: usize = 240;
    if value.chars().count() <= MAX_LEN {
        return value.to_string();
    }
    let truncated: String = value.chars().take(MAX_LEN).collect();
    format!("{truncated}...")
}

/// Subprocess role hint used by `classify_subprocess_error` to pick a
/// provider-specific wire code for the generic "CLI invocation failed"
/// fallthrough (iter-149). Auth-keyword stderr still routes to
/// `CoreError::Auth` regardless of kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubprocessKind {
    /// LLM intent-planning subprocess — fallthrough → `CoreError::Analysis`
    /// (`provider.analysis_failed`).
    Llm,
    /// OCR extraction subprocess — fallthrough → `CoreError::OcrError`
    /// (`provider.ocr_failed`).
    Ocr,
}

pub(crate) fn classify_subprocess_error(
    kind: SubprocessKind,
    surface_id: &str,
    stderr: &str,
) -> CoreError {
    let normalized = stderr.trim();
    let lowered = normalized.to_ascii_lowercase();
    let cli_id =
        super::cli_id_for_surface_id(surface_id).unwrap_or_else(|_| surface_id.to_string());
    if lowered.contains("login")
        || lowered.contains("auth")
        || lowered.contains("sign in")
        || lowered.contains("not authenticated")
    {
        return CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
            message: format!(
                "{} CLI authentication is required: {}",
                cli_id,
                truncate_for_error(normalized)
            ),
        };
    }

    let message = format!(
        "{} CLI invocation failed: {}",
        cli_id,
        truncate_for_error(normalized)
    );
    match kind {
        SubprocessKind::Llm => CoreError::Analysis {
            code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
            message,
        },
        SubprocessKind::Ocr => CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message,
        },
    }
}

pub(super) fn is_gemini_json_flag_error(error: &CoreError) -> bool {
    // Iter-93: expanded to include CoreError::Analysis and CoreError::OcrError
    // so flag-error detection keeps working after subprocess parser errors
    // were re-routed from Internal to the domain-specific variants.
    let message = match error {
        CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: value,
        }
        | CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: value,
        }
        | CoreError::Auth {
            code: oneshim_core::error_codes::AuthCode::Failed,
            message: value,
        }
        | CoreError::Analysis {
            code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
            message: value,
        }
        | CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: value,
        } => value,
        _ => return false,
    };
    let lowered = message.to_ascii_lowercase();
    lowered.contains("output-format")
        && (lowered.contains("unknown option")
            || lowered.contains("unknown arguments")
            || lowered.contains("unexpected argument")
            || lowered.contains("unrecognized option"))
}

pub(crate) fn append_model_flag(command: &mut Command, surface_id: &str, model: &str) {
    if let Ok(transport) = catalog_subprocess_transport(surface_id) {
        if let Some(flag) = transport
            .model_flag
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            command.arg(flag).arg(model);
        }
    }
}

/// Append catalog-driven oneshot flags (e.g. `--bare`, `--no-session-persistence`) to a CLI command.
/// Callers set `--output-format` separately since LLM/OCR and session modes need different formats.
pub(crate) fn append_oneshot_flags(command: &mut Command, surface_id: &str) {
    if let Ok(transport) = catalog_subprocess_transport(surface_id) {
        for flag in &transport.oneshot_flags {
            command.arg(flag);
        }
    }
}

pub(super) fn find_executable(name: &str) -> Option<PathBuf> {
    if name.contains(std::path::MAIN_SEPARATOR) {
        let path = PathBuf::from(name);
        return is_executable(&path).then_some(path);
    }

    let path_var = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let exts: Vec<String> = std::env::var_os("PATHEXT")
        .map(|value| {
            std::env::split_paths(&PathBuf::from(value))
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                ".COM".to_string(),
                ".EXE".to_string(),
                ".BAT".to_string(),
                ".CMD".to_string(),
            ]
        });

    for dir in std::env::split_paths(&path_var) {
        let base = dir.join(name);
        if is_executable(&base) {
            return Some(base);
        }
        #[cfg(windows)]
        {
            for ext in &exts {
                let candidate = dir.join(format!("{name}{ext}"));
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            return metadata.permissions().mode() & 0o111 != 0;
        }
        false
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_direct_json_output() {
        let raw = r#"{"target_text":"Save","target_role":"button","action_type":"click","confidence":1.4}"#;
        let action = parse_interpreted_action_output(raw).unwrap();
        assert_eq!(action.target_text.as_deref(), Some("Save"));
        assert_eq!(action.target_role.as_deref(), Some("button"));
        assert_eq!(action.action_type, "click");
        assert_eq!(action.confidence, 1.0);
    }

    #[test]
    fn parses_nested_json_payload() {
        let raw = json!({
            "result": {
                "target_text": "Search",
                "target_role": "input",
                "action_type": "type",
                "confidence": 0.82
            }
        })
        .to_string();
        let action = parse_interpreted_action_output(&raw).unwrap();
        assert_eq!(action.target_text.as_deref(), Some("Search"));
        assert_eq!(action.action_type, "type");
    }

    #[test]
    fn parses_nested_ocr_json_payload() {
        let raw = json!({
            "response": {
                "results": [
                    {
                        "text": "Save",
                        "x": 10,
                        "y": 20,
                        "width": 80,
                        "height": 24,
                        "confidence": 1.2
                    }
                ]
            }
        })
        .to_string();
        let results = parse_ocr_output(&raw).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "Save");
        assert_eq!(results[0].confidence, 1.0);
    }

    #[test]
    fn parses_string_wrapped_ocr_json_payload() {
        let raw = json!({
            "message": "{\"results\":[{\"text\":\"Open\",\"x\":0,\"y\":0,\"width\":40,\"height\":18,\"confidence\":0.9}]}"
        })
        .to_string();
        let results = parse_ocr_output(&raw).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "Open");
    }

    #[test]
    fn builds_prompt_with_screen_context_and_skills() {
        let prompt = build_intent_prompt(
            &ScreenContext {
                visible_texts: vec!["Save".to_string()],
                active_app: "Editor".to_string(),
                active_window_title: "main.rs".to_string(),
                layout_description: Some("toolbar".to_string()),
            },
            "click save",
            &SkillContext::default(),
        )
        .unwrap();

        assert!(prompt.contains("click save"));
        assert!(prompt.contains("\"active_app\": \"Editor\""));
        assert!(prompt.contains("\"action_type\""));
    }

    /// Iter-93 regression guard: empty OCR output from the subprocess is an
    /// OCR provider failure (wire code `provider.ocr_failed`), not an
    /// internal-generic failure. Pre-iter-93 this was labelled
    /// `internal.generic`, hiding OCR provider misbehaviour in telemetry.
    #[test]
    fn empty_ocr_output_maps_to_ocr_error() {
        let err = parse_ocr_output("").unwrap_err();
        assert_eq!(err.code(), "provider.ocr_failed");
    }

    /// Iter-93 regression guard: non-JSON OCR output is also an OCR provider
    /// failure — the subprocess misbehaved by returning unparseable text.
    #[test]
    fn non_json_ocr_output_maps_to_ocr_error() {
        let err = parse_ocr_output("this is definitely not json").unwrap_err();
        assert_eq!(err.code(), "provider.ocr_failed");
    }

    /// Iter-93 regression guard: empty LLM intent output is an analysis
    /// provider failure (wire code `provider.analysis_failed`), consistent
    /// with AnalysisClient / RemoteLlmProvider surfaces.
    #[test]
    fn empty_intent_output_maps_to_analysis_error() {
        let err = parse_interpreted_action_output("").unwrap_err();
        assert_eq!(err.code(), "provider.analysis_failed");
    }

    /// Iter-93 regression guard: non-JSON LLM intent output is also an
    /// analysis provider failure.
    #[test]
    fn non_json_intent_output_maps_to_analysis_error() {
        let err = parse_interpreted_action_output("not-json-at-all").unwrap_err();
        assert_eq!(err.code(), "provider.analysis_failed");
    }

    /// Iter-93 regression guard: is_gemini_json_flag_error must still
    /// recognise the flag-error message when it arrives via the newly-
    /// routed CoreError::Analysis / CoreError::OcrError variants (not just
    /// Internal). Without this, the subprocess retry path for Gemini would
    /// stop triggering after the iter-93 re-routing.
    #[test]
    fn gemini_flag_error_detected_in_analysis_variant() {
        let err = CoreError::Analysis {
            code: oneshim_core::error_codes::ProviderCode::AnalysisFailed,
            message: "unknown option --output-format".into(),
        };
        assert!(is_gemini_json_flag_error(&err));
    }

    #[test]
    fn gemini_flag_error_detected_in_ocr_variant() {
        let err = CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: "unrecognized option: --output-format".into(),
        };
        assert!(is_gemini_json_flag_error(&err));
    }

    /// Iter-149 regression guard: LLM subprocess non-zero exit without auth
    /// keyword should surface as `CoreError::Analysis`
    /// (`provider.analysis_failed`), not `Internal.Generic`. The subprocess IS
    /// the LLM provider — its exit failure is a provider failure.
    #[test]
    fn llm_subprocess_generic_exit_maps_to_analysis() {
        let err =
            classify_subprocess_error(SubprocessKind::Llm, "codex_llm", "segfault: core dumped");
        assert_eq!(err.code(), "provider.analysis_failed");
    }

    /// Iter-149 regression guard: OCR subprocess non-zero exit without auth
    /// keyword should surface as `CoreError::OcrError`
    /// (`provider.ocr_failed`), not `Internal.Generic`.
    #[test]
    fn ocr_subprocess_generic_exit_maps_to_ocr_error() {
        let err = classify_subprocess_error(SubprocessKind::Ocr, "gemini_ocr", "model not found");
        assert_eq!(err.code(), "provider.ocr_failed");
    }

    /// Iter-149: auth-keyword stderr still routes to CoreError::Auth
    /// regardless of SubprocessKind. Verified for both Llm and Ocr kinds so
    /// a refactor that accidentally stops checking auth keywords in one
    /// branch fails the test.
    #[test]
    fn auth_keyword_wins_over_kind_for_llm() {
        let err = classify_subprocess_error(
            SubprocessKind::Llm,
            "codex_llm",
            "please sign in to continue",
        );
        assert_eq!(err.code(), "auth.failed");
    }

    #[test]
    fn auth_keyword_wins_over_kind_for_ocr() {
        let err = classify_subprocess_error(
            SubprocessKind::Ocr,
            "gemini_ocr",
            "not authenticated: run gcloud auth login",
        );
        assert_eq!(err.code(), "auth.failed");
    }
}
