use crate::args::{collect_option_values, parse_mode};
use crate::finding::Mode;
use crate::fs_scan::{first_non_ascii, is_ignored, is_locale_file};
use crate::i18n::SUPPORTED_LOCALES;
use std::path::Path;

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

#[test]
fn supported_locales_match_frontend_resources() {
    assert_eq!(
        SUPPORTED_LOCALES.as_slice(),
        ["en", "ko", "ja", "zh-CN", "es"].as_slice()
    );
}

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
    let args: Vec<String> = vec!["--path"].into_iter().map(String::from).collect();
    let vals = collect_option_values(&args, "--path");
    assert!(vals.is_empty());
}

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

#[test]
fn is_locale_file_matches() {
    assert!(is_locale_file(Path::new(
        "crates/maekon-web/frontend/src/i18n/locales/en.json"
    )));
}

#[test]
fn is_locale_file_no_match() {
    assert!(!is_locale_file(Path::new("crates/maekon-core/src/lib.rs")));
}

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
