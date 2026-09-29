//! #12334: exercise the analysis port through an inert native CLI on every host.

use super::super::{DetectedSubprocessCli, ACTION_SCHEMA_JSON};
use super::{SubprocessAnalysisProvider, SUGGESTION_SCHEMA_JSON};
use maekon_core::config::AiProviderConfig;
use maekon_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};
use maekon_core::ports::analysis_provider::AnalysisProvider;
use std::path::{Path, PathBuf};

const TRUSTED_PROMPT: &str = "Analyze only the supplied inert activity fixture.";
const NONEMPTY_CONTEXT: &str =
    r#"{"fixture":"schema-fixture-nonempty","activity":"Editor && notes | review"}"#;
const EMPTY_CONTEXT: &str = r#"{"fixture":"schema-fixture-empty","activity":[]}"#;

#[tokio::test]
async fn codex_analysis_invocation_supplies_suggestion_schema_file() {
    assert_analysis_invocation("provider_surface.openai.subprocess_cli").await;
}

#[tokio::test]
async fn claude_analysis_invocation_supplies_suggestion_schema_argument() {
    assert_analysis_invocation("provider_surface.anthropic.subprocess_cli").await;
}

async fn assert_analysis_invocation(surface_id: &str) {
    let temp_dir = tempfile::tempdir().expect("analysis fixture directory");
    let executable_path = write_fake_analysis_cli(temp_dir.path());
    let provider = SubprocessAnalysisProvider::new(
        DetectedSubprocessCli {
            surface_id: surface_id.to_string(),
            executable_path,
        },
        &AiProviderConfig::default(),
    );

    let suggestions = provider
        .analyze(NONEMPTY_CONTEXT, TRUSTED_PROMPT)
        .await
        .expect("analysis invocation must deliver the suggestion schema and stdin context");
    assert_eq!(suggestions.len(), 1);
    let suggestion = &suggestions[0];
    assert_eq!(
        suggestion.suggestion_type,
        SuggestionType::WorkflowOptimization
    );
    assert_eq!(suggestion.content, "Review the queued notes together.");
    assert_eq!(suggestion.confidence_score, 0.91);
    assert_eq!(suggestion.priority, Priority::High);
    assert_eq!(suggestion.source, SuggestionSource::LlmLocal);
    assert_eq!(
        suggestion.reasoning.as_deref(),
        Some("The fixture contains three related notes.")
    );

    let empty = provider
        .analyze(EMPTY_CONTEXT, TRUSTED_PROMPT)
        .await
        .expect("a valid empty CLI response must remain a successful analysis");
    assert!(empty.is_empty());

    // The same executable must reject the original action-schema wiring for
    // both responses; otherwise the fixture could conceal this regression.
    let mut wrong_schema = provider.clone();
    wrong_schema.runner = wrong_schema.runner.with_schema(ACTION_SCHEMA_JSON);
    for context in [NONEMPTY_CONTEXT, EMPTY_CONTEXT] {
        let error = wrong_schema
            .analyze(context, TRUSTED_PROMPT)
            .await
            .expect_err("the inert CLI must reject an action schema on the analysis port");
        assert_eq!(error.code(), "provider.analysis_failed");
        assert!(error
            .to_string()
            .contains("fixture suggestion schema mismatch"));
    }
}

fn write_fake_analysis_cli(base_dir: &Path) -> PathBuf {
    let bin_dir = base_dir.join("Analysis CLI").join("bin");
    std::fs::create_dir_all(&bin_dir).expect("fake analysis CLI directory");
    let source_path = bin_dir.join("fake_analysis.rs");
    let executable_path = bin_dir.join(if cfg!(windows) {
        "fake-analysis.exe"
    } else {
        "fake-analysis"
    });
    let source = format!(
        "const EXPECTED_SCHEMA: &str = {SUGGESTION_SCHEMA_JSON:?};\n\
         const TRUSTED_PROMPT: &str = {TRUSTED_PROMPT:?};\n\
         const NONEMPTY_CONTEXT: &str = {NONEMPTY_CONTEXT:?};\n\
         const EMPTY_CONTEXT: &str = {EMPTY_CONTEXT:?};\n\
         {FAKE_CLI_SOURCE}"
    );
    std::fs::write(&source_path, source).expect("fake analysis CLI source");
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let status = std::process::Command::new(rustc)
        .arg(&source_path)
        .arg("-o")
        .arg(&executable_path)
        .status()
        .expect("compile inert analysis CLI");
    assert!(status.success(), "inert analysis CLI must compile");
    executable_path
}

const FAKE_CLI_SOURCE: &str = r##"
use std::io::Read;

fn argument<'a>(args: &'a [String], flag: &str) -> &'a str {
    let mut values = args.windows(2).filter(|pair| pair[0] == flag);
    let value = &values.next().expect("required CLI flag")[1];
    assert!(values.next().is_none(), "duplicate CLI flag");
    value
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let codex = args.first().map(String::as_str) == Some("exec");
    if codex {
        assert!(args.iter().any(|arg| arg == "-"), "Codex stdin sentinel");
        assert!(!args.iter().any(|arg| arg == "--json-schema"));
    } else {
        assert_eq!(argument(&args, "-p"), "-", "Claude stdin sentinel");
        assert_eq!(argument(&args, "--output-format"), "json");
        assert!(!args.iter().any(|arg| arg == "--output-schema"));
    }
    assert!(
        !args.iter().any(|arg| arg.contains("schema-fixture-") || arg.contains(TRUSTED_PROMPT)),
        "analysis context and instructions must stay out of argv"
    );
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt).expect("analysis stdin");
    assert!(prompt.contains(TRUSTED_PROMPT), "trusted instructions in stdin");
    let nonempty = prompt.contains(NONEMPTY_CONTEXT);
    let empty = prompt.contains(EMPTY_CONTEXT);
    assert!(nonempty != empty, "exactly one unchanged activity context in stdin");

    // Read the actual schema argument/file, independently of the prompt text.
    let schema = if codex {
        std::fs::read_to_string(argument(&args, "--output-schema")).expect("Codex schema file")
    } else {
        argument(&args, "--json-schema").to_string()
    };
    if schema != EXPECTED_SCHEMA {
        eprintln!("fixture suggestion schema mismatch");
        std::process::exit(86);
    }
    let output = if nonempty {
        r#"{"suggestions":[{"type":"WorkflowOptimization","content":"Review the queued notes together.","confidence":0.91,"reasoning":"The fixture contains three related notes."}]}"#
    } else {
        r#"{"suggestions":[]}"#
    };
    if codex {
        std::fs::write(argument(&args, "--output-last-message"), output).expect("Codex output file");
        println!("fixture progress is not the analysis result");
    } else {
        println!("{{\"type\":\"result\",\"structured_output\":{}}}", output);
    }
}
"##;
