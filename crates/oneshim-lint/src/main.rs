//! # oneshim-lint
//!
//! Workspace lint tool (`language-check` binary). Validates coding conventions,
//! naming patterns, and architecture rules across the workspace.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

const FRONTEND_SRC_DIR: &str = "crates/oneshim-web/frontend/src";
const FRONTEND_LOCALES_DIR: &str = "crates/oneshim-web/frontend/src/i18n/locales";
const DEFAULT_CODE_ROOT: &str = "crates";
const SUPPORTED_LOCALES: [&str; 4] = ["en", "ko", "ja", "zh"];
const UI_ATTRS: [&str; 7] = [
    "placeholder",
    "title",
    "aria-label",
    "label",
    "helperText",
    "alt",
    "tooltip",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    NonEnglish,
    I18n,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone)]
struct Finding {
    severity: Severity,
    category: &'static str,
    path: PathBuf,
    line: usize,
    column: usize,
    message: String,
    snippet: String,
}

impl Finding {
    fn new(
        severity: Severity,
        category: &'static str,
        path: PathBuf,
        line: usize,
        column: usize,
        message: String,
        snippet: String,
    ) -> Self {
        Self {
            severity,
            category,
            path,
            line,
            column,
            message,
            snippet,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_help();
        return ExitCode::SUCCESS;
    }

    let mode = parse_mode(&args);
    let strict_i18n = args.iter().any(|arg| arg == "--strict-i18n");
    let scan_paths = collect_option_values(&args, "--path");
    let ignore_paths = collect_option_values(&args, "--ignore");

    let repo_root = PathBuf::from(".");
    let mut findings: Vec<Finding> = Vec::new();

    if mode == Mode::NonEnglish || mode == Mode::All {
        findings.extend(scan_non_english_text(
            &repo_root,
            &scan_paths,
            &ignore_paths,
        ));
    }

    if mode == Mode::I18n || mode == Mode::All {
        findings.extend(scan_i18n(&repo_root, &ignore_paths));
    }

    print_summary(&findings, strict_i18n);

    let has_errors = findings.iter().any(|f| f.severity == Severity::Error);
    let has_strict_warnings =
        strict_i18n && findings.iter().any(|f| f.severity == Severity::Warning);

    if has_errors || has_strict_warnings {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_mode(args: &[String]) -> Mode {
    if let Some(first) = args.first() {
        match first.as_str() {
            "non-english" => Mode::NonEnglish,
            "i18n" => Mode::I18n,
            _ => Mode::All,
        }
    } else {
        Mode::All
    }
}

fn print_help() {
    println!("language-check - language and i18n quality checker");
    println!();
    println!("Usage:");
    println!("  cargo run -p oneshim-lint --bin language-check -- [non-english|i18n|all] [--strict-i18n] [--path <dir>] [--ignore <substring>]");
    println!();
    println!("Modes:");
    println!(
        "  non-english   Scan code files for non-ASCII characters (excluding locale JSON files)."
    );
    println!("  i18n          Validate frontend locale key coverage and i18n usage heuristics.");
    println!("  all           Run both checks (default).");
    println!();
    println!("Options:");
    println!("  --strict-i18n Treat i18n warnings (hardcoded UI strings) as build failures.");
    println!("  --path <dir>  Limit scan to a specific subdirectory (repeatable).");
    println!("  --ignore <s>  Ignore files whose path contains substring <s> (repeatable).");
}

fn print_summary(findings: &[Finding], strict_i18n: bool) {
    if findings.is_empty() {
        println!("language-check: no findings");
        return;
    }

    let mut error_count = 0usize;
    let mut warning_count = 0usize;
    for finding in findings {
        match finding.severity {
            Severity::Error => error_count += 1,
            Severity::Warning => warning_count += 1,
        }

        let sev = match finding.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN ",
        };

        println!(
            "[{}] {}:{}:{} [{}] {}",
            sev,
            finding.path.display(),
            finding.line,
            finding.column,
            finding.category,
            finding.message
        );
        if !finding.snippet.is_empty() {
            println!("  -> {}", finding.snippet);
        }
    }

    println!();
    println!(
        "language-check summary: {} error(s), {} warning(s), strict_i18n={}",
        error_count, warning_count, strict_i18n
    );
}

fn collect_option_values(args: &[String], option: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == option {
            if let Some(value) = args.get(i + 1) {
                values.push(value.clone());
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    values
}

fn scan_non_english_text(
    repo_root: &Path,
    scan_paths: &[String],
    ignore_paths: &[String],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let roots: Vec<PathBuf> = if scan_paths.is_empty() {
        vec![repo_root.join(DEFAULT_CODE_ROOT)]
    } else {
        scan_paths.iter().map(|p| repo_root.join(p)).collect()
    };

    let mut files = Vec::new();
    for root in roots {
        files.extend(collect_files(&root, &["rs", "ts", "tsx", "js", "jsx"]));
    }

    for file in files {
        if is_ignored(&file, ignore_paths) {
            continue;
        }
        if is_locale_file(&file) {
            continue;
        }

        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            if let Some((col, ch)) = first_non_ascii(line) {
                let category = if ch.is_alphabetic() {
                    "non-english-character"
                } else {
                    "non-ascii-character"
                };
                findings.push(Finding::new(
                    Severity::Error,
                    category,
                    file.clone(),
                    line_idx + 1,
                    col,
                    format!("Non-ASCII character detected: {:?}", ch),
                    line.trim().to_string(),
                ));
            }
        }
    }

    findings
}

fn scan_i18n(repo_root: &Path, ignore_paths: &[String]) -> Vec<Finding> {
    let mut findings = Vec::new();
    let locale_root = repo_root.join(FRONTEND_LOCALES_DIR);

    let mut locale_keys: HashMap<String, BTreeSet<String>> = HashMap::new();
    for locale in SUPPORTED_LOCALES {
        let locale_path = locale_root.join(format!("{locale}.json"));
        match load_locale_keys(&locale_path) {
            Ok(keys) => {
                locale_keys.insert(locale.to_string(), keys);
            }
            Err(err) => findings.push(Finding::new(
                Severity::Error,
                "locale-load",
                locale_path.clone(),
                1,
                1,
                err,
                String::new(),
            )),
        }
    }

    let en_keys = locale_keys.get("en").cloned().unwrap_or_default();
    for locale in SUPPORTED_LOCALES {
        if locale == "en" {
            continue;
        }

        let Some(keys) = locale_keys.get(locale) else {
            continue;
        };

        for missing in en_keys.difference(keys) {
            findings.push(Finding::new(
                Severity::Error,
                "missing-locale-key",
                locale_root.join(format!("{locale}.json")),
                1,
                1,
                format!("Missing translation key in {locale}: {missing}"),
                String::new(),
            ));
        }

        for extra in keys.difference(&en_keys) {
            findings.push(Finding::new(
                Severity::Warning,
                "extra-locale-key",
                locale_root.join(format!("{locale}.json")),
                1,
                1,
                format!("Extra key present only in {locale}: {extra}"),
                String::new(),
            ));
        }
    }

    let frontend_src = repo_root.join(FRONTEND_SRC_DIR);
    let files = collect_files(&frontend_src, &["ts", "tsx"]);
    for file in files {
        if is_ignored(&file, ignore_paths) {
            continue;
        }
        if is_locale_file(&file) {
            continue;
        }

        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };

        for (line_idx, line) in content.lines().enumerate() {
            for key in extract_translation_keys(line) {
                if !en_keys.contains(&key) {
                    findings.push(Finding::new(
                        Severity::Error,
                        "missing-i18n-key",
                        file.clone(),
                        line_idx + 1,
                        1,
                        format!("Unknown i18n key used: {key}"),
                        line.trim().to_string(),
                    ));
                }
            }

            if file.extension().and_then(|ext| ext.to_str()) == Some("tsx") {
                for (column, message) in detect_hardcoded_ui_literals(line) {
                    findings.push(Finding::new(
                        Severity::Warning,
                        "hardcoded-ui-copy",
                        file.clone(),
                        line_idx + 1,
                        column,
                        message,
                        line.trim().to_string(),
                    ));
                }
            }
        }
    }

    findings
}

fn load_locale_keys(path: &Path) -> Result<BTreeSet<String>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read locale file {}: {e}", path.display()))?;
    let value: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse locale JSON {}: {e}", path.display()))?;

    let mut keys = BTreeSet::new();
    flatten_json_keys("", &value, &mut keys);
    Ok(keys)
}

fn flatten_json_keys(prefix: &str, value: &Value, keys: &mut BTreeSet<String>) {
    if let Value::Object(map) = value {
        for (key, nested) in map {
            let full_key = if prefix.is_empty() {
                key.to_string()
            } else {
                format!("{prefix}.{key}")
            };

            match nested {
                Value::Object(_) => flatten_json_keys(&full_key, nested, keys),
                _ => {
                    keys.insert(full_key);
                }
            }
        }
    }
}

fn extract_translation_keys(line: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut search_from = 0usize;

    while search_from < line.len() {
        let Some(rel_pos) = line[search_from..].find("t(") else {
            break;
        };
        let pos = search_from + rel_pos;

        if pos > 0 {
            let prev = line[..pos].chars().next_back().unwrap_or(' ');
            if prev.is_ascii_alphanumeric() || prev == '_' {
                search_from = pos + 2;
                continue;
            }
        }

        let mut idx = pos + 2;
        while idx < line.len() {
            let ch = line[idx..].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                idx += ch.len_utf8();
            } else {
                break;
            }
        }

        if idx >= line.len() {
            break;
        }

        let quote = line[idx..].chars().next().unwrap_or(' ');
        if quote != '"' && quote != '\'' {
            search_from = pos + 2;
            continue;
        }

        idx += quote.len_utf8();
        let start = idx;
        let mut escaped = false;

        while idx < line.len() {
            let ch = line[idx..].chars().next().unwrap_or(' ');
            if escaped {
                escaped = false;
                idx += ch.len_utf8();
                continue;
            }
            if ch == '\\' {
                escaped = true;
                idx += ch.len_utf8();
                continue;
            }
            if ch == quote {
                keys.push(line[start..idx].to_string());
                idx += ch.len_utf8();
                break;
            }
            idx += ch.len_utf8();
        }

        search_from = idx;
    }

