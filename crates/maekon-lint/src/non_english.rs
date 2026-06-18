use crate::finding::{Finding, Severity};
use crate::fs_scan::{collect_files, is_ignored};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_CODE_ROOT: &str = "crates";

/// True for letters that are acceptable in an English-first comment.
///
/// This gate targets non-ENGLISH *language* in COMMENTS (e.g. Korean/CJK
/// explanations in English-first client code), NOT non-ASCII typography. Em-dashes,
/// arrows, `x`-cross, middle dots, curly quotes, etc. are punctuation/symbols (not
/// alphabetic) and are always allowed. Allowed letters:
/// - Latin script, including accented/extended Latin (e-acute, n-tilde, u-umlaut) —
///   English-compatible.
/// - Greek letters and the micro sign — these appear as scientific/math notation in
///   English technical writing (e.g. `alpha (learning rate)` written as the symbol,
///   `O(microseconds)`), not as Greek prose.
///
/// Only alphabetic characters in other scripts (Hangul, CJK, Kana, Cyrillic, Arabic,
/// ...) count as genuinely non-English.
fn is_english_compatible_letter(ch: char) -> bool {
    if !ch.is_alphabetic() {
        return false;
    }
    let cp = ch as u32;
    ch.is_ascii_alphabetic()
        || cp == 0x00B5 // micro sign
        || (0x00C0..=0x02AF).contains(&cp) // Latin-1 Supplement + Latin Extended-A/B + IPA
        || (0x0370..=0x03FF).contains(&cp) // Greek and Coptic (math/scientific notation)
        || (0x1E00..=0x1EFF).contains(&cp) // Latin Extended Additional
}

/// Find the first genuinely non-English letter that appears inside a COMMENT
/// (`//`, `///`, `//!`, or a single-line `/* ... */`) on this line.
///
/// **Scope: comments only.** Non-Latin letters inside string/char literals or code
/// are deliberately NOT flagged. This gate enforces English-first *comments*;
/// runtime strings are a legitimate, separate concern and are intentionally
/// bilingual in places — e.g. localized UI text, or non-English keywords a
/// rule-based classifier matches against non-English window titles / user input
/// (a `hint.contains(...)` check against localized phrases). Flagging those would
/// force behavior changes or ugly `\u{...}` escaping of load-bearing values, so
/// they are out of scope here (an i18n/product concern tracked separately).
///
/// A file may opt out entirely with a `lint:allow-non-english-comments` marker
/// (used by files that document CJK/Korean text tokenization, where example tokens
/// are illustrative and translating them away would defeat the documentation).
///
/// The scan is string-literal aware (so `//` inside `"http://..."` is not mistaken
/// for a comment) and line-scoped. A `/* */` body that spans multiple lines is only
/// inspected on the line bearing `/*` — a deliberate approximation: the priority is
/// to never FALSE-POSITIVE on a Korean string literal, and comments are English, so
/// a missed block-comment continuation line is immaterial.
fn first_non_english_in_comment(line: &str) -> Option<(usize, char)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while i < chars.len() {
        let ch = chars[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            i += 1;
            continue;
        }
        // Line comment: everything to end-of-line is comment.
        if ch == '/' && chars.get(i + 1) == Some(&'/') {
            for (off, c) in chars[i + 2..].iter().enumerate() {
                if c.is_alphabetic() && !is_english_compatible_letter(*c) {
                    return Some((i + 2 + off + 1, *c));
                }
            }
            return None;
        }
        // Block comment: scan until the matching `*/` on this line.
        if ch == '/' && chars.get(i + 1) == Some(&'*') {
            let mut j = i + 2;
            while j < chars.len() {
                if chars[j] == '*' && chars.get(j + 1) == Some(&'/') {
                    break;
                }
                if chars[j].is_alphabetic() && !is_english_compatible_letter(chars[j]) {
                    return Some((j + 1, chars[j]));
                }
                j += 1;
            }
            i = if j + 1 < chars.len() {
                j + 2
            } else {
                chars.len()
            };
            continue;
        }
        i += 1;
    }
    None
}

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

        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };

        // File-level opt-out for files that legitimately require non-English in
        // comments — e.g. documentation of CJK/Korean text tokenization, where the
        // example tokens are illustrative and translating them away would defeat the
        // purpose. The marker should carry a one-line justification.
        if content.contains("lint:allow-non-english-comments") {
            continue;
        }

        for (line_idx, line) in content.lines().enumerate() {
            if let Some((col, ch)) = first_non_english_in_comment(line) {
                findings.push(Finding::new(
                    Severity::Error,
                    "non-english-comment",
                    file.clone(),
                    line_idx + 1,
                    col,
                    format!(
                        "Non-English text in comment: {:?} (comments must be English)",
                        ch
                    ),
                    line.trim().to_string(),
                ));
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::{first_non_english_in_comment, is_english_compatible_letter};

    // Non-ASCII test characters are written as `\u{...}` escapes so THIS source
    // file stays pure-ASCII and the scanner never flags its own test data.

    #[test]
    fn flags_non_english_in_line_comment() {
        let (col, ch) =
            first_non_english_in_comment("    let x = 1; // \u{D55C} comment").expect("flagged");
        assert_eq!(ch, '\u{D55C}'); // Hangul
        assert_eq!(col, 19);
        assert!(first_non_english_in_comment("/// \u{4E2D}\u{6587} doc").is_some()); // CJK doc
        assert!(first_non_english_in_comment("//! \u{3053} module").is_some()); // Kana inner-doc
    }

    #[test]
    fn flags_non_english_in_block_comment() {
        assert!(first_non_english_in_comment("let y = 2; /* \u{D55C} */ let z = 3;").is_some());
    }

    #[test]
    fn ignores_non_english_in_string_literals() {
        // Localized UI text / classifier match-keywords live in strings — allowed.
        assert!(first_non_english_in_comment("    let s = \"\u{D55C}\u{AE00}\";").is_none());
        assert!(
            first_non_english_in_comment("    if hint.contains(\"\u{D074}\u{B9AD}\") { }")
                .is_none()
        );
        // A `//` INSIDE a string must not be treated as a comment start.
        assert!(first_non_english_in_comment("let u = \"a//\u{D55C}b\";").is_none());
        // Test-data repeat literal — allowed (it is a string).
        assert!(first_non_english_in_comment("let b = \"\u{AC00}\".repeat(100);").is_none());
    }

    #[test]
    fn allows_english_comments_with_typography_and_accented_latin() {
        assert!(
            first_non_english_in_comment("// Single monitor \u{2014} capture display").is_none()
        ); // em-dash
        assert!(first_non_english_in_comment("// caf\u{00E9} r\u{00E9}sum\u{00E9}").is_none()); // accented Latin
        assert!(first_non_english_in_comment("let x = 1; // plain ascii").is_none());
        // Greek math symbols + micro sign are English-compatible scientific notation.
        assert!(
            first_non_english_in_comment("// \u{03B1} learning rate, \u{03B2} momentum").is_none()
        );
        assert!(first_non_english_in_comment("// latency O(\u{00B5}s)").is_none());
        assert!(is_english_compatible_letter('\u{00E9}')); // e-acute
        assert!(is_english_compatible_letter('\u{03B1}')); // Greek alpha
        assert!(is_english_compatible_letter('\u{00B5}')); // micro sign
        assert!(!is_english_compatible_letter('\u{D55C}')); // Hangul
        assert!(!is_english_compatible_letter('\u{4E2D}')); // CJK
    }
}
