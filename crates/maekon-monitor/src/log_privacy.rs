//! Content-free logging helpers for window metadata and suggestion content (#5591, #6006).
//!
//! Window titles and AI suggestion content are PII (document names, mail subjects,
//! URLs, inferred user activity). The storage path masks them through the PII
//! filter, so tracing statements must not bypass that by formatting raw values
//! into log lines — the file layer persists logs to `{data_dir}/logs/` outside
//! the consent/erasure machinery.
//! Log [`title_digest`] instead of raw window titles, and [`content_digest`]
//! instead of raw suggestion/notification content; the
//! `forbid_raw_title_in_tracing_macros` test below enforces this for ALL crates
//! in the workspace source tree (textual scan, so it also covers
//! `#[cfg(target_os)]` modules that do not compile on the host running the
//! tests).

/// Content-free digest of a window title, safe for tracing logs.
///
/// Deliberately exposes only the character count: enough to tell empty from
/// populated titles while debugging, without persisting any content. A stable
/// hash is intentionally avoided — known titles could be confirmed against it
/// by dictionary lookup.
// The helper is used by both maekon-monitor itself (Linux/Windows active-window
// paths) and by src-tauri scheduler/notification paths that must not log raw
// titles. Keep it available and un-gated on every platform.
pub fn title_digest(title: &str) -> String {
    format!("title_len={}", title.chars().count())
}