    keys
}

fn detect_hardcoded_ui_literals(line: &str) -> Vec<(usize, String)> {
    let mut hits = Vec::new();

    for attr in UI_ATTRS {
        let marker = format!("{attr}=\"");
        let mut search_from = 0usize;
        while let Some(rel_pos) = line[search_from..].find(&marker) {
            let pos = search_from + rel_pos;
            if pos > 0 {
                let prev = line[..pos].chars().next_back().unwrap_or(' ');
                if prev.is_ascii_alphanumeric() || prev == '_' || prev == '-' {
                    search_from = pos + marker.len();
                    continue;
                }
            }
            let value_start = pos + marker.len();
            let Some(value_end_rel) = line[value_start..].find('"') else {
                break;
            };
            let value_end = value_start + value_end_rel;
            let value = &line[value_start..value_end];
            if contains_human_text(value) {
                hits.push((
                    pos + 1,
                    format!("Hardcoded UI attribute `{attr}` should use i18n"),
                ));
            }
            search_from = value_end + 1;
        }
    }

    let mut segment_start = 0usize;
    while let Some(gt_rel) = line[segment_start..].find('>') {
        let gt = segment_start + gt_rel;
        let Some(lt_rel) = line[gt + 1..].find('<') else {
            break;
        };
        let lt = gt + 1 + lt_rel;
        let segment = line[gt + 1..lt].trim();
        if !segment.starts_with('{') && !segment.ends_with('}') && contains_human_text(segment) {
            hits.push((gt + 2, "Hardcoded UI text node should use i18n".to_string()));
        }
        segment_start = lt + 1;
    }

    hits
}

