//! # maekon-lint
//!
//! Workspace lint tool (`language-check` binary). Validates coding conventions,
//! naming patterns, and architecture rules across the workspace.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

mod args;
mod finding;
mod fs_scan;
mod i18n;
mod non_english;
mod report;

#[cfg(test)]
mod cli_tests;
#[cfg(test)]
mod i18n_tests;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use args::{collect_option_values, parse_mode, print_help};
use finding::{Finding, Mode, Severity};
use i18n::scan_i18n;
use non_english::scan_non_english_text;
use report::print_summary;

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
    // --strict-i18n promotes *hardcoded UI copy* to a build failure (the "no
    // untranslated UI strings" policy). It deliberately does NOT fail on
    // `template-literal-i18n-key` (dynamic `t(`ns.${x}`)` keys are legitimate and
    // cannot be statically resolved) or `extra-locale-key` (informational), which
    // would otherwise make the strict gate impossible to ever satisfy.
    let has_strict_warnings = strict_i18n
        && findings
            .iter()
            .any(|f| f.severity == Severity::Warning && f.category == "hardcoded-ui-copy");

    if has_errors || has_strict_warnings {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
