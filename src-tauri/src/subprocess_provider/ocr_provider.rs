use async_trait::async_trait;
use oneshim_core::config::AiProviderConfig;
use oneshim_core::error::CoreError;
use oneshim_core::ports::ocr_provider::{OcrProvider, OcrResult};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use super::{
    append_model_flag, append_oneshot_flags, build_codex_ocr_prompt, build_path_based_ocr_prompt,
    classify_subprocess_error, default_llm_model_for_surface, default_ocr_model_for_surface,
    invocation_runtime_for_surface, is_gemini_json_flag_error, parse_ocr_output,
    provider_name_for_surface_id, write_subprocess_ocr_image, BoxFuture, DetectedSubprocessCli,
    SubprocessKind, DEFAULT_SUBPROCESS_TIMEOUT_SECS, OCR_SCHEMA_JSON,
};
use oneshim_api_contracts::provider_specs::subprocess_supports_json_output;

#[derive(Debug, Clone)]
pub struct SubprocessOcrProvider {
    pub(super) surface: DetectedSubprocessCli,
    pub(super) provider_name: String,
    pub(super) model: String,
    pub(super) timeout: Duration,
}

impl SubprocessOcrProvider {
    pub fn new(surface: DetectedSubprocessCli, config: &AiProviderConfig) -> Self {
        let model = config
            .ocr_api
            .as_ref()
            .and_then(|endpoint| endpoint.model.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                default_ocr_model_for_surface(&surface.surface_id)
                    .ok()
                    .flatten()
            })
            .or_else(|| {
                default_llm_model_for_surface(&surface.surface_id)
                    .ok()
                    .flatten()
            })
            .unwrap_or_else(|| "gpt-5.4".to_string());
        let timeout_secs = config
            .ocr_api
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

    async fn invoke(&self, image: &[u8], image_format: &str) -> Result<Vec<OcrResult>, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create subprocess OCR tempdir: {err}"),
        })?;
        let image_path = write_subprocess_ocr_image(temp_dir.path(), image, image_format)?;
        let runtime = invocation_runtime_for_surface(&self.surface.surface_id)?;
        let raw = (runtime.ocr_invoke)(self, temp_dir.path(), &image_path).await?;

        parse_ocr_output(&raw)
    }

    pub(super) async fn run_codex_ocr(
        &self,
        workdir: &Path,
        image_path: &Path,
    ) -> Result<String, CoreError> {
        let schema_path = workdir.join("ocr.schema.json");
        let output_path = workdir.join("codex-ocr-output.json");
        std::fs::write(&schema_path, OCR_SCHEMA_JSON).map_err(|err| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to write Codex OCR schema: {err}"),
        })?;

        let prompt = build_codex_ocr_prompt(&self.model);
        let mut child = Command::new(&self.surface.executable_path);
        child
            .arg("exec")
            .arg("--sandbox")
            .arg("read-only")
            .arg("--skip-git-repo-check")
            .arg("--color")
            .arg("never")
            .arg("-C")
            .arg(workdir)
            .arg("--image")
            .arg(image_path)
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
            message: format!("Failed to spawn Codex OCR subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: "Failed to open stdin for Codex OCR subprocess".to_string(),
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
                SubprocessKind::Ocr,
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

    pub(super) async fn run_claude_ocr(
        &self,
        workdir: &Path,
        image_path: &Path,
    ) -> Result<String, CoreError> {
        let prompt = build_path_based_ocr_prompt(image_path, &self.model);
        let mut command = Command::new(&self.surface.executable_path);
        command.arg("-p");
        append_oneshot_flags(&mut command, &self.surface.surface_id);
        command
            .arg("--output-format")
            .arg("text")
            .arg("--json-schema")
            .arg(OCR_SCHEMA_JSON)
            .arg(prompt)
            .current_dir(workdir)
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
                SubprocessKind::Ocr,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub(super) async fn run_gemini_ocr(
        &self,
        workdir: &Path,
        image_path: &Path,
    ) -> Result<String, CoreError> {
        let prompt = build_path_based_ocr_prompt(image_path, &self.model);
        let output = match self.run_gemini_command(workdir, &prompt, true).await {
            Ok(output) => output,
            Err(error) if is_gemini_json_flag_error(&error) => {
                self.run_gemini_command(workdir, &prompt, false).await?
            }
            Err(error) => return Err(error),
        };

        if !output.status.success() {
            return Err(classify_subprocess_error(
                SubprocessKind::Ocr,
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
impl OcrProvider for SubprocessOcrProvider {
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        self.invoke(image, image_format).await
    }

    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    fn is_external(&self) -> bool {
        true
    }
}

pub(super) fn codex_ocr_runtime<'a>(
    provider: &'a SubprocessOcrProvider,
    workdir: &'a Path,
    image_path: &'a Path,
) -> BoxFuture<'a, Result<String, CoreError>> {
    Box::pin(provider.run_codex_ocr(workdir, image_path))
}

pub(super) fn claude_ocr_runtime<'a>(
    provider: &'a SubprocessOcrProvider,
    workdir: &'a Path,
    image_path: &'a Path,
) -> BoxFuture<'a, Result<String, CoreError>> {
    Box::pin(provider.run_claude_ocr(workdir, image_path))
}

pub(super) fn gemini_ocr_runtime<'a>(
    provider: &'a SubprocessOcrProvider,
    workdir: &'a Path,
    image_path: &'a Path,
) -> BoxFuture<'a, Result<String, CoreError>> {
    Box::pin(provider.run_gemini_ocr(workdir, image_path))
}