/// Content-free digest of suggestion or notification body text, safe for
/// tracing logs.
///
/// AI-generated suggestion content encodes information derived from the user's
/// active window and OCR text, making it indirectly PII. Log only the character
/// count so the log file layer cannot reconstruct the original text.
pub fn content_digest(content: &str) -> String {
    format!("content_len={}", content.chars().count())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn digest_is_content_free() {
        let digest = title_digest("Q3 layoffs draft - CONFIDENTIAL");
        assert!(!digest.contains("CONFIDENTIAL"));
        assert_eq!(digest, "title_len=31");
    }

    #[test]
    fn digest_counts_unicode_chars_not_bytes() {
        // Multi-byte CJK input (4 Hangul chars + 1 space) written as \u escapes to
        // keep the source ASCII; the digest must count 5 chars, not the 13 UTF-8
        // bytes.
        assert_eq!(
            title_digest("\u{D55C}\u{AE00} \u{C81C}\u{BAA9}"),
            "title_len=5"
        );
        assert_eq!(title_digest(""), "title_len=0");
    }

    #[test]
    fn content_digest_is_content_free() {
        let d = content_digest("Take a 5-minute break — you have been coding for 2 hours");
        assert!(!d.contains("break"));
        assert!(d.starts_with("content_len="));
    }

    const TRACING_MACROS: [&str; 5] = ["trace!", "debug!", "info!", "warn!", "error!"];
    // Argument shapes that interpolate a raw title value into a log line. Covers
    // both a local `title` binding AND struct field access like `window.title` /
    // `parsed.title` — the field-access form (#5638) slipped past the original
    // binding-only patterns, leaking macOS window titles for several cycles.
    // The `window_title` field form (#29) is its own family: `%window_title`
    // structured-field syntax and positional `, window_title)` / `, window_title,`
    // args slip past the `title`/`%title` patterns above because of the
    // `window_` prefix. Cover the named-field form explicitly.
    const RAW_TITLE_PATTERNS: [&str; 12] = [
        ", title)",
        ", title,",
        ", &title",
        "%title",
        "?title",
        "{title}",
        ".title)",
        ".title,",
        ".title ",
        "%window_title",
        ", window_title)",
        ", window_title,",
    ];
    // Patterns that interpolate raw suggestion/notification body content.
    // `suggestion.content` is PII-adjacent: it is generated from OCR text and
    // window titles captured from the user's screen.
    const RAW_CONTENT_PATTERNS: [&str; 5] = [
        "suggestion.content",
        ".content)",
        ".content,",
        ", body)",
        ", body,",
    ];

    /// Workspace-wide guard: no tracing macro in ANY crate may format a raw
    /// `title` or suggestion/notification `content`/`body` value.
    /// Best-effort textual scan, multi-line aware (joins a macro call until
    /// its parentheses balance, so rustfmt-wrapped calls are covered too);
    /// routing titles through [`title_digest`] and content through
    /// [`content_digest`] is the sanctioned way.
    ///
    /// The scan covers ALL crates under `clients/maekon-client/crates/` and the
    /// `src-tauri/src/` tree, not just maekon-monitor, so that analysis,
    /// suggestion, and notification paths cannot leak raw content (#6006).
    #[test]
    fn forbid_raw_title_in_tracing_macros() {
        // Walk the workspace root (two levels above this crate's Cargo.toml).
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        // CARGO_MANIFEST_DIR = .../clients/maekon-client/crates/maekon-monitor
        let workspace_root = manifest_dir
            .parent() // crates/
            .and_then(Path::parent) // maekon-client/
            .expect("workspace root");

        let scan_roots = [
            workspace_root.join("crates"),
            workspace_root.join("src-tauri").join("src"),
        ];

        let mut violations = Vec::new();
        for root in &scan_roots {
            if root.is_dir() {
                visit(root, &mut violations);
            }
        }
        assert!(
            violations.is_empty(),
            "raw window title or suggestion content formatted in a tracing macro — \
             use title_digest()/content_digest() instead: {violations:?}"
        );
    }

    /// The scanner must catch rustfmt-wrapped (multi-line) raw-title calls.
    #[test]
    fn scanner_detects_multiline_raw_title() {
        let wrapped = "debug!(\n    \"active window: {} - {}\",\n    app_name,\n    title\n);\n";
        assert_eq!(find_violations(wrapped), vec![1]);

        let digested =
            "debug!(\n    \"active window: {} ({})\",\n    app_name,\n    title_digest(&title)\n);\n";
        assert!(find_violations(digested).is_empty());

        let single = "    debug!(\"win: {} - {}\", app, title);\n";
        assert_eq!(find_violations(single), vec![1]);
    }

    /// Field-access form (`window.title` / `parsed.title`) is the macOS shape
    /// that escaped the binding-only patterns (#5638) — must be caught now.
    #[test]
    fn scanner_detects_struct_field_title() {
        let positional =
            "        debug!(\n            \"active window: {} - {}\",\n            w.app_name, w.title\n        );\n";
        assert_eq!(find_violations(positional), vec![1]);

        let with_pid = "    debug!(\"skip: {} - {} (pid={})\", p.app_name, parsed.title, p.pid);\n";
        assert_eq!(find_violations(with_pid), vec![1]);

        // The digested field-access form must NOT be flagged.
        let digested =
            "    debug!(\"active window: {} ({})\", w.app_name, title_digest(&w.title));\n";
        assert!(find_violations(digested).is_empty());
    }

    /// Raw suggestion content patterns must be caught by the scanner.
    #[test]
    fn scanner_detects_raw_suggestion_content() {
        let raw = "    info!(id = %s.id, \"suggestion: {}\", s.content);\n";
        assert_eq!(find_violations(raw), vec![1]);

        let digested =
            "    info!(id = %s.id, content = %content_digest(&s.content), \"suggestion logged\");\n";
        assert!(find_violations(digested).is_empty());
    }

    fn visit(dir: &Path, violations: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            // This module holds raw-title/content string fixtures for the
            // scanner's own tests — exempt it from the walk (covered by review,
            // and the scanner behaviour itself is pinned by the pure-fn tests
            // above).
            if path
                .file_name()
                .is_some_and(|name| name == "log_privacy.rs")
            {
                continue;
            }
            if path.is_dir() {
                visit(&path, violations);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).expect("read source file");
                for line_no in find_violations(&source) {
                    violations.push(format!("{}:{line_no}", path.display()));
                }
            }
        }
    }

    /// Pure scanner: returns 1-based line numbers of tracing calls that
    /// interpolate a raw `title` binding.
    fn find_violations(source: &str) -> Vec<usize> {
        let lines: Vec<&str> = source.lines().collect();
        let mut violations = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let Some(macro_pos) = TRACING_MACROS.iter().filter_map(|m| lines[i].find(m)).min()
            else {
                i += 1;
                continue;
            };
            // Join the call text from the macro name until parens balance.
            let mut call = String::new();
            let mut depth = 0i32;
            let mut opened = false;
            let mut j = i;
            'call: while j < lines.len() {
                let segment = if j == i {
                    &lines[i][macro_pos..]
                } else {
                    lines[j]
                };
                for ch in segment.chars() {
                    call.push(ch);
                    match ch {
                        '(' => {
                            depth += 1;
                            opened = true;
                        }
                        ')' => depth -= 1,
                        _ => {}
                    }
                    if opened && depth == 0 {
                        break 'call;
                    }
                }
                call.push(' ');
                j += 1;
            }
            let normalized = normalize(&call);
            // `title_digest`/`content_digest` reduce to a length-only digest;
            // `sanitize_title_with_level` (maekon-vision's privacy filter) masks
            // PII patterns. Both are sanctioned masking sinks — a value derived
            // from one is no longer a raw title/content interpolation (#29).
            let already_safe = normalized.contains("title_digest")
                || normalized.contains("content_digest")
                || normalized.contains("sanitize_title_with_level");
            if !already_safe
                && (RAW_TITLE_PATTERNS.iter().any(|p| normalized.contains(p))
                    || RAW_CONTENT_PATTERNS.iter().any(|p| normalized.contains(p)))
            {
                violations.push(i + 1);
            }
            i = j + 1;
        }
        violations
    }

    /// Collapse whitespace runs and detach it from `,`/`)` so wrapped call
    /// text matches the same patterns as a single-line call.
    fn normalize(call: &str) -> String {
        call.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace(" )", ")")
            .replace(" ,", ",")
    }
}
