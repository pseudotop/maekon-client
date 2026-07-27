use async_trait::async_trait;
use maekon_core::config::AiProviderConfig;
use maekon_core::error::CoreError;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tempfile::tempdir;
use tokio::process::Command;

use super::{
    append_codex_reasoning_effort, append_model_flag, append_oneshot_flags, build_codex_ocr_prompt,
    build_path_based_ocr_prompt, classify_subprocess_error_with_redactions,
    default_llm_model_for_surface, default_ocr_model_for_surface, invocation_runtime_for_surface,
    is_gemini_json_flag_error, parse_ocr_output, provider_name_for_surface_id,
    write_prompt_and_collect_output, write_subprocess_ocr_image, BoxFuture, DetectedSubprocessCli,
    SubprocessKind, DEFAULT_CODEX_SUBPROCESS_MODEL, DEFAULT_SUBPROCESS_TIMEOUT_SECS,
    OCR_SCHEMA_JSON,
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
            .unwrap_or_else(|| DEFAULT_CODEX_SUBPROCESS_MODEL.to_string());
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
        append_codex_reasoning_effort(&mut child, &self.surface.surface_id);

        let child = child.spawn().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex OCR subprocess: {err}"),
        })?;

        // #6262: bounded stdin write + output collection with concurrent pipe
        // draining (avoids stdin/stdout deadlock; see parsing::write_prompt_and_collect_output).
        let output =
            write_prompt_and_collect_output(child, &prompt, "Codex OCR", self.timeout).await?;

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
        // Keep path-based OCR instructions out of argv because they can include
        // private local paths and nearby UI context.
        command.arg("-p").arg("-");
        append_oneshot_flags(&mut command, &self.surface.surface_id);
        command
            .arg("--output-format")
            .arg("json")
            .arg("--json-schema")
            .arg(OCR_SCHEMA_JSON)
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        let child = command.spawn().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Claude OCR subprocess: {err}"),
        })?;

        // #6262: bounded stdin write + output collection (see run_codex_ocr).
        let output =
            write_prompt_and_collect_output(child, &prompt, "Claude OCR", self.timeout).await?;

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
            Ok(output) if output.status.success() => output,
            // #6263: the `--output-format json` version incompatibility surfaces
            // as a NON-ZERO EXIT, not an `Err`, so the previous Err-only fallback
            // arm never fired. Classify the failed first attempt and retry once
            // without the json flag if it is the json-flag error.
            Ok(failed) => {
                let image_path_text = image_path.to_string_lossy();
                let candidate = classify_subprocess_error_with_redactions(
                    SubprocessKind::Ocr,
                    &self.surface.surface_id,
                    &String::from_utf8_lossy(&failed.stderr),
                    &[&prompt, image_path_text.as_ref()],
                );
                if is_gemini_json_flag_error(&candidate) {
                    self.run_gemini_command(workdir, &prompt, false).await?
                } else {
                    return Err(candidate);
                }
            }
            // Legacy path: flag error returned as an `Err` still retries.
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
        // Match LLM subprocess handling: stdin avoids leaking OCR prompts and
        // local image paths through process listings.
        command.arg("-p").arg("-");
        if prefer_json_output
            && subprocess_supports_json_output(&self.surface.surface_id).unwrap_or(false)
        {
            command.arg("--output-format").arg("json");
        }
        command
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        append_model_flag(&mut command, &self.surface.surface_id, &self.model);

        let child = command.spawn().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Gemini OCR subprocess: {err}"),
        })?;

        // #6262: bounded stdin write + output collection (see run_codex_ocr).
        write_prompt_and_collect_output(child, prompt, "Gemini OCR", self.timeout).await
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

    #[tokio::test]
    async fn gemini_ocr_invocation_delivers_prompt_via_stdin() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_ocr_cli(temp_dir.path());
        let image_path = temp_dir.path().join("screen.png");
        std::fs::write(&image_path, b"fake png").expect("image");
        let provider = SubprocessOcrProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );

        let raw = provider
            .run_gemini_ocr(temp_dir.path(), &image_path)
            .await
            .expect("fake Gemini CLI should receive OCR prompt via stdin");
        let results = parse_ocr_output(&raw).expect("Gemini OCR JSON envelope");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].text, "Search");
        assert_eq!(results[0].confidence, 0.91);
    }

    fn write_fake_gemini_ocr_cli(base_dir: &Path) -> PathBuf {
        let bin_dir = base_dir.join("Gemini CLI").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let source_path = bin_dir.join("fake_gemini_ocr.rs");
        let executable_path = bin_dir.join(if cfg!(windows) {
            "gemini.exe"
        } else {
            "gemini"
        });
        std::fs::write(
            &source_path,
            r##"
fn main() {
    use std::io::Read;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let prompt_slot = args
        .windows(2)
        .find(|window| window[0] == "-p")
        .map(|window| window[1].as_str());
    let has_json_output = args
        .windows(2)
        .any(|window| window[0] == "--output-format" && window[1] == "json");
    if prompt_slot != Some("-") {
        eprintln!("expected stdin sentinel '-' after -p");
        std::process::exit(44);
    }
    if !has_json_output {
        eprintln!("expected --output-format json");
        std::process::exit(42);
    }
    if args.iter().any(|arg| arg.contains("Read the local image file")) {
        eprintln!("raw OCR prompt leaked through argv");
        std::process::exit(45);
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).expect("stdin");
    if !prompt.contains("Read the local image file") {
        eprintln!("stdin prompt missing OCR instructions");
        std::process::exit(46);
    }
    println!(
        "{}",
        r#"{"results":[{"text":"Search","x":30,"y":40,"width":100,"height":30,"confidence":0.91}]}"#
    );
}
"##,
        )
        .expect("fake cli source");
        compile_fake_cli(&source_path, &executable_path);
        executable_path
    }

    fn compile_fake_cli(source_path: &Path, executable_path: &Path) {
        use std::process::Command as StdCommand;

        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = StdCommand::new(rustc)
            .arg(source_path)
            .arg("-o")
            .arg(executable_path)
            .status()
            .expect("compile fake cli");
        assert!(status.success(), "fake cli should compile");
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
    use std::io::Read;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has_json_output = args
        .windows(2)
        .any(|window| window[0] == "--output-format" && window[1] == "json");
    let has_schema = args.iter().any(|arg| arg == "--json-schema");
    let prompt_slot = args
        .windows(2)
        .find(|window| window[0] == "-p")
        .map(|window| window[1].as_str());
    if !has_json_output {
        eprintln!("expected --output-format json");
        std::process::exit(42);
    }
    if !has_schema {
        eprintln!("expected --json-schema");
        std::process::exit(43);
    }
    if prompt_slot != Some("-") {
        eprintln!("expected stdin sentinel '-' after -p");
        std::process::exit(44);
    }
    if args.iter().any(|arg| arg.contains("Read the local image file")) {
        eprintln!("raw OCR prompt leaked through argv");
        std::process::exit(45);
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).expect("stdin");
    if !prompt.contains("Read the local image file") {
        eprintln!("stdin prompt missing OCR instructions");
        std::process::exit(46);
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
found_stdin_sentinel=0
previous=
for arg in "$@"; do
  if [ "$previous" = "--output-format" ] && [ "$arg" = "json" ]; then
    found_output=1
  fi
  if [ "$previous" = "--json-schema" ]; then
    found_schema=1
  fi
  if [ "$previous" = "-p" ] && [ "$arg" = "-" ]; then
    found_stdin_sentinel=1
  fi
  case "$arg" in
    *"Read the local image file"*)
      echo "raw OCR prompt leaked through argv" >&2
      exit 45
      ;;
  esac
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
if [ "$found_stdin_sentinel" -ne 1 ]; then
  echo "expected stdin sentinel '-' after -p" >&2
  exit 44
fi
prompt="$(cat)"
case "$prompt" in
  *"Read the local image file"*) ;;
  *)
    echo "stdin prompt missing OCR instructions" >&2
    exit 46
    ;;
esac
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
