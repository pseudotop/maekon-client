use async_trait::async_trait;
use maekon_core::config::AiProviderConfig;
use maekon_core::error::CoreError;
use maekon_core::ports::llm_provider::{
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
    append_model_flag, append_oneshot_flags, build_intent_prompt,
    classify_subprocess_error_with_redactions, default_llm_model_for_surface,
    invocation_runtime_for_surface, is_gemini_json_flag_error, parse_interpreted_action_output,
    provider_name_for_surface_id, BoxFuture, DetectedSubprocessCli, SubprocessKind,
    ACTION_SCHEMA_JSON, DEFAULT_SUBPROCESS_TIMEOUT_SECS,
};
use maekon_api_contracts::provider_specs::subprocess_supports_json_output;

const MAX_ARGV_PROMPT_BYTES: usize = 24 * 1024;

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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Codex subprocess tempdir: {err}"),
        })?;
        let schema_path = temp_dir.path().join("action.schema.json");
        let output_path = temp_dir.path().join("codex-output.json");
        // F-RC-10: use tokio::fs::write in async context
        tokio::fs::write(&schema_path, ACTION_SCHEMA_JSON)
            .await
            .map_err(|err| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
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
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to spawn Codex CLI subprocess: {err}"),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
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
                code: maekon_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[prompt],
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

    pub(super) async fn run_claude(&self, prompt: &str) -> Result<String, CoreError> {
        ensure_argv_prompt_safe(&self.surface.surface_id, prompt)?;
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("Failed to create Claude subprocess tempdir: {err}"),
        })?;

        let mut command = Command::new(&self.surface.executable_path);
        command.arg("-p");
        append_oneshot_flags(&mut command, &self.surface.surface_id);
        command
            .arg("--output-format")
            .arg("json")
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
                code: maekon_core::error_codes::NetworkCode::Timeout,
                timeout_ms: self.timeout.as_millis() as u64,
            })?
            .map_err(CoreError::Io)?;

        if !output.status.success() {
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[prompt],
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub(super) async fn run_gemini(&self, prompt: &str) -> Result<String, CoreError> {
        let temp_dir = tempdir().map_err(|err| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
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
            return Err(classify_subprocess_error_with_redactions(
                SubprocessKind::Llm,
                &self.surface.surface_id,
                &String::from_utf8_lossy(&output.stderr),
                &[prompt],
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
        ensure_argv_prompt_safe(&self.surface.surface_id, prompt)?;
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

fn ensure_argv_prompt_safe(surface_id: &str, prompt: &str) -> Result<(), CoreError> {
    if prompt.len() <= MAX_ARGV_PROMPT_BYTES {
        return Ok(());
    }

    Err(CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Invalid,
        message: format!(
            "Provider CLI surface '{surface_id}' uses argv prompt transport; prompt payload is too large for the Windows-safe command-line budget."
        ),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[tokio::test]
    async fn claude_llm_invocation_uses_json_output_envelope() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_claude_llm_cli(temp_dir.path());
        let provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.anthropic.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );

        let raw = provider
            .run_claude("Click the Save button")
            .await
            .expect("fake Claude CLI should receive json output args");
        let action = parse_interpreted_action_output(&raw).expect("Claude JSON envelope");

        assert_eq!(action.target_text.as_deref(), Some("Save"));
        assert_eq!(action.action_type, "click");
        assert_eq!(action.confidence, 0.97);
    }

    #[tokio::test]
    async fn codex_llm_invocation_uses_stdin_and_output_file_contract() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_codex_llm_cli(temp_dir.path());
        let provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.openai.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );

        let raw = provider
            .run_codex("Codex prompt with spaces && shell metacharacters | should stay stdin")
            .await
            .expect("fake Codex CLI should write output file");
        let action = parse_interpreted_action_output(&raw).expect("Codex direct JSON output");

        assert_eq!(action.target_text.as_deref(), Some("Codex"));
        assert_eq!(action.action_type, "click");
        assert_eq!(action.confidence, 0.93);
    }

    #[tokio::test]
    async fn gemini_llm_invocation_preserves_prompt_argv_and_raw_json_stdout() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_llm_cli(temp_dir.path());
        let provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );

        let prompt = "Gemini prompt with spaces && shell metacharacters | should stay one argv";
        let raw = provider
            .run_gemini(prompt)
            .await
            .expect("fake Gemini CLI should preserve prompt argv");
        let action = parse_interpreted_action_output(&raw).expect("Gemini raw JSON output");

        assert_eq!(action.target_text.as_deref(), Some("Gemini"));
        assert_eq!(action.action_type, "click");
        assert_eq!(action.confidence, 0.94);
    }

    #[tokio::test]
    async fn gemini_llm_non_zero_exit_maps_to_analysis_error() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_llm_cli(temp_dir.path());
        let provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );

        let err = provider
            .run_gemini("NONZERO")
            .await
            .expect_err("fake Gemini CLI should fail");

        assert_eq!(err.code(), "provider.analysis_failed");
    }

    #[tokio::test]
    async fn gemini_llm_timeout_maps_to_request_timeout() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_llm_cli(temp_dir.path());
        let mut provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );
        provider.timeout = Duration::from_millis(50);

        let err = provider
            .run_gemini("TIMEOUT")
            .await
            .expect_err("fake Gemini CLI should time out");

        assert_eq!(err.code(), "network.timeout");
    }

    #[tokio::test]
    async fn gemini_llm_rejects_argv_prompt_that_exceeds_windows_safe_limit() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_llm_cli(temp_dir.path());
        let provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );
        let prompt = "x".repeat(MAX_ARGV_PROMPT_BYTES + 1);

        let err = provider
            .run_gemini(&prompt)
            .await
            .expect_err("long argv prompt should be rejected before spawn");

        assert_eq!(err.code(), "config.invalid");
        assert!(!err.to_string().contains(&prompt[..128]));
    }

    #[tokio::test]
    async fn gemini_llm_redacts_raw_prompt_echoed_by_provider_stderr() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_gemini_llm_cli(temp_dir.path());
        let provider = SubprocessLlmProvider::new(
            DetectedSubprocessCli {
                surface_id: "provider_surface.google.subprocess_cli".to_string(),
                executable_path,
            },
            &AiProviderConfig::default(),
        );
        let prompt = "ECHO_ERROR private prompt for alice@example.com with token sk-secret";

        let err = provider
            .run_gemini(prompt)
            .await
            .expect_err("fake provider should echo prompt to stderr");
        let message = err.to_string();

        assert_eq!(err.code(), "provider.analysis_failed");
        assert!(!message.contains(prompt));
        assert!(!message.contains("alice@example.com"));
        assert!(!message.contains("sk-secret"));
    }

    fn write_fake_codex_llm_cli(base_dir: &Path) -> PathBuf {
        let bin_dir = base_dir.join("Codex CLI").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let source_path = bin_dir.join("fake_codex.rs");
        let executable_path = bin_dir.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        std::fs::write(
            &source_path,
            r##"
use std::io::Read;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|value| value.as_str()) != Some("exec") {
        eprintln!("expected exec subcommand");
        std::process::exit(81);
    }
    let output_path = args
        .windows(2)
        .find_map(|window| {
            if window[0] == "--output-last-message" {
                Some(PathBuf::from(&window[1]))
            } else {
                None
            }
        })
        .expect("output path");
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).expect("stdin");
    if !prompt.contains("Codex prompt with spaces && shell metacharacters | should stay stdin") {
        eprintln!("stdin prompt was not preserved");
        std::process::exit(82);
    }
    std::fs::write(
        output_path,
        r#"{"target_text":"Codex","target_role":"button","action_type":"click","confidence":0.93}"#,
    )
    .expect("write output");
}
"##,
        )
        .expect("fake cli source");
        compile_fake_cli(&source_path, &executable_path);
        executable_path
    }

    fn write_fake_gemini_llm_cli(base_dir: &Path) -> PathBuf {
        let bin_dir = base_dir.join("Gemini CLI").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let source_path = bin_dir.join("fake_gemini.rs");
        let executable_path = bin_dir.join(if cfg!(windows) {
            "gemini.exe"
        } else {
            "gemini"
        });
        std::fs::write(
            &source_path,
            r##"
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let prompt_index = args
        .iter()
        .position(|arg| arg == "-p")
        .and_then(|index| index.checked_add(1))
        .expect("prompt index");
    let prompt = args.get(prompt_index).expect("prompt");
    match prompt.as_str() {
        "NONZERO" => {
            eprintln!("model not found");
            std::process::exit(7);
        }
        "TIMEOUT" => {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        value if value.starts_with("ECHO_ERROR ") => {
            eprintln!("provider echoed prompt: {value}");
            std::process::exit(9);
        }
        expected if expected == "Gemini prompt with spaces && shell metacharacters | should stay one argv" => {
            println!(
                "{}",
                r#"{"target_text":"Gemini","target_role":"button","action_type":"click","confidence":0.94}"#
            );
        }
        _ => {
            eprintln!("prompt argv was not preserved");
            std::process::exit(8);
        }
    }
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
    fn write_fake_claude_llm_cli(base_dir: &Path) -> PathBuf {
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
        r#"{"type":"result","result":"Output provided.","structured_output":{"target_text":"Save","target_role":"button","action_type":"click","confidence":0.97}}"#
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
    fn write_fake_claude_llm_cli(base_dir: &Path) -> PathBuf {
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
printf '%s\n' '{"type":"result","result":"Output provided.","structured_output":{"target_text":"Save","target_role":"button","action_type":"click","confidence":0.97}}'
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
