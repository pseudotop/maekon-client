//! CLI subscription bridge artifacts.
//! Generates command/skill files so Codex / Claude Code / Gemini CLI can
//! consume Maekon exported context in provider subscription mode.

use oneshim_core::error::CoreError;
use std::path::{Path, PathBuf};

const ONESHIM_BRIDGE_NAME: &str = "oneshim-context";
const ONESHIM_MANAGED_BRIDGE_MARKER: &str = "ONESHIM-MANAGED-CLI-BRIDGE: oneshim-context";
const ONESHIM_CLI_BRIDGE_AUTOINSTALL_ENV: &str = "ONESHIM_CLI_BRIDGE_AUTOINSTALL";
const ONESHIM_CLI_BRIDGE_INCLUDE_USER_SCOPE_ENV: &str = "ONESHIM_CLI_BRIDGE_INCLUDE_USER_SCOPE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliClient {
    Codex,
    ClaudeCode,
    GeminiCli,
}

impl CliClient {
    fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
            Self::GeminiCli => "gemini-cli",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeScope {
    Project,
    User,
}

impl BridgeScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeArtifactKind {
    MarkdownCommand,
    GeminiTomlCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliBridgePlan {
    pub client: CliClient,
    pub scope: BridgeScope,
    pub artifact: BridgeArtifactKind,
    pub file_path: PathBuf,
}

impl CliBridgePlan {
    pub fn summary(&self) -> String {
        format!(
            "client={}, scope={}, artifact={:?}, file={}",
            self.client.as_str(),
            self.scope.as_str(),
            self.artifact,
            self.file_path.display()
        )
    }
}

#[derive(Debug, Default, Clone)]
pub struct BridgeSyncReport {
    pub written_files: Vec<PathBuf>,
    pub unchanged_files: Vec<PathBuf>,
    pub errors: Vec<String>,
}

impl BridgeSyncReport {
    pub fn is_successful(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Debug, Default, Clone)]
pub struct BridgeRevokeReport {
    pub removed_files: Vec<PathBuf>,
    pub skipped_files: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// Default path where Maekon exports context snapshots for CLI bridge consumers.
pub fn default_context_export_path(data_dir: &Path) -> PathBuf {
    data_dir.join("exports").join("oneshim-context.json")
}

/// Build default bridge plans for project scope and optional user scope.
pub fn default_bridge_plans(project_root: &Path, include_user_scope: bool) -> Vec<CliBridgePlan> {
    default_bridge_plans_with_home(
        project_root,
        include_user_scope,
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn default_bridge_plans_with_home(
    project_root: &Path,
    include_user_scope: bool,
    home_dir: Option<PathBuf>,
) -> Vec<CliBridgePlan> {
    let mut plans = vec![
        project_bridge_plan(CliClient::Codex, project_root),
        project_bridge_plan(CliClient::ClaudeCode, project_root),
        project_bridge_plan(CliClient::GeminiCli, project_root),
    ];

    if include_user_scope {
        if let Some(home) = home_dir {
            plans.push(user_bridge_plan(CliClient::Codex, &home));
            plans.push(user_bridge_plan(CliClient::ClaudeCode, &home));
            plans.push(user_bridge_plan(CliClient::GeminiCli, &home));
        }
    }

    plans
}

fn project_bridge_plan(client: CliClient, project_root: &Path) -> CliBridgePlan {
    let artifact = bridge_artifact_for(client);
    CliBridgePlan {
        client,
        scope: BridgeScope::Project,
        artifact,
        file_path: bridge_file_path(project_root, client),
    }
}

fn user_bridge_plan(client: CliClient, home_dir: &Path) -> CliBridgePlan {
    let artifact = bridge_artifact_for(client);
    CliBridgePlan {
        client,
        scope: BridgeScope::User,
        artifact,
        file_path: bridge_file_path(home_dir, client),
    }
}

fn bridge_artifact_for(client: CliClient) -> BridgeArtifactKind {
    match client {
        CliClient::Codex | CliClient::ClaudeCode => BridgeArtifactKind::MarkdownCommand,
        CliClient::GeminiCli => BridgeArtifactKind::GeminiTomlCommand,
    }
}

fn bridge_file_path(base_dir: &Path, client: CliClient) -> PathBuf {
    match client {
        CliClient::Codex => base_dir
            .join(".codex")
            .join("commands")
            .join(format!("{ONESHIM_BRIDGE_NAME}.md")),
        CliClient::ClaudeCode => base_dir
            .join(".claude")
            .join("commands")
            .join(format!("{ONESHIM_BRIDGE_NAME}.md")),
        CliClient::GeminiCli => base_dir
            .join(".gemini")
            .join("commands")
            .join(format!("{ONESHIM_BRIDGE_NAME}.toml")),
    }
}

/// Render bridge artifact content for one plan.
pub fn render_bridge_template(plan: &CliBridgePlan, context_export_path: &Path) -> String {
    match plan.artifact {
        BridgeArtifactKind::MarkdownCommand => {
            render_markdown_command_template(plan.client, context_export_path)
        }
        BridgeArtifactKind::GeminiTomlCommand => {
            render_gemini_command_template(context_export_path)
        }
    }
}

fn render_markdown_command_template(client: CliClient, context_export_path: &Path) -> String {
    let client_hint = match client {
        CliClient::Codex => {
            "Codex command mode: focus on concrete fixes, tests, and docs from Maekon context."
        }
        CliClient::ClaudeCode => {
            "Claude Code command mode: convert Maekon context into actionable engineering steps."
        }
        CliClient::GeminiCli => "Gemini CLI markdown command mode.",
    };

    format!(
        r#"<!-- {marker} -->
---
description: Use Maekon context export for actionable coding support.
---

{client_hint}

Read Maekon context export at `{context_path}`.

Execution contract:
1. If the file exists, summarize highest-impact action items first.
2. If the file is missing, state that Maekon context export is unavailable and continue with baseline analysis.
3. Prefer concrete outputs: failing tests, regression risks, implementation steps, and verification commands.
4. Keep recommendations provider-neutral across OpenAI, Anthropic, and Google workflows.
"#,
        marker = ONESHIM_MANAGED_BRIDGE_MARKER,
        client_hint = client_hint,
        context_path = context_export_path.display()
    )
}

fn render_gemini_command_template(context_export_path: &Path) -> String {
    format!(
        r#"# {marker}
description = "Use Maekon context export for actionable coding support"
prompt = """
Read Maekon context export at `{context_path}`.

Execution contract:
1. If the file exists, summarize highest-impact action items first.
2. If the file is missing, state that Maekon context export is unavailable and continue with baseline analysis.
3. Prefer concrete outputs: failing tests, regression risks, implementation steps, and verification commands.
4. Keep recommendations provider-neutral across OpenAI, Anthropic, and Google workflows.
"""
"#,
        marker = ONESHIM_MANAGED_BRIDGE_MARKER,
        context_path = context_export_path.display()
    )
}

/// Materialize a bridge artifact. Returns `true` when file content changed.
pub fn materialize_bridge_file(
    plan: &CliBridgePlan,
    context_export_path: &Path,
) -> Result<bool, CoreError> {
    if let Some(parent) = plan.file_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!(
                "브리지 디렉토리 create failure ({}): {}",
                parent.display(),
                e
            ),
        })?;
    }

