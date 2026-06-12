//! Content-free logging helpers for window metadata (#5591).
//!
//! Window titles are PII (document names, mail subjects, URLs). The storage
//! path masks them through the PII filter, so tracing statements must not
//! bypass that by formatting the raw value into log lines — the file layer
//! persists logs to `{data_dir}/logs/` outside the consent/erasure machinery.
//! Log [`title_digest`] instead of the title itself; the
//! `forbid_raw_title_in_tracing_macros` test below enforces this for the whole
//! crate source tree (textual scan, so it also covers `#[cfg(target_os)]`
//! modules that do not compile on the host running the tests).

/// Content-free digest of a window title, safe for tracing logs.
///
/// Deliberately exposes only the character count: enough to tell empty from
/// populated titles while debugging, without persisting any content. A stable
/// hash is intentionally avoided — known titles could be confirmed against it
/// by dictionary lookup.
// Only the Linux and Windows active-window paths log titles today; keep the
// helper available (and its guard test running) on every platform.
#[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
pub(crate) fn title_digest(title: &str) -> String {
    format!("title_len={}", title.chars().count())
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
        assert_eq!(title_digest("한글 제목"), "title_len=5");
        assert_eq!(title_digest(""), "title_len=0");
    }

    const TRACING_MACROS: [&str; 5] = ["trace!", "debug!", "info!", "warn!", "error!"];
    // Argument shapes that interpolate a raw title value into a log line. Covers
    // both a local `title` binding AND struct field access like `window.title` /
    // `parsed.title` — the field-access form (#5638) slipped past the original
    // binding-only patterns, leaking macOS window titles for several cycles.
    const RAW_TITLE_PATTERNS: [&str; 9] = [
        ", title)", ", title,", ", &title", "%title", "?title", "{title}", ".title)", ".title,",
        ".title ",
    ];

    /// Crate-wide guard: no tracing macro may format a raw `title` value.
    /// Best-effort textual scan, multi-line aware (joins a macro call until
    /// its parentheses balance, so rustfmt-wrapped calls are covered too);
    /// routing titles through [`title_digest`] is the sanctioned way.
    #[test]
    fn forbid_raw_title_in_tracing_macros() {
        let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        visit(&src_dir, &mut violations);
        assert!(
            violations.is_empty(),
            "raw window title formatted in a tracing macro — log title_digest(&title) instead: {violations:?}"
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

    fn visit(dir: &Path, violations: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read src dir") {
            let path = entry.expect("dir entry").path();
            // This module holds raw-title string fixtures for the scanner's
            // own tests — exempt it from the walk (covered by review, and the
            // scanner behaviour itself is pinned by the pure-fn tests above).
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
            if !normalized.contains("title_digest")
                && RAW_TITLE_PATTERNS.iter().any(|p| normalized.contains(p))
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
