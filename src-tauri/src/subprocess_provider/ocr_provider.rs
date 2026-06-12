use async_trait::async_trait;
use maekon_core::config::AiProviderConfig;
use maekon_core::error::CoreError;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tempfile::tempdir;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use super::{
    append_model_flag, append_oneshot_flags, build_codex_ocr_prompt, build_path_based_ocr_prompt,
    classify_subprocess_error_with_redactions, default_llm_model_for_surface,
    default_ocr_model_for_surface, invocation_runtime_for_surface, is_gemini_json_flag_error,
    parse_ocr_output, provider_name_for_surface_id, write_subprocess_ocr_image, BoxFuture,
    DetectedSubprocessCli, SubprocessKind, DEFAULT_SUBPROCESS_TIMEOUT_SECS, OCR_SCHEMA_JSON,
};
use maekon_api_contracts::provider_specs::subprocess_supports_json_output;

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
            code: maekon_core::error_codes::InternalCode::Generic,
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
        // F-RC-10: use tokio::fs::write in async context
        tokio::fs::write(&schema_path, OCR_SCHEMA_JSON)
            .await
            .map_err(|err| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex OCR subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
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
                code: maekon_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            let image_path_text = image_path.to_string_lossy();
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Ocr,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[&prompt, image_path_text.as_ref()],
            ));
        }

        // F-RC-10: use tokio::fs::read_to_string in async context
        if let Ok(rendered) = tokio::fs::read_to_string(&output_path).await {
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
            .arg("json")
            .arg("--json-schema")
            .arg(OCR_SCHEMA_JSON)
            .arg(&prompt)
            .current_dir(workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        let output = timeout(self.timeout, command.output())
            .await
            .map_err(|_| CoreError::RequestTimeout {
                code: maekon_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            let image_path_text = image_path.to_string_lossy();
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Ocr,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[&prompt, image_path_text.as_ref()],
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
            let image_path_text = image_path.to_string_lossy();
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Ocr,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[&prompt, image_path_text.as_ref()],
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
                code: maekon_core::error_codes::NetworkCode::Timeout,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[tokio::test]
    async fn claude_ocr_invocation_uses_json_output_envelope() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_claude_ocr_cli(temp_dir.path());
        let image_path = temp_dir.path().join("screen.png");
        std::fs::write(&image_path, b"fake png").expect("image");
        let provider = SubprocessOcrProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );

        let raw = provider
            .run_claude_ocr(temp_dir.path(), &image_path)
            .await
            .expect("fake Claude CLI should receive json OCR args");
        let results = parse_ocr_output(&raw).expect("Claude OCR JSON envelope");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "Save");
        assert_eq!(results[0].confidence, 0.96);
    }

    #[cfg(windows)]
    fn write_fake_claude_ocr_cli(base_dir: &Path) -> PathBuf {
        use std::process::Command as StdCommand;

        let bin_dir = base_dir.join("Claude Code").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let source_path = bin_dir.join("fake_claude.rs");
        let executable_path = bin_dir.join("claude.exe");
        std::fs::write(
            &source_path,
            r##"
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has_json_output = args
        .windows(2)
        .any(|window| window[0] == "--output-format" && window[1] == "json");
    let has_schema = args.iter().any(|arg| arg == "--json-schema");
    if !has_json_output {
        eprintln!("expected --output-format json");
        std::process::exit(42);
    }
    if !has_schema {
        eprintln!("expected --json-schema");
        std::process::exit(43);
    }
    println!(
        "{}",
        r#"{"type":"result","result":"Output provided.","structured_output":{"results":[{"text":"Save","x":10,"y":20,"width":80,"height":24,"confidence":0.96}]}}"#
    );
}
"##,
        )
        .expect("fake cli source");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = StdCommand::new(rustc)
            .arg(&source_path)
            .arg("-o")
            .arg(&executable_path)
            .status()
            .expect("compile fake cli");
        assert!(status.success(), "fake cli should compile");
        executable_path
    }

    #[cfg(unix)]
    fn write_fake_claude_ocr_cli(base_dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let bin_dir = base_dir.join("Claude Code").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let script_path = bin_dir.join("claude");
        std::fs::write(
            &script_path,
            r#"#!/bin/sh
found_output=0
found_schema=0
previous=
for arg in "$@"; do
  if [ "$previous" = "--output-format" ] && [ "$arg" = "json" ]; then
    found_output=1
  fi
  if [ "$previous" = "--json-schema" ]; then
    found_schema=1
  fi
  previous="$arg"
done
if [ "$found_output" -ne 1 ]; then
  echo "expected --output-format json" >&2
  exit 42
fi
if [ "$found_schema" -ne 1 ]; then
  echo "expected --json-schema" >&2
  exit 43
fi
printf '%s\n' '{"type":"result","result":"Output provided.","structured_output":{"results":[{"text":"Save","x":10,"y":20,"width":80,"height":24,"confidence":0.96}]}}'
"#,
        )
        .expect("fake cli script");
        let mut permissions = std::fs::metadata(&script_path)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script_path, permissions).expect("chmod");
        script_path
    }
}
