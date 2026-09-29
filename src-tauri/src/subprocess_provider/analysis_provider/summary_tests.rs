//! #12338: real summary-port dispatch through inert, locally compiled CLIs.

use super::*;
use maekon_core::models::ai_summary::{AiSummaryArtifact, AiSummaryProviderClass};
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

const PRIVATE_FIXTURE: &str = "summary-fixture@example.test";
const CONTEXT: &str = r#"{"fixture":"summary-fixture-data"}"#;
const TASK: &str = "Summarize the summary-fixture-data activity.";
const SUMMARY: &str = "Reviewed notes for summary-fixture@example.test.";

fn fixture_provider(surface: &str) -> SubprocessAnalysisProvider {
    SubprocessAnalysisProvider::new(
        DetectedSubprocessCli {
            surface_id: format!("provider_surface.{surface}.subprocess_cli"),
            executable_path: fake_cli(),
        },
        &AiProviderConfig::default(),
    )
}

#[tokio::test]
async fn cli_summary_port_delivers_its_schema_and_keeps_suggestion_schema() {
    for surface in ["openai", "anthropic", "google"] {
        let provider = fixture_provider(surface);
        assert_eq!(
            provider
                .summarize_text(CONTEXT, TASK)
                .await
                .expect("summary port"),
            SUMMARY
        );
        // A summary invocation must not replace this adapter's analysis schema.
        assert!(provider
            .analyze(CONTEXT, TASK)
            .await
            .expect("suggestion port after summary")
            .is_empty());
        let error = provider
            .summarize_text("summary-case-malformed", TASK)
            .await
            .expect_err("malformed summary");
        assert!(matches!(
            error,
            CoreError::Analysis {
                code: maekon_core::error_codes::ProviderCode::AnalysisFailed,
                ..
            }
        ));
        assert!(!error.to_string().contains(PRIVATE_FIXTURE));
    }
}

#[test]
fn summary_parser_rejects_empty_error_and_overdeep_envelopes() {
    for value in [
        serde_json::json!({"summary":" "}),
        serde_json::json!({"summary":42}),
        serde_json::json!({"suggestions":[]}),
        serde_json::json!({"error":"failed", "summary":"unsafe"}),
        serde_json::json!({"is_error":true,"structured_output":{"summary":"unsafe"}}),
    ] {
        assert_eq!(parse_summary_value(&value, 0), None, "{value}");
    }
    let expected = "{\"narrative\":\"Done\",\"highlights\":[]}";
    let envelope =
        serde_json::json!({"result": serde_json::json!({"summary": expected}).to_string()});
    assert_eq!(parse_summary_value(&envelope, 0).as_deref(), Some(expected));
    let mut deep = serde_json::json!({"summary":"present"});
    for _ in 0..8 {
        deep = serde_json::json!({"result":deep});
    }
    assert_eq!(parse_summary_value(&deep, 0), None);
}

#[test]
fn summary_parser_shares_six_envelope_limit_across_objects_and_json_strings() {
    // #12338: six wrappers are supported; a seventh must be rejected even when
    // a provider encodes its next envelope as JSON text rather than an object.
    for encoding in [
        [false; 6],
        [true; 6],
        [false, true, false, true, false, true],
    ] {
        let mut boundary = serde_json::json!({"summary": SUMMARY});
        for as_text in encoding {
            boundary = wrap_summary_envelope(boundary, as_text);
        }
        assert_eq!(
            parse_summary_value(&boundary, 0).as_deref(),
            Some(SUMMARY),
            "six supported envelopes: {encoding:?}"
        );
        for as_text in [false, true] {
            let too_deep = wrap_summary_envelope(boundary.clone(), as_text);
            assert_eq!(
                parse_summary_value(&too_deep, 0),
                None,
                "seventh envelope must be rejected: {encoding:?}, text={as_text}"
            );
        }
    }
}

fn wrap_summary_envelope(value: serde_json::Value, as_text: bool) -> serde_json::Value {
    if as_text {
        serde_json::Value::String(value.to_string())
    } else {
        serde_json::json!({"result": value})
    }
}

fn privacy_filter() -> maekon_analysis::PiiFilter {
    Box::new(|value| value.replace(PRIVATE_FIXTURE, "[redacted]"))
}

