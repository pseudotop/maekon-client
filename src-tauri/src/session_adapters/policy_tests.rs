//! #12094 portable, inert CLI controls. No installed provider or account is used.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use futures::StreamExt;
use maekon_core::config::AiSessionConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    MessageRole, OutboundMessage, SessionConfig, SessionMessage, SessionTransport,
};
use maekon_core::ports::conversation_session::ConversationSession;

use super::claude_session::ClaudeSubprocessSession;
use super::subprocess_session::GenericSubprocessSession;
use crate::subprocess_provider::DetectedSubprocessCli;

pub(crate) const CODEX: &str = "provider_surface.openai.subprocess_cli";
pub(crate) const APP_SERVER: &str = "provider_surface.openai.codex_app_server";
pub(crate) const CLAUDE: &str = "provider_surface.anthropic.subprocess_cli";
pub(crate) const GEMINI: &str = "provider_surface.google.subprocess_cli";

pub(crate) fn config(surface: &str) -> SessionConfig {
    SessionConfig {
        transport: SessionTransport::Subprocess,
        surface_id: Some(surface.to_string()),
        model: Some("policy-fixture".to_string()),
        system_prompt: None,
        tools_enabled: true,
        cwd: None,
        sandbox_policy: None,
        approval_policy: None,
    }
}

pub(crate) fn message() -> SessionMessage {
    SessionMessage {
        screen_derived: false,
        role: MessageRole::User,
        content: "policy prompt".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    }
}

pub(crate) async fn send(session: &dyn ConversationSession) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut stream = session
            .send_message(&message())
            .await
            .expect("inert stream");
        let mut response = String::new();
        while let Some(event) = stream.next().await {
            match event.expect("inert event") {
                OutboundMessage::Text { content, .. } | OutboundMessage::Result { content, .. } => {
                    response.push_str(&content)
                }
                _ => {}
            }
        }
        assert!(response.contains("POLICY_OK"), "response: {response}");
    })
    .await
    .expect("bounded inert response");
}

pub(crate) struct InertCli {
    pub(crate) root: PathBuf,
    _temp: Option<tempfile::TempDir>,
}

impl InertCli {
    pub(crate) fn new(kind: &str, approval: Option<&str>) -> Self {
        // Compile once, then link into a private directory per case. The child
        // reads only synthetic files next to itself; no process-wide env edits.
        static BINARY: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        let (_, binary) = BINARY.get_or_init(|| {
            let temp = tempfile::tempdir().expect("compiler fixture directory");
            let source = temp.path().join("policy_cli.rs");
            let binary = temp
                .path()
                .join(format!("policy_cli{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&source, CLI_SOURCE).expect("inert source");
            let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
            let output = std::process::Command::new(rustc)
                .arg(&source)
                .arg("-o")
                .arg(&binary)
                .output()
                .expect("compile inert CLI");
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            (temp, binary)
        });
        // Keep the compiled executable immutable. Repeated copies can leave
        // a transient write reference during parallel Linux fork/exec (ETXTBSY).
        // Nest cases beside the binary so every hard link uses one filesystem.
        let temp = tempfile::Builder::new()
            .prefix("policy-case-")
            .tempdir_in(binary.parent().expect("compiled fixture directory"))
            .expect("invocation fixture directory");
        let root = temp.path().to_path_buf();
        std::fs::hard_link(binary, Self::executable_in(&root)).expect("link inert executable");
        std::fs::write(root.join("kind.txt"), kind).expect("synthetic provider kind");
        std::fs::write(root.join("approval.txt"), approval.unwrap_or("default"))
            .expect("synthetic expected policy");
        Self {
            root,
            _temp: Some(temp),
        }
    }

    pub(crate) fn attached(root: PathBuf) -> Self {
        Self { root, _temp: None }
    }

    fn with_hostile_startup_integration(self) -> Self {
        // Model inherited startup/discovery configuration using only a local
        // marker. This is a synthetic format, not a real provider integration.
        std::fs::write(
            self.root.join("startup-integration.conf"),
            "synthetic-local-startup-hook=enabled\n",
        )
        .expect("synthetic hostile startup configuration");
        self
    }

    fn executable_in(root: &Path) -> PathBuf {
        root.join(format!("codex{}", std::env::consts::EXE_SUFFIX))
    }

    pub(crate) fn surface(&self, surface: &str) -> DetectedSubprocessCli {
        DetectedSubprocessCli {
            surface_id: surface.to_string(),
            executable_path: Self::executable_in(&self.root),
        }
    }

    pub(crate) fn spawned(&self) -> bool {
        self.root.join("spawns.log").exists()
    }

    fn startup_integration_started(&self) -> bool {
        self.root.join("startup-integrations.log").exists()
    }

    pub(crate) fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).expect("inert child evidence")
    }
}

