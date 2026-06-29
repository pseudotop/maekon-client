use crate::finding::{Finding, Severity};
use crate::fs_scan::{collect_files, is_ignored};
use std::fs;
use std::path::{Path, PathBuf};

/// Code roots scanned when the caller passes no explicit `--path`.
///
/// Mirrors the `["crates", "src-tauri"]` set the workspace hedge gates walk
/// (see `is_ok_hedge_gate.rs`). `src-tauri` is the active Tauri main binary and
/// holds the largest body of first-party app logic (DI wiring, the loop
/// scheduler, Tauri IPC commands), so the documented local helper
/// (`scripts/check-language.sh`, which forwards no `--path`) and the tool's
/// intrinsic default must both cover it — not `crates/` only. Defaulting to
/// `crates/` alone silently returned a misleading "clean" for any non-English
/// comment anywhere under `src-tauri/src/**` (#7081 MLINT-3).
const DEFAULT_CODE_ROOTS: &[&str] = &["crates", "src-tauri"];

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
///
/// Test-only Rust-mode convenience wrapper; production scanning calls
/// `first_non_english_in_comment_impl` directly with the per-file `js_like` flag.
#[cfg(test)]
fn first_non_english_in_comment(line: &str) -> Option<(usize, char)> {
    // Rust/default mode: only double-quoted strings are tracked (`'` denotes char
    // literals and lifetimes in Rust, not strings).
    first_non_english_in_comment_impl(line, false)
}

/// `js_like`: when true, single-quoted (`'...'`) and template-literal (`` `...` ``)
/// strings are ALSO treated as string literals, so a `//` inside a JS/TS string
/// (e.g. `'http://...'`) is not mistaken for a comment start. Disabled for Rust,
/// where `'` denotes char literals / lifetimes (`'static`) rather than strings.
fn first_non_english_in_comment_impl(line: &str, js_like: bool) -> Option<(usize, char)> {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    // The active string delimiter (`"`, or `'`/`` ` `` in js_like mode), or None.
    let mut string_delim: Option<char> = None;
    let mut escaped = false;
    while i < chars.len() {
        let ch = chars[i];
        if let Some(delim) = string_delim {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delim {
                string_delim = None;
            }
            i += 1;
            continue;
        }
        if ch == '"' || (js_like && (ch == '\'' || ch == '`')) {
            string_delim = Some(ch);
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
        DEFAULT_CODE_ROOTS
            .iter()
            .map(|root| repo_root.join(root))
            .collect()
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

        // JS-family files use `'` and `` ` `` for strings; Rust uses `'` for char
        // literals / lifetimes, so single-quote string-tracking must stay off there.
        let js_like = matches!(
            file.extension().and_then(|e| e.to_str()),
            Some("ts" | "tsx" | "js" | "jsx")
        );

        for (line_idx, line) in content.lines().enumerate() {
            if let Some((col, ch)) = first_non_english_in_comment_impl(line, js_like) {
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
    use super::{
        first_non_english_in_comment, first_non_english_in_comment_impl,
        is_english_compatible_letter,
    };

    #[test]
    fn js_mode_tracks_single_quote_and_template_strings() {
        // JS mode: `//` inside single-quote / template strings is NOT a comment, so
        // non-English inside such a string must not be flagged (the false positive).
        assert!(
            first_non_english_in_comment_impl("const u = 'http://\u{D55C}.example';", true)
                .is_none()
        );
        assert!(first_non_english_in_comment_impl("const t = `http://\u{D55C}`;", true).is_none());
        // A genuine JS line comment with non-English is still flagged in JS mode.
        assert!(first_non_english_in_comment_impl("const x = 1; // \u{D55C}", true).is_some());
        // Rust mode: `'` is a lifetime/char, NOT a string — a real comment after a
        // lifetime must still be scanned (no regression for `.rs`).
        assert!(first_non_english_in_comment_impl("fn f<'a>() {} // \u{D55C}", false).is_some());
    }

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
    fn default_roots_scan_both_crates_and_src_tauri() {
        // #7081 MLINT-3: with no `--path`, the scanner must walk BOTH `crates/`
        // and `src-tauri/` (the active Tauri main binary), matching the hedge
        // gates — not `crates/` only. A non-English comment planted under each
        // root must be reported.
        use std::fs;
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        for sub in ["crates/pkg/src", "src-tauri/src"] {
            fs::create_dir_all(root.join(sub)).expect("mkdir");
        }
        // Hangul (\u{D55C}) inside a line comment under each root.
        fs::write(
            root.join("crates/pkg/src/a.rs"),
            "// \u{D55C} comment\nfn a() {}\n",
        )
        .expect("write crates file");
        fs::write(
            root.join("src-tauri/src/b.rs"),
            "// \u{D55C} comment\nfn b() {}\n",
        )
        .expect("write src-tauri file");

        let findings = super::scan_non_english_text(root, &[], &[]);
        let paths: Vec<String> = findings
            .iter()
            .map(|f| f.path.to_string_lossy().into_owned())
            .collect();
        assert!(
            paths.iter().any(|p| p.ends_with("crates/pkg/src/a.rs")),
            "crates/ must be scanned by default: {paths:?}"
        );
        assert!(
            paths.iter().any(|p| p.ends_with("src-tauri/src/b.rs")),
            "src-tauri/ must be scanned by default (#7081 MLINT-3): {paths:?}"
        );
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
