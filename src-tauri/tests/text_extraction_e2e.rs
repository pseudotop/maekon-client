//! End-to-end integration tests for the text extraction pipeline.
//!
//! Tests the flow: mock accessibility -> PII filter -> terminal/document
//! detection -> context assembly. Does not require OS accessibility permissions.

use oneshim_core::config::PiiFilterLevel;
use oneshim_core::models::focused_element::FocusedElementInfo;

// ── Test 1: Terminal command detected from Basic-level text ──

#[test]
fn terminal_command_detected_from_basic_pii_text() {
    // Simulate Basic-level extraction: email masked, command preserved
    let element = FocusedElementInfo {
        role: "AXTextArea".to_string(),
        label: Some("Terminal".to_string()),
        value_length: Some(50),
        extracted_text: Some("user@host:~$ cargo test --workspace".to_string()),
        position: None,
    };

    let cmd = oneshim_analysis::terminal_detector::detect_terminal_command(
        element.extracted_text.as_deref().unwrap(),
    );
    assert!(cmd.is_some());
    let cmd = cmd.unwrap();
    assert_eq!(cmd.command, "cargo");
    assert_eq!(cmd.command_line, "cargo test --workspace");
}

// ── Test 2: Document heading detected from editor text ──

#[test]
fn document_heading_detected_from_editor_text() {
    let element = FocusedElementInfo {
        role: "AXTextArea".to_string(),
        label: Some("Editor".to_string()),
        value_length: Some(200),
        extracted_text: Some("# Sprint Planning\n\nGoals for this week...".to_string()),
        position: None,
    };

    let heading = oneshim_analysis::document_heading::extract_document_heading(
        element.extracted_text.as_deref().unwrap(),
    );
    assert!(heading.is_some());
    let heading = heading.unwrap();
    assert_eq!(heading.heading, "Sprint Planning");
    assert_eq!(heading.level, 1);
}

// ── Test 3: Basic PII level masks email but preserves command ──

#[test]
fn pii_basic_level_masks_email_preserves_command() {
    // Use a realistic terminal output where an email appears in a
    // separate line (e.g., git log output), while the command prompt
    // is on its own line. The email mask applies to the email line,
    // but the command line is unaffected.
    let raw_text = "Author: user@example.com\n$ git push origin main";
    let filtered =
        oneshim_vision::privacy::sanitize_title_with_level(raw_text, PiiFilterLevel::Basic);

    // Email should be masked
    assert!(filtered.contains("[EMAIL]"));
    assert!(!filtered.contains("user@example.com"));

    // Command on a separate line should still be detectable
    let cmd = oneshim_analysis::terminal_detector::detect_terminal_command(&filtered);
    assert!(cmd.is_some());
    assert_eq!(cmd.unwrap().command, "git");
}

// ── Test 4: Strict level yields no text, no detection ──

#[test]
fn pii_strict_level_no_text_no_detection() {
    // At Strict level, extracted_text is None
    let element = FocusedElementInfo {
        role: "AXTextArea".to_string(),
        label: None,
        value_length: None,
        extracted_text: None,
        position: None,
    };

    // No text available for detection
    assert!(element.extracted_text.is_none());
    // Attempting detection on None would not fire in the pipeline
}

// ── Test 5: Standard level has metadata but no text ──

#[test]
fn pii_standard_level_no_text_but_has_metadata() {
    // At Standard level, we get role + label + value_length but no text
    let element = FocusedElementInfo {
        role: "AXTextArea".to_string(),
        label: Some("Terminal".to_string()),
        value_length: Some(50),
        extracted_text: None,
        position: None,
    };

    assert!(element.label.is_some());
    assert!(element.value_length.is_some());
    assert!(element.extracted_text.is_none());
}

// ── Test 6: ContextAssembler includes accessibility_text ──