    let content = render_bridge_template(plan, context_export_path);

    if let Ok(existing) = std::fs::read_to_string(&plan.file_path) {
        if existing == content {
            return Ok(false);
        }
    }

    std::fs::write(&plan.file_path, content).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!(
            "브리지 file create failure ({}): {}",
            plan.file_path.display(),
            e
        ),
    })?;

    Ok(true)
}

/// Sync bridge artifacts to disk.
pub fn sync_bridge_files(
    project_root: &Path,
    context_export_path: &Path,
    include_user_scope: bool,
) -> BridgeSyncReport {
    let mut report = BridgeSyncReport::default();

    for plan in default_bridge_plans(project_root, include_user_scope) {
        match materialize_bridge_file(&plan, context_export_path) {
            Ok(true) => report.written_files.push(plan.file_path),
            Ok(false) => report.unchanged_files.push(plan.file_path),
            Err(err) => {
                report
                    .errors
                    .push(format!("{} ({})", plan.summary(), err.to_string().trim()));
            }
        }
    }

    report
}

pub fn revoke_bridge_files(project_root: &Path, include_user_scope: bool) -> BridgeRevokeReport {
    let mut report = BridgeRevokeReport::default();

    for plan in default_bridge_plans(project_root, include_user_scope) {
        match revoke_bridge_file(&plan) {
            Ok(BridgeFileRevokeOutcome::Removed) => report.removed_files.push(plan.file_path),
            Ok(BridgeFileRevokeOutcome::Skipped) => report.skipped_files.push(plan.file_path),
            Err(err) => {
                report
                    .errors
                    .push(format!("{} ({})", plan.summary(), err.to_string().trim()));
            }
        }
    }

    report
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

/// Whether bridge files should be auto-installed by runtime.
pub fn should_autoinstall_bridge_files() -> bool {
    env_flag_enabled(ONESHIM_CLI_BRIDGE_AUTOINSTALL_ENV)
}

/// Whether user-scope bridge files (`~/.codex`, `~/.claude`, `~/.gemini`) are included.
pub fn should_include_user_scope() -> bool {
    env_flag_enabled(ONESHIM_CLI_BRIDGE_INCLUDE_USER_SCOPE_ENV)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BridgeFileRevokeOutcome {
    Removed,
    Skipped,
}

fn revoke_bridge_file(plan: &CliBridgePlan) -> Result<BridgeFileRevokeOutcome, CoreError> {
    if !plan.file_path.exists() {
        return Ok(BridgeFileRevokeOutcome::Skipped);
    }

    let contents = std::fs::read_to_string(&plan.file_path).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!(
            "bridge file read failure ({}): {}",
            plan.file_path.display(),
            e
        ),
    })?;

    if !is_managed_bridge_content(&contents) {
        return Ok(BridgeFileRevokeOutcome::Skipped);
    }

    std::fs::remove_file(&plan.file_path).map_err(|e| CoreError::Internal {
        code: oneshim_core::error_codes::InternalCode::Generic,
        message: format!(
            "bridge file remove failure ({}): {}",
            plan.file_path.display(),
            e
        ),
    })?;

    Ok(BridgeFileRevokeOutcome::Removed)
}