#[tokio::test]
async fn cli_summary_consumers_keep_text_json_and_filtered_provenance() {
    use chrono::Utc;
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics};
    use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
    let provider: Arc<dyn AnalysisProvider> = Arc::new(fixture_provider("openai"));
    let summarizer = maekon_analysis::LlmSegmentSummarizer::new_with_provider_class(
        provider.clone(),
        privacy_filter(),
        true,
        60,
        AiSummaryProviderClass::Subprocess,
    );
    let segment = SegmentSummary {
        segment_id: "seg-summary-fixture".into(),
        start_time: Utc::now(),
        end_time: Utc::now(),
        duration_secs: 600,
        regime_id: None,
        trigger_reason: TriggerReason::ForcedMaxDuration,
        event_count: 3,
        app_breakdown: Default::default(),
        category_breakdown: Default::default(),
        context_switch_count: 1,
        dominant_category: "Development".into(),
        avg_importance: 0.7,
        patterns_detected: vec![],
        content_activities: vec![],
        container: None,
        llm_summary: None,
    };
    let artifact = summarizer.summarize(&segment).await;
    assert_eq!(
        artifact.text.as_deref(),
        Some("Reviewed notes for [redacted].")
    );
    assert_provenance(&artifact);
    let generator = maekon_analysis::DailyInsightGenerator::new_with_provider_class(
        provider,
        privacy_filter(),
        AiSummaryProviderClass::Subprocess,
    );
    let digest = DailyDigest {
        date: Utc::now().date_naive(),
        insight: None,
        timeline: vec![],
        statistics: DailyStatistics {
            deep_work_hours: 1.0,
            communication_hours: 0.0,
            meeting_hours: 0.0,
            context_switches: 1,
            longest_focus_mins: 60,
            longest_focus_content: "fixture notes".into(),
            regime_distribution: Default::default(),
            comparison: None,
        },
        generated_at: Utc::now(),
        digest_provenance: "heuristic".into(),
        ai_narrative: Default::default(),
    };
    let (insight, artifact) = generator.generate_with_artifact(&digest).await;
    let insight = insight.expect("the daily consumer parses JSON inside the summary string");
    assert_eq!(insight.narrative, "Reviewed notes for [redacted].");
    assert_eq!(insight.highlights.len(), 1);
    assert_eq!(insight.highlights[0].text, "Finished notes for [redacted].");
    assert_eq!(digest.digest_provenance, "heuristic");
    assert_provenance(&artifact);
}

fn assert_provenance(artifact: &AiSummaryArtifact) {
    assert!(artifact.is_generated());
    assert_eq!(
        artifact.provider_class,
        Some(AiSummaryProviderClass::Subprocess)
    );
    assert_eq!(artifact.failure_reason, None);
    assert!(!serde_json::to_string(artifact)
        .unwrap()
        .contains(PRIVATE_FIXTURE));
}

fn fake_cli() -> PathBuf {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("summary fixture");
        let source = dir.path().join("summary_cli.rs");
        let binary = dir.path().join(if cfg!(windows) { "summary-cli.exe" } else { "summary-cli" });
        let wire_summary = serde_json::json!({"summary":SUMMARY}).to_string();
        let daily = serde_json::json!({"narrative":SUMMARY,"highlights":[{"type":"achievement","text":"Finished notes for summary-fixture@example.test."}]}).to_string();
        let wire_daily = serde_json::json!({"summary":daily}).to_string();
        let code = format!("const SUMMARY_SCHEMA: &str = {SUMMARY_SCHEMA_JSON:?};\nconst SUGGESTION_SCHEMA: &str = {SUGGESTION_SCHEMA_JSON:?};\nconst WIRE_SUMMARY: &str = {wire_summary:?};\nconst WIRE_DAILY: &str = {wire_daily:?};\n{FAKE_CLI}");
        std::fs::write(&source, code).expect("inert CLI source");
        let status = std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(&source).arg("-o").arg(&binary).status().expect("compile inert CLI");
        assert!(status.success());
        (dir, binary)
    }).1.clone()
}

const FAKE_CLI: &str = r###"
use std::io::Read;
fn argument<'a>(args: &'a [String], flag: &str) -> &'a str {
    let mut matches = args.windows(2).filter(|pair| pair[0] == flag);
    let value = &matches.next().expect("required flag")[1];
    assert!(matches.next().is_none(), "unique flag"); value
}
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let codex = args.first().map(String::as_str) == Some("exec");
    if codex { assert!(args.iter().any(|arg| arg == "-")); }
    else { assert_eq!(argument(&args,"-p"), "-"); }
    assert!(!args.iter().any(|arg| arg.contains("summary-fixture")), "context must stay out of argv");
    let mut prompt = String::new(); std::io::stdin().read_to_string(&mut prompt).unwrap();
    let summary = prompt.contains(SUMMARY_SCHEMA);
    let expected = if summary { SUMMARY_SCHEMA } else { SUGGESTION_SCHEMA };
    if codex {
        assert_eq!(std::fs::read_to_string(argument(&args,"--output-schema")).unwrap(), expected, "summary schema file");
    } else if args.iter().any(|arg| arg == "--json-schema") {
        assert_eq!(argument(&args,"--json-schema"), expected, "summary schema argument");
    } else { assert_eq!(argument(&args,"--output-format"), "json"); }
    assert!(prompt.contains("summary-fixture-data") || prompt.contains("summary-case-malformed") || prompt.contains("desktop work session segment") || prompt.contains("productivity coach"), "real task and context in stdin");
    let output = if prompt.contains("summary-case-malformed") { "summary-fixture@example.test" }
        else if !summary { r#"{"suggestions":[]}"# }
        else if prompt.contains("productivity coach") { WIRE_DAILY } else { WIRE_SUMMARY };
    if codex { std::fs::write(argument(&args,"--output-last-message"),output).unwrap(); println!("progress only"); }
    else if args.iter().any(|arg| arg == "--json-schema") { println!("{{\"structured_output\":{output}}}"); }
    else { println!("{output}"); }
}
"###;