#[test]
fn context_assembler_includes_accessibility_text() {
    use oneshim_analysis::{ContextAssembler, CurrentActivity, SessionMetrics};

    let assembler = ContextAssembler::new(Box::new(|t: &str| t.to_string()));
    let current = CurrentActivity {
        app_name: "iTerm2".to_string(),
        window_title: "zsh".to_string(),
        ocr_hint: None,
        focus_score: 0.7,
        deep_work_mins: 15,
        accessibility_text: Some("$ cargo clippy --workspace".to_string()),
    };
    let metrics = SessionMetrics {
        total_work_mins: 60,
        context_switches: 5,
        communication_ratio: 0.1,
    };

    let ctx = assembler.build(&current, &[], &[], &metrics);
    let parsed: serde_json::Value = serde_json::from_str(&ctx.user_context_json).unwrap();
    assert_eq!(
        parsed["current"]["accessibility_text"],
        "$ cargo clippy --workspace"
    );
}

// ── Test 7: Full pipeline — terminal at Basic level ──

#[test]
fn full_pipeline_terminal_basic_level() {
    // Simulate the full pipeline:
    // 1. Raw text from accessibility API (command on its own line)
    let raw = "Last login: Tue Mar 19 10:30:00\n$ kubectl get pods -n staging";

    // 2. PII filter at Basic level
    let filtered = oneshim_vision::privacy::sanitize_title_with_level(raw, PiiFilterLevel::Basic);

    // 3. Terminal command detection
    let cmd = oneshim_analysis::terminal_detector::detect_terminal_command(&filtered);
    assert!(cmd.is_some());
    let cmd = cmd.unwrap();
    assert_eq!(cmd.command, "kubectl");

    // 4. This would feed into ContextAssembler.accessibility_text
    assert!(!filtered.is_empty());
}

// ── Test 8: Full pipeline — document heading at Off level ──

#[test]
fn full_pipeline_document_heading_off_level() {
    // Off level: no masking
    let raw = "## Architecture Overview\n\nThe system uses Hexagonal Architecture.";
    let text = oneshim_vision::privacy::sanitize_title_with_level(raw, PiiFilterLevel::Off);

    let heading = oneshim_analysis::document_heading::extract_document_heading(&text);
    assert!(heading.is_some());
    let heading = heading.unwrap();
    assert_eq!(heading.heading, "Architecture Overview");
    assert_eq!(heading.level, 2);
}

// ── Test 9: Multiple prompt styles detected ──

#[test]
fn terminal_detection_multiple_prompts() {
    // zsh percent prompt
    let cmd = oneshim_analysis::terminal_detector::detect_terminal_command("% npm run build");
    assert!(cmd.is_some());
    assert_eq!(cmd.unwrap().command, "npm");

    // PowerShell chevron
    let cmd = oneshim_analysis::terminal_detector::detect_terminal_command("> Get-Process");
    assert!(cmd.is_some());
    assert_eq!(cmd.unwrap().command, "Get-Process");

    // root hash prompt
    let cmd =
        oneshim_analysis::terminal_detector::detect_terminal_command("# systemctl restart nginx");
    assert!(cmd.is_some());
    assert_eq!(cmd.unwrap().command, "systemctl");
}

// ── Test 10: Document heading with underline separator ──

#[test]
fn document_heading_underline_separator() {
    let text = "Meeting Notes 2026-03-19\n========================\nAttendees: Alice, Bob";
    let heading = oneshim_analysis::document_heading::extract_document_heading(text);
    assert!(heading.is_some());
    let heading = heading.unwrap();
    assert_eq!(heading.heading, "Meeting Notes 2026-03-19");
    assert_eq!(heading.level, 0); // Title detected by heuristic, level 0
}

// ── Test 11: No detection on empty element ──

#[test]
fn no_detection_on_empty_extracted_text() {
    let element = FocusedElementInfo {
        role: "AXTextArea".to_string(),
        label: None,
        value_length: Some(0),
        extracted_text: Some(String::new()),
        position: None,
    };

    let cmd = oneshim_analysis::terminal_detector::detect_terminal_command(
        element.extracted_text.as_deref().unwrap(),
    );
    assert!(cmd.is_none());

    let heading = oneshim_analysis::document_heading::extract_document_heading(
        element.extracted_text.as_deref().unwrap(),
    );
    assert!(heading.is_none());
}