fn is_managed_bridge_content(contents: &str) -> bool {
    if contents.contains(ONESHIM_MANAGED_BRIDGE_MARKER) {
        return true;
    }

    // Keep recognizing pre-Maekon generated bridge files so revoke can clean them up.
    (contents.contains("Use ONESHIM context export for actionable coding support")
        && contents.contains("Read ONESHIM context export at"))
        || (contents.contains("Use Maekon context export for actionable coding support")
            && contents.contains("Read Maekon context export at"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_context_export_path_uses_exports_dir() {
        let data_dir = PathBuf::from("/tmp/oneshim-data");
        let path = default_context_export_path(&data_dir);
        assert!(path
            .to_string_lossy()
            .contains("/tmp/oneshim-data/exports/oneshim-context.json"));
    }

    #[test]
    fn default_bridge_plans_include_project_plans_for_all_clients() {
        let project = PathBuf::from("/tmp/oneshim-project");
        let plans = default_bridge_plans_with_home(&project, false, None);

        assert!(plans.iter().any(|plan| {
            plan.client == CliClient::Codex
                && plan.scope == BridgeScope::Project
                && plan
                    .file_path
                    .to_string_lossy()
                    .contains(".codex/commands/oneshim-context.md")
        }));

        assert!(plans.iter().any(|plan| {
            plan.client == CliClient::ClaudeCode
                && plan.scope == BridgeScope::Project
                && plan
                    .file_path
                    .to_string_lossy()
                    .contains(".claude/commands/oneshim-context.md")
        }));

        assert!(plans.iter().any(|plan| {
            plan.client == CliClient::GeminiCli
                && plan.scope == BridgeScope::Project
                && plan
                    .file_path
                    .to_string_lossy()
                    .contains(".gemini/commands/oneshim-context.toml")
        }));
    }

    #[test]
    fn default_bridge_plans_can_include_user_scope() {
        let project = PathBuf::from("/tmp/oneshim-project");
        let home = PathBuf::from("/tmp/fake-home");
        let plans = default_bridge_plans_with_home(&project, true, Some(home));

        let user_count = plans
            .iter()
            .filter(|plan| plan.scope == BridgeScope::User)
            .count();
        assert_eq!(user_count, 3);
    }

    #[test]
    fn render_markdown_template_includes_context_path() {
        let plan = CliBridgePlan {
            client: CliClient::Codex,
            scope: BridgeScope::Project,
            artifact: BridgeArtifactKind::MarkdownCommand,
            file_path: PathBuf::from("/tmp/.codex/commands/oneshim-context.md"),
        };
        let context_path = PathBuf::from("/tmp/context/export.json");
        let rendered = render_bridge_template(&plan, &context_path);

        assert!(rendered.contains(ONESHIM_MANAGED_BRIDGE_MARKER));
        assert!(rendered.contains("description:"));
        assert!(rendered.contains("/tmp/context/export.json"));
        assert!(rendered.contains("Codex command mode"));
    }

    #[test]
    fn render_gemini_template_is_toml() {
        let plan = CliBridgePlan {
            client: CliClient::GeminiCli,
            scope: BridgeScope::Project,
            artifact: BridgeArtifactKind::GeminiTomlCommand,
            file_path: PathBuf::from("/tmp/.gemini/commands/oneshim-context.toml"),
        };
        let context_path = PathBuf::from("/tmp/context/export.json");
        let rendered = render_bridge_template(&plan, &context_path);

        assert!(rendered.contains(ONESHIM_MANAGED_BRIDGE_MARKER));
        assert!(rendered.contains("description ="));
        assert!(rendered.contains("prompt ="));
        assert!(rendered.contains("/tmp/context/export.json"));
    }

    #[test]
    fn materialize_bridge_file_is_idempotent() {
        let temp = TempDir::new().expect("temp dir");
        let plan = CliBridgePlan {
            client: CliClient::Codex,
            scope: BridgeScope::Project,
            artifact: BridgeArtifactKind::MarkdownCommand,
            file_path: temp.path().join(".codex/commands/oneshim-context.md"),
        };
        let context_path = temp.path().join("context.json");

        let first = materialize_bridge_file(&plan, &context_path).expect("write 1");
        let second = materialize_bridge_file(&plan, &context_path).expect("write 2");

        assert!(first);
        assert!(!second);
    }

    #[test]
    fn sync_bridge_files_writes_three_project_files() {
        let temp = TempDir::new().expect("temp dir");
        let report = sync_bridge_files(temp.path(), &temp.path().join("ctx.json"), false);

        assert!(report.errors.is_empty());
        assert_eq!(report.written_files.len(), 3);
    }

    #[test]
    fn revoke_bridge_files_removes_managed_project_files() {
        let temp = TempDir::new().expect("temp dir");
        let context_path = temp.path().join("ctx.json");
        let sync_report = sync_bridge_files(temp.path(), &context_path, false);
        assert_eq!(sync_report.written_files.len(), 3);

        let revoke_report = revoke_bridge_files(temp.path(), false);
        assert!(revoke_report.errors.is_empty());
        assert_eq!(revoke_report.removed_files.len(), 3);
    }

    #[test]
    fn revoke_bridge_files_skips_non_managed_content() {
        let temp = TempDir::new().expect("temp dir");
        let plan = CliBridgePlan {
            client: CliClient::Codex,
            scope: BridgeScope::Project,
            artifact: BridgeArtifactKind::MarkdownCommand,
            file_path: temp.path().join(".codex/commands/oneshim-context.md"),
        };
        std::fs::create_dir_all(plan.file_path.parent().unwrap()).unwrap();
        std::fs::write(&plan.file_path, "user managed command").unwrap();

        let report = revoke_bridge_files(temp.path(), false);
        assert!(report.errors.is_empty());
        assert_eq!(report.removed_files.len(), 0);
        assert_eq!(report.skipped_files.len(), 3);
        assert!(plan.file_path.exists());
    }
}
