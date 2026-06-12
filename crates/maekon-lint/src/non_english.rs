use crate::finding::{Finding, Severity};
use crate::fs_scan::{collect_files, first_non_ascii, is_ignored, is_locale_file};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CODE_ROOT: &str = "crates";

pub(crate) fn scan_non_english_text(
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
