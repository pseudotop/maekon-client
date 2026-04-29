use async_trait::async_trait;
use oneshim_core::config::AiProviderConfig;
use oneshim_core::error::CoreError;
use oneshim_core::ports::llm_provider::{
    InterpretedAction, LlmProvider, ScreenContext, SkillContext,
};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use super::{
    append_model_flag, append_oneshot_flags, build_intent_prompt, classify_subprocess_error,
    default_llm_model_for_surface, invocation_runtime_for_surface, is_gemini_json_flag_error,
    parse_interpreted_action_output, provider_name_for_surface_id, BoxFuture,
    DetectedSubprocessCli, SubprocessKind, ACTION_SCHEMA_JSON, DEFAULT_SUBPROCESS_TIMEOUT_SECS,
};
use oneshim_api_contracts::provider_specs::subprocess_supports_json_output;

#[derive(Debug, Clone)]
pub struct SubprocessLlmProvider {
    pub(super) surface: DetectedSubprocessCli,
    pub(super) provider_name: String,
    pub(super) model: String,
    pub(super) timeout: Duration,
}

impl SubprocessLlmProvider {
    pub fn new(surface: DetectedSubprocessCli, config: &AiProviderConfig) -> Self {
        let model = config
            .llm_api
            .as_ref()
            .and_then(|endpoint| endpoint.model.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                default_llm_model_for_surface(&surface.surface_id)
                    .ok()
                    .flatten()
            })
            .unwrap_or_else(|| "gpt-5.4".to_string());
        let timeout_secs = config
            .llm_api
            .as_ref()
            .map(|endpoint| endpoint.timeout_secs)
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SUBPROCESS_TIMEOUT_SECS);

        Self {
            provider_name: provider_name_for_surface_id(&surface.surface_id)
                .unwrap_or_else(|_| "subprocess-provider-cli".to_string()),
            surface,
            model,
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    async fn invoke(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        let prompt = build_intent_prompt(screen_context, intent_hint, skill_ctx)?;
        let runtime = invocation_runtime_for_surface(&self.surface.surface_id)?;
        let raw = (runtime.llm_invoke)(self, &prompt).await?;

        parse_interpreted_action_output(&raw)
    }

    pub(super) async fn run_codex(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Codex subprocess tempdir: {err}"),
        })?;
        let schema_path = temp_dir.path().join("action.schema.json");
        let output_path = temp_dir.path().join("codex-output.json");
        std::fs::write(&schema_path, ACTION_SCHEMA_JSON).map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to write Codex output schema: {err}"),
        })?;

        let mut child = Command::new(&self.surface.executable_path);
        child
            .arg("exec")
            .arg("--sandbox")
            .arg("read-only")
            .arg("--skip-git-repo-check")
            .arg("--color")
            .arg("never")
            .arg("-C")
            .arg(temp_dir.path())
            .arg("--output-schema")
            .arg(&schema_path)
            .arg("--output-last-message")
            .arg(&output_path)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_model_flag(&mut child, &self.surface.surface_id, &self.model);

        let mut child = child.spawn().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex CLI subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to open stdin for Codex CLI subprocess".to_string(),
        })?;
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(CoreError::Io)?;
        drop(stdin);

        let output = timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            return Err(classify_subprocess_error(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }

        if let Ok(rendered) = std::fs::read_to_string(&output_path) {
            if !rendered.trim().is_empty() {
                return Ok(rendered);
            }
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub(super) async fn run_claude(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Claude subprocess tempdir: {err}"),
        })?;

        let mut command = Command::new(&self.surface.executable_path);
        command.arg("-p");
        append_oneshot_flags(&mut command, &self.surface.surface_id);
        command
            .arg("--output-format")
            .arg("text")
            .arg("--json-schema")
            .arg(ACTION_SCHEMA_JSON)
            .arg(prompt)
            .current_dir(temp_dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        let output = timeout(self.timeout, command.output())
            .await
            .map_err(|_| CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            return Err(classify_subprocess_error(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub(super) async fn run_gemini(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Gemini subprocess tempdir: {err}"),
        })?;

        let output = match self.run_gemini_command(temp_dir.path(), prompt, true).await {
            Ok(output) => output,
            Err(error) if is_gemini_json_flag_error(&error) => {
                self.run_gemini_command(temp_dir.path(), prompt, false)
                    .await?
            }
            Err(error) => return Err(error),
        };

        if !output.status.success() {
            return Err(classify_subprocess_error(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    async fn run_gemini_command(
        &self,
        workdir: &Path,
        prompt: &str,
        prefer_json_output: bool,
    ) -> Result<std::process::Output, CoreError> {
        let mut command = Command::new(&self.surface.executable_path);
        command.arg("-p").arg(prompt);
        if prefer_json_output
            && subprocess_supports_json_output(&self.surface.surface_id).unwrap_or(false)
        {
            command.arg("--output-format").arg("json");
        }
        command
            .current_dir(workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        timeout(self.timeout, command.output())
            .await
            .map_err(|_| CoreError::RequestTimeout {
                code: oneshim_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)
    }
}

#[async_trait]
impl LlmProvider for SubprocessLlmProvider {
    async fn interpret_intent(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
    ) -> Result<InterpretedAction, CoreError> {
        self.invoke(screen_context, intent_hint, &SkillContext::default())
            .await
    }

    async fn interpret_intent_with_skills(
        &self,
        screen_context: &ScreenContext,
        intent_hint: &str,
        skill_ctx: &SkillContext,
    ) -> Result<InterpretedAction, CoreError> {
        self.invoke(screen_context, intent_hint, skill_ctx).await
    }

    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    fn is_external(&self) -> bool {
        true
    }
}

pub(super) fn codex_llm_runtime<'a>(
    provider: &'a SubprocessLlmProvider,
    prompt: &'a str,
) -> BoxFuture<'a, Result<String, CoreError>> {
    Box::pin(provider.run_codex(prompt))
}

pub(super) fn claude_llm_runtime<'a>(
    provider: &'a SubprocessLlmProvider,
    prompt: &'a str,
) -> BoxFuture<'a, Result<String, CoreError>> {
    Box::pin(provider.run_claude(prompt))
}

pub(super) fn gemini_llm_runtime<'a>(
    provider: &'a SubprocessLlmProvider,
    prompt: &'a str,
) -> BoxFuture<'a, Result<String, CoreError>> {
    Box::pin(provider.run_gemini(prompt))
}