fn contains_human_text(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 2 {
        return false;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") || trimmed.starts_with('/')
    {
        return false;
    }

    let has_letter = trimmed.chars().any(|c| c.is_alphabetic());
    if !has_letter {
        return false;
    }

    if trimmed.len() <= 4 && trimmed.chars().all(|c| c.is_ascii_uppercase()) {
        return false;
    }

    true
}

fn is_locale_file(path: &Path) -> bool {
    path.to_string_lossy().contains("/src/i18n/locales/")
}

fn is_ignored(path: &Path, ignores: &[String]) -> bool {
    if ignores.is_empty() {
        return false;
    }
    let path_str = path.to_string_lossy();
    ignores.iter().any(|needle| path_str.contains(needle))
}

fn first_non_ascii(line: &str) -> Option<(usize, char)> {
    for (idx, ch) in line.char_indices() {
        if !ch.is_ascii() {
            return Some((idx + 1, ch));
        }
    }
    None
}

fn collect_files(root: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut extension_set: HashSet<&str> = HashSet::new();
    extension_set.extend(extensions.iter().copied());
    collect_files_recursive(root, &extension_set, &mut files);
    files
}

fn collect_files_recursive(root: &Path, extensions: &HashSet<&str>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if file_name == ".git"
            || file_name == "target"
            || file_name == "node_modules"
            || file_name == "dist"
        {
            continue;
        }

        if path.is_dir() {
            collect_files_recursive(&path, extensions, out);
            continue;
        }

        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if extensions.contains(ext) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // ── extract_translation_keys (existing) ──────────────────────────

    #[test]
    fn extract_translation_keys_works() {
        let line =
            r#"const x = t('dashboard.title', 'Dashboard'); const y = i18n.t("common.save")"#;
        let keys = extract_translation_keys(line);
        assert_eq!(keys, vec!["dashboard.title", "common.save"]);
    }

    #[test]
    fn extract_translation_keys_unicode() {
        let keys = extract_translation_keys("t('settings.édition')");
        assert_eq!(keys, vec!["settings.édition"]);
    }

    #[test]
    fn extract_translation_keys_ignores_non_i18n_calls() {
        let line = r#"const value = set("x"); const n = get(1);"#;
        let keys = extract_translation_keys(line);
        assert!(keys.is_empty());
    }

    // ── contains_human_text (existing + edge cases) ──────────────────

    #[test]
    fn contains_human_text_heuristic() {
        assert!(contains_human_text("Click to continue"));
        assert!(contains_human_text("사용자 설정"));
        assert!(!contains_human_text("12345"));
        assert!(!contains_human_text("OK"));
    }

    #[test]
    fn contains_human_text_urls_rejected() {
        assert!(!contains_human_text("http://example.com"));
        assert!(!contains_human_text("https://example.com/path"));
        assert!(!contains_human_text("/api/v1/users"));
    }

    #[test]
    fn contains_human_text_short_strings_rejected() {
        assert!(!contains_human_text(""));
        assert!(!contains_human_text("X"));
        assert!(!contains_human_text(" "));
    }

    #[test]
    fn contains_human_text_short_uppercase_acronyms_rejected() {
        assert!(!contains_human_text("ID"));
        assert!(!contains_human_text("URL"));
        assert!(!contains_human_text("HTTP"));
    }

    #[test]
    fn contains_human_text_long_uppercase_accepted() {
        // >4 chars uppercase is accepted
        assert!(contains_human_text("HELLO"));
        assert!(contains_human_text("SETTINGS"));
    }

    #[test]
    fn contains_human_text_whitespace_trimmed() {
        assert!(!contains_human_text("  "));
        assert!(!contains_human_text("  1  "));
        assert!(contains_human_text("  Hello World  "));
    }

    #[test]
    fn contains_human_text_mixed_content() {
        assert!(contains_human_text("Item #42"));
        assert!(contains_human_text("v2.0 release"));
        assert!(!contains_human_text("123-456"));
        assert!(!contains_human_text("###"));
    }

    // ── flatten_json_keys (existing) ─────────────────────────────────

    #[test]
    fn flatten_json_keys_works() {
        let value = serde_json::json!({
            "common": {
                "save": "Save",
                "cancel": "Cancel"
            },
            "dashboard": {
                "title": "Dashboard"
            }
        });
        let mut keys = BTreeSet::new();
        flatten_json_keys("", &value, &mut keys);
        assert!(keys.contains("common.save"));
        assert!(keys.contains("common.cancel"));
        assert!(keys.contains("dashboard.title"));
    }

    // ── parse_mode ───────────────────────────────────────────────────

    #[test]
    fn parse_mode_non_english() {
        let args = vec!["non-english".to_string()];
        assert_eq!(parse_mode(&args), Mode::NonEnglish);
    }

    #[test]
    fn parse_mode_i18n() {
        let args = vec!["i18n".to_string()];
        assert_eq!(parse_mode(&args), Mode::I18n);
    }

    #[test]
    fn parse_mode_explicit_all() {
        let args = vec!["all".to_string()];
        assert_eq!(parse_mode(&args), Mode::All);
    }

    #[test]
    fn parse_mode_empty_args_defaults_to_all() {
        let args: Vec<String> = vec![];
        assert_eq!(parse_mode(&args), Mode::All);
    }

    #[test]
    fn parse_mode_unknown_first_arg_defaults_to_all() {
        let args = vec!["unknown-mode".to_string()];
        assert_eq!(parse_mode(&args), Mode::All);
    }

    #[test]
    fn parse_mode_flag_as_first_arg_defaults_to_all() {
        let args = vec!["--strict-i18n".to_string()];
        assert_eq!(parse_mode(&args), Mode::All);
    }

    #[test]
    fn parse_mode_with_trailing_flags() {
        let args = vec![
            "non-english".to_string(),
            "--strict-i18n".to_string(),
            "--path".to_string(),
            "src".to_string(),
        ];
        assert_eq!(parse_mode(&args), Mode::NonEnglish);
    }

    // ── detect_hardcoded_ui_literals ─────────────────────────────────

    #[test]
    fn detect_hardcoded_placeholder_attribute() {
        let line = r#"<Input placeholder="Enter your name" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("placeholder"));
    }

    #[test]
    fn detect_hardcoded_title_attribute() {
        let line = r#"<div title="Click to expand details">"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("title"));
    }

    #[test]
    fn detect_hardcoded_aria_label() {
        let line = r#"<button aria-label="Close dialog">"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("aria-label"));
    }

    #[test]
    fn detect_hardcoded_label_attribute() {
        let line = r#"<Field label="Username" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("label"));
    }

    #[test]
    fn detect_hardcoded_helper_text() {
        let line = r#"<TextField helperText="Must be at least 8 characters" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("helperText"));
    }

    #[test]
    fn detect_hardcoded_alt_attribute() {
        let line = r#"<img alt="User avatar" src="pic.png" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("alt"));
    }

    #[test]
    fn detect_hardcoded_tooltip_attribute() {
        let line = r#"<Icon tooltip="Show more options" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("tooltip"));
    }

    #[test]
    fn detect_hardcoded_multiple_attrs_on_same_line() {
        let line = r#"<Input placeholder="Enter name" title="Name field" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn detect_hardcoded_skips_non_human_values() {
        // URL in placeholder — not human text
        let line = r#"<img alt="/icons/logo.svg" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_hardcoded_skips_short_acronym_values() {
        let line = r#"<span title="ID" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_hardcoded_text_node_between_tags() {
        let line = r#"<span>Submit form</span>"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("text node"));
    }

    #[test]
    fn detect_hardcoded_text_node_skips_jsx_expression() {
        // {variable} between tags should not be flagged
        let line = r#"<span>{userName}</span>"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_hardcoded_text_node_skips_empty_content() {
        let line = r#"<span></span>"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_hardcoded_text_node_skips_numeric_content() {
        let line = r#"<span>42</span>"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_hardcoded_no_false_positive_on_data_attr() {
        // data-placeholder is not in UI_ATTRS, should not trigger
        let line = r#"<div data-placeholder="internal value" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    #[test]
    fn detect_nested_jsx_tags() {
        let hits = detect_hardcoded_ui_literals("<div><span>Submit</span></div>");
        assert!(!hits.is_empty(), "should detect 'Submit' in nested tags");
    }

    #[test]
    fn detect_hardcoded_attr_preceded_by_alnum_skipped() {
        // "myplaceholder=" should not match the "placeholder=" pattern
        // because 'y' is alphanumeric and precedes it
        let line = r#"<Input myplaceholder="Enter name" />"#;
        let hits = detect_hardcoded_ui_literals(line);
        assert!(hits.is_empty());
    }

    // ── load_locale_keys ─────────────────────────────────────────────

    #[test]
    fn load_locale_keys_valid_flat_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("en.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, r#"{{"save":"Save","cancel":"Cancel"}}"#).unwrap();

        let keys = load_locale_keys(&path).unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains("save"));
        assert!(keys.contains("cancel"));
    }

    #[test]
    fn load_locale_keys_valid_nested_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("en.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(
            f,
            r#"{{"common":{{"save":"Save","cancel":"Cancel"}},"dashboard":{{"title":"Dashboard"}}}}"#
        )
        .unwrap();

        let keys = load_locale_keys(&path).unwrap();
        assert_eq!(keys.len(), 3);
        assert!(keys.contains("common.save"));
        assert!(keys.contains("common.cancel"));
        assert!(keys.contains("dashboard.title"));
    }

    #[test]
    fn load_locale_keys_deeply_nested_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("en.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, r#"{{"a":{{"b":{{"c":"deep"}}}}}}"#).unwrap();

        let keys = load_locale_keys(&path).unwrap();
        assert_eq!(keys.len(), 1);
        assert!(keys.contains("a.b.c"));
    }

    #[test]
    fn load_locale_keys_file_not_found() {
        let result = load_locale_keys(Path::new("/tmp/nonexistent-locale-file.json"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Failed to read locale file"));
    }

    #[test]
    fn load_locale_keys_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{{not valid json}}").unwrap();

        let result = load_locale_keys(&path);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Failed to parse locale JSON"));
    }

    #[test]
    fn load_locale_keys_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.json");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{{}}").unwrap();

        let keys = load_locale_keys(&path).unwrap();
        assert!(keys.is_empty());
    }

    // ── collect_option_values ────────────────────────────────────────

    #[test]
    fn collect_option_values_single() {
        let args: Vec<String> = vec!["--path", "src"]
            .into_iter()
            .map(String::from)
            .collect();
        let vals = collect_option_values(&args, "--path");
        assert_eq!(vals, vec!["src"]);
    }

    #[test]
    fn collect_option_values_multiple() {
        let args: Vec<String> = vec!["--ignore", "test", "--ignore", "dist"]
            .into_iter()
            .map(String::from)
            .collect();
        let vals = collect_option_values(&args, "--ignore");
        assert_eq!(vals, vec!["test", "dist"]);
    }

    #[test]
    fn collect_option_values_none() {
        let args: Vec<String> = vec!["--strict-i18n"]
            .into_iter()
            .map(String::from)
            .collect();
        let vals = collect_option_values(&args, "--path");
        assert!(vals.is_empty());
    }

    #[test]
    fn collect_option_values_dangling_flag() {
        // --path at end without a value
        let args: Vec<String> = vec!["--path"].into_iter().map(String::from).collect();
        let vals = collect_option_values(&args, "--path");
        assert!(vals.is_empty());
    }

    // ── first_non_ascii ──────────────────────────────────────────────

    #[test]
    fn first_non_ascii_all_ascii() {
        assert!(first_non_ascii("hello world 123!").is_none());
    }

    #[test]
    fn first_non_ascii_cjk_character() {
        let result = first_non_ascii("let x = '안녕';");
        assert!(result.is_some());
        let (_, ch) = result.unwrap();
        assert_eq!(ch, '안');
    }

    #[test]
    fn first_non_ascii_emoji() {
        let result = first_non_ascii("// TODO 🚀");
        assert!(result.is_some());
    }

    #[test]
    fn first_non_ascii_empty_string() {
        assert!(first_non_ascii("").is_none());
    }

    // ── is_locale_file ───────────────────────────────────────────────

    #[test]
    fn is_locale_file_matches() {
        assert!(is_locale_file(Path::new(
            "crates/oneshim-web/frontend/src/i18n/locales/en.json"
        )));
    }

    #[test]
    fn is_locale_file_no_match() {
        assert!(!is_locale_file(Path::new("crates/oneshim-core/src/lib.rs")));
    }

    // ── is_ignored ───────────────────────────────────────────────────

    #[test]
    fn is_ignored_matches() {
        let ignores = vec!["node_modules".to_string(), "dist".to_string()];
        assert!(is_ignored(
            Path::new("frontend/node_modules/react/index.js"),
            &ignores
        ));
    }

    #[test]
    fn is_ignored_no_match() {
        let ignores = vec!["node_modules".to_string()];
        assert!(!is_ignored(Path::new("src/main.rs"), &ignores));
    }

    #[test]
    fn is_ignored_empty_list() {
        assert!(!is_ignored(Path::new("anything.rs"), &[]));
    }
}