const CLI_SOURCE: &str = r##"
use std::io::{BufRead, Read, Write};

fn append(root: &std::path::Path, name: &str, text: &str) {
    let mut f = std::fs::OpenOptions::new().create(true).append(true)
        .open(root.join(name)).unwrap();
    writeln!(f, "{text}").unwrap();
}

fn main() {
    let exe = std::env::current_exe().unwrap();
    let root = exe.parent().unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    append(root, "spawns.log", &args.join(" "));
    std::fs::write(root.join("argv.txt"), args.join("\n")).unwrap();
    // Deliberately model an inherited startup hook before command dispatch.
    // Read only this executable's private fixture, never a user/provider config.
    let startup_config = root.join("startup-integration.conf");
    if startup_config.exists() {
        let config = std::fs::read_to_string(startup_config).unwrap();
        assert_eq!(config, "synthetic-local-startup-hook=enabled\n");
        append(root, "startup-integrations.log", "synthetic-local-startup-hook");
    }
    if args.iter().any(|a| a == "--version") {
        if root.join("fail-version").exists() { std::process::exit(7); }
        println!("codex-cli 0.145.0");
        return;
    }
    if args.first().map(String::as_str) == Some("login") {
        println!("Logged in using ChatGPT");
        return;
    }
    if args.first().map(String::as_str) == Some("app-server") {
        for line in std::io::stdin().lock().lines() {
            let line = line.unwrap();
            append(root, "requests.jsonl", &line);
            let Some((_, tail)) = line.split_once("\"id\":") else { continue };
            let id: String = tail.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
            if id.is_empty() { continue; }
            let result = if line.contains("\"initialize\"") {
                r#"{"userAgent":"codex-cli/0.145.0"}"#
            } else if line.contains("\"account/read\"") {
                r#"{"account":{"type":"chatgpt","email":"fixture@example.invalid","planType":"plus"},"requiresOpenaiAuth":false}"#
            } else if line.contains("\"thread/start\"") {
                r#"{"threadId":"policy_thread"}"#
            } else { "{}" };
            println!("{{\"id\":{id},\"result\":{result}}}");
            if line.contains("\"turn/start\"") {
                println!("{}", r#"{"method":"item/agentMessage/delta","params":{"text":"POLICY_OK"}}"#);
                println!("{}", r#"{"method":"turn/completed","params":{}}"#);
            }
            std::io::stdout().flush().unwrap();
        }
        return;
    }
    let kind = std::fs::read_to_string(root.join("kind.txt")).unwrap();
    let approval = std::fs::read_to_string(root.join("approval.txt")).unwrap();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1));
    assert_eq!(value("--model").map(String::as_str), Some("policy-fixture"));
    assert!(!args.iter().any(|a| a.contains("policy prompt")));
    let overrides: Vec<_> = args.iter().filter(|a| a.starts_with("approval_policy=")).collect();
    if kind == "codex" {
        assert_eq!(args.first().map(String::as_str), Some("exec"));
        assert_eq!(value("--sandbox").map(String::as_str), Some("read-only"));
        assert!(args.iter().any(|a| a == "-"));
        if approval == "default" { assert!(overrides.is_empty()); }
        else {
            let expected = format!("approval_policy=\"{approval}\"");
            assert_eq!(overrides, vec![&expected]);
            assert!(args.windows(2).any(|a| a[0] == "-c" && a[1] == expected));
        }
        assert!(!args.iter().any(|a| a == "-a" || a == "--ask-for-approval"));
    } else {
        assert_eq!(value("-p").map(String::as_str), Some("-"));
        assert!(overrides.is_empty());
        assert!(value("--sandbox").is_none());
        if kind == "claude" {
            assert!(args.iter().any(|a| a == "--tools="));
            assert!(!args.iter().any(|a| a == "--no-session-persistence"));
        }
    }
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).unwrap();
    std::fs::write(root.join("stdin.txt"), &prompt).unwrap();
    assert!(prompt.contains("policy prompt"));
    if kind == "codex" && args.iter().any(|a| a == "--json") {
        println!("{}", r#"{"type":"item.completed","item":{"type":"agent_message","id":"policy","text":"POLICY_OK"}}"#);
        println!("{}", r#"{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}"#);
    } else if kind == "claude" {
        println!("{}", r#"{"type":"result","result":"POLICY_OK","usage":{"input_tokens":1,"output_tokens":1}}"#);
    } else if kind == "gemini" {
        println!("{}", r#"{"text":"POLICY_OK","usage":{"input_tokens":1,"output_tokens":1}}"#);
    } else { println!("POLICY_OK"); }
}
"##;

fn construct(
    fixture: &InertCli,
    surface: &str,
    config: &SessionConfig,
) -> Result<Arc<dyn ConversationSession>, CoreError> {
    let settings = Arc::new(AiSessionConfig::default());
    if surface == CLAUDE {
        Ok(Arc::new(ClaudeSubprocessSession::new(
            fixture.surface(surface),
            config,
            settings,
            None,
        )?))
    } else {
        Ok(Arc::new(GenericSubprocessSession::new(
            fixture.surface(surface),
            config,
            settings,
            None,
        )?))
    }
}

#[tokio::test]
async fn subprocess_policy_direct_constructor_denial_has_live_spawn_control() {
    for (surface, kind) in [(CODEX, "codex"), (CLAUDE, "claude"), (GEMINI, "gemini")] {
        let fixture = InertCli::new(kind, None).with_hostile_startup_integration();
        let mut request = config(surface);
        request.tools_enabled = false;
        let result = construct(&fixture, surface, &request);
        let denied = matches!(&result, Err(CoreError::PolicyDenied { .. }));
        // Removal control: if a constructor wrongly admits false, run the SAME
        // inert prompt. Markers expose both the invocation and the synthetic
        // startup hook; neither marker represents a model or network call.
        if let Ok(session) = result {
            send(session.as_ref()).await;
        }
        let spawned = fixture.spawned();
        let integrated = fixture.startup_integration_started();
        assert!(
            !spawned && !integrated,
            "denied request spawned {kind}: spawn={spawned}, startup_integration={integrated}"
        );
        assert!(denied, "constructor must report policy.denied for {kind}");

        request.tools_enabled = true;
        let session = construct(&fixture, surface, &request).expect("enabled constructor");
        assert_eq!(session.info().turn_count, 0);
        send(session.as_ref()).await;
        assert!(fixture.spawned());
        assert!(fixture.startup_integration_started());
        assert_eq!(
            fixture.read("startup-integrations.log"),
            "synthetic-local-startup-hook\n"
        );
        assert_eq!(session.info().turn_count, 1);
        assert!(fixture.read("stdin.txt").contains("policy prompt"));
    }
}

#[tokio::test]
async fn subprocess_policy_codex_stream_preserves_explicit_approval_and_default() {
    for sandbox in [None, Some("read-only")] {
        for approval in [None, Some("never"), Some("on-request")] {
            let fixture = InertCli::new("codex", approval);
            let mut request = config(CODEX);
            request.sandbox_policy = sandbox.map(str::to_string);
            request.approval_policy = approval.map(str::to_string);
            let session = construct(&fixture, CODEX, &request).expect("verified Codex policy");
            send(session.as_ref()).await;
            assert!(fixture.read("argv.txt").contains("read-only"));
        }
    }
}

#[test]
fn subprocess_policy_rejects_unverified_explicit_requests_before_construction() {
    for surface in [CODEX, CLAUDE, GEMINI] {
        let fixture = InertCli::new("codex", None);
        for sandbox in [
            "",
            "workspace-write",
            "danger-full-access",
            "READ-ONLY",
            "invalid",
        ] {
            let mut request = config(surface);
            request.sandbox_policy = Some(sandbox.to_string());
            let Err(CoreError::PolicyDenied { message, .. }) =
                construct(&fixture, surface, &request)
            else {
                panic!("unsupported sandbox request must be denied for {surface}");
            };
            if surface == CODEX {
                assert!(
                    message.contains("read-only"),
                    "Codex sandbox denial must identify the required read-only policy: {message}"
                );
            }
        }
        for approval in ["", "untrusted", "on-failure", "NEVER", "invalid"] {
            let mut request = config(surface);
            request.approval_policy = Some(approval.to_string());
            assert!(matches!(
                construct(&fixture, surface, &request),
                Err(CoreError::PolicyDenied { .. })
            ));
        }
        if surface != CODEX {
            for (sandbox, approval) in [
                (Some("read-only"), None),
                (None, Some("never")),
                (None, Some("on-request")),
            ] {
                let mut request = config(surface);
                request.sandbox_policy = sandbox.map(str::to_string);
                request.approval_policy = approval.map(str::to_string);
                assert!(matches!(
                    construct(&fixture, surface, &request),
                    Err(CoreError::PolicyDenied { .. })
                ));
            }
        }
        assert!(!fixture.spawned());
    }
}
