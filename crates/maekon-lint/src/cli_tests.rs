use crate::args::{collect_option_values, parse_mode};
use crate::finding::Mode;
use crate::fs_scan::{first_non_ascii, is_ignored};
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
    // Test input embeds the Korean word "annyeong" (U+C548 U+B155) written as
    // \u{...} escapes so this source file stays ASCII while preserving the exact
    // bytes under test.
    let result = first_non_ascii("let x = '\u{c548}\u{b155}';");
    assert!(result.is_some());
    let (_, ch) = result.unwrap();
    assert_eq!(ch, '\u{c548}');
}

#[test]
fn first_non_ascii_emoji() {
    // The rocket emoji (U+1F680) is written as a \u{...} escape so this source
    // file stays ASCII; the test still exercises non-ASCII emoji detection.
    let result = first_non_ascii("// TODO \u{1f680}");
    assert!(result.is_some());
}

#[test]
fn first_non_ascii_empty_string() {
    assert!(first_non_ascii("").is_none());
}

// is_locale_file tests removed: the function was deleted (Finding #16).
// collect_files only collects .rs/.ts/.tsx/.js/.jsx — never .json — so the
// guard could never trigger.  See fs_scan.rs for the full rationale.

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

// --- Finding #14 regression tests: empty needle must NOT ignore every path ---

/// A single empty-string needle must NOT cause every path to be ignored.
/// `"any_string".contains("")` is always true, so without the filter this
/// would silently make the lint scan nothing and exit 0.
#[test]
fn is_ignored_empty_needle_does_not_ignore_all_paths() {
    let ignores = vec!["".to_string()];
    assert!(
        !is_ignored(Path::new("src/main.rs"), &ignores),
        "an empty needle must not match every path"
    );
}

/// An empty needle mixed with a real needle: only the real needle matches.
#[test]
fn is_ignored_empty_needle_mixed_with_real_needle() {
    let ignores = vec!["".to_string(), "node_modules".to_string()];
    // Path that contains "node_modules" — should be ignored (real needle fires).
    assert!(is_ignored(
        Path::new("frontend/node_modules/pkg/index.js"),
        &ignores,
    ));
    // Path that does NOT contain "node_modules" — must NOT be ignored by the
    // empty needle.
    assert!(
        !is_ignored(Path::new("src/lib.rs"), &ignores),
        "empty needle must not shadow a non-matching real needle"
    );
}

/// All-empty needle list is equivalent to no ignores.
#[test]
fn is_ignored_all_empty_needles_not_ignored() {
    let ignores = vec!["".to_string(), "".to_string()];
    assert!(
        !is_ignored(Path::new("crates/maekon-lint/src/main.rs"), &ignores),
        "a list of only empty needles must not ignore anything"
    );
}
