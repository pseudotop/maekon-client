//! Workspace gate: no new value-blind `assert!(x.is_err())` hedges (#5631).
//!
//! Tranche 1-3 of #5631 strengthened all such hedges to `.unwrap_err()` plus
//! variant/message assertions; this gate keeps the count at zero going forward.
//! It runs as a normal test of the `maekon-lint` package, so CI's
//! `cargo test --workspace` fires it without any extra pipeline wiring
//! (ADR-075 P-4: no dead gates).
//!
//! Escape hatch: a genuinely-justified site (e.g. `Result<_, ()>` where the
//! error type carries no discriminating payload) may carry a
//! `lint:allow-is-err-hedge` marker comment on the call; prefer
//! `.unwrap_err()` plus variant/message assertions instead.

use std::path::{Path, PathBuf};

const MARKER: &str = "lint:allow-is-err-hedge";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

#[test]
fn no_is_err_hedge_assertions_in_workspace() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for dir in ["crates", "src-tauri"] {
        visit(&root.join(dir), &mut violations);
    }
    assert!(
        violations.is_empty(),
        "value-blind assert!(..is_err()) hedge(s) found — use .unwrap_err() plus variant/message \
         assertions (see #5631), or mark a justified site with `{MARKER}`: {violations:#?}"
    );
}

fn visit(dir: &Path, violations: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // tolerated: optional dirs may be absent in sparse layouts
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Non-source trees; `frontend` is the React app (no .rs, huge).
        if name == "target" || name == "node_modules" || name == "frontend" || name == "gen" {
            continue;
        }
        // This gate holds raw `assert!(..is_err())` fixtures for its own
        // scanner tests — exempt it (scanner behaviour is pinned below).
        // Also exempt the is_ok gate which may mention the pattern in comments.
        if name == "is_err_hedge_gate.rs" || name == "is_ok_hedge_gate.rs" {
            continue;
        }
        if path.is_dir() {
            visit(&path, violations);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source file");
            for line_no in find_is_err_hedges(&source) {
                violations.push(format!("{}:{line_no}", path.display()));
            }
        }
    }
}

/// Pure scanner: 1-based line numbers of `assert!(...)` calls whose argument
/// text contains `.is_err()`. Multi-line aware (joins the call until its
/// parentheses balance, mirroring the is_ok gate).
/// Skips `debug_assert!` (word boundary), commented-out call sites, and
/// calls carrying the allow marker.
///
/// Fix #12: the pre-assert `//` comment check is string-literal aware — a `//`
/// that appears inside a double-quoted string literal is NOT treated as a line
/// comment start.  Example: `let u="http://x"; assert!(parse(u).is_err());`
/// must NOT be skipped.
///
/// Fix #13: after consuming a balanced `assert!(...)` call the scanner resumes
/// from just after its closing `)` on the same line, so a second `assert!` on
/// the same line is also inspected.
fn find_is_err_hedges(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let mut violations = Vec::new();
    let mut i = 0;
    // `col_offset` tracks where on line `i` to start scanning (supports #13:
    // multiple assert! calls on the same line).
    let mut col_offset = 0usize;
    while i < lines.len() {
        let line = lines[i];
        let search_from = col_offset;
        col_offset = 0; // reset; will be set again only for same-line continue

        let Some(rel_pos) = find_assert_macro(&line[search_from..]) else {
            i += 1;
            continue;
        };
        let pos = search_from + rel_pos;

        // Fix #12: only treat `//` as a comment start when it is NOT inside a
        // string literal.  Walk the prefix [0..pos] tracking whether we are
        // inside a double-quoted string (handling `\"` escapes).
        if comment_before(line, pos) {
            // The `assert!(` is commented out; skip the rest of this line.
            i += 1;
            continue;
        }

        // Join the call text from the macro name until parens balance.
        let mut call = String::new();
        let mut depth = 0i32;
        let mut opened = false;
        let mut j = i;
        // `end_col` will be set to the byte index just after the closing `)`.
        let mut end_col = 0usize;
        // Track string/char literals so a paren INSIDE a literal (e.g. `run(")")`
        // or `parse_paren(')')`) does not balance the depth and terminate the join
        // before `.is_err()` is seen (review4: false-negative). `in_string` is
        // escape-aware; a char-literal paren is recognized as `'('`/`')'` via the
        // surrounding quotes — lifetime-safe, since a lifetime like `'a` never has a
        // closing quote immediately after a paren.
        let mut in_string = false;
        let mut prev_ch: Option<char> = None;
        'call: while j < lines.len() {
            let segment = if j == i { &lines[i][pos..] } else { lines[j] };
            let mut chars = segment.char_indices().peekable();
            while let Some((byte_idx, ch)) = chars.next() {
                call.push(ch);
                if in_string {
                    match ch {
                        '\\' => {
                            // Consume the escaped char so `\"` does not close the string.
                            if let Some((_, esc)) = chars.next() {
                                call.push(esc);
                            }
                        }
                        '"' => in_string = false,
                        _ => {}
                    }
                    prev_ch = Some(ch);
                    continue;
                }
                match ch {
                    '"' => in_string = true,
                    '(' | ')' => {
                        let next_is_quote = chars.peek().map(|(_, c)| *c) == Some('\'');
                        let in_char_literal = prev_ch == Some('\'') && next_is_quote;
                        if !in_char_literal {
                            if ch == '(' {
                                depth += 1;
                                opened = true;
                            } else {
                                depth -= 1;
                            }
                            if opened && depth == 0 {
                                // byte_idx is relative to `segment`; compute absolute col.
                                end_col = if j == i {
                                    pos + byte_idx + ch.len_utf8()
                                } else {
                                    byte_idx + ch.len_utf8()
                                };
                                break 'call;
                            }
                        }
                    }
                    _ => {}
                }
                prev_ch = Some(ch);
            }
            call.push(' ');
            prev_ch = Some(' ');
            j += 1;
        }
        // Marker accepted inside the call span, on the opening line, or on the
        // line directly above — the preceding-line form survives rustfmt
        // re-wrapping a single-line assert (a trailing same-line marker does not).
        let exempt = call.contains(MARKER)
            || lines[i].contains(MARKER)
            || (i > 0 && lines[i - 1].contains(MARKER));
        if normalize(&call).contains(".is_err()") && !exempt {
            violations.push(i + 1);
        }

        // Fix #13: if the closing `)` was on the same starting line, resume
        // scanning that line from just after it rather than jumping to j+1.
        if j == i && end_col < line.len() {
            col_offset = end_col;
            // Stay on the same line (do NOT increment i).
        } else {
            i = j + 1;
        }
    }
    violations
}

/// Returns `true` when there is a `//` comment-start before byte offset `pos`
/// on `line` that is NOT inside a double-quoted string literal.
///
/// Handles:
/// - `\"` escape sequences inside strings (does not prematurely close the string)
/// - `\\` escape sequences (a backslash that is itself escaped, so the next
///   char is NOT part of an escape)
/// - Single-quoted char literals (e.g. `'\''`) are NOT handled — they are rare
///   before an `assert!(` and never contain `//` in practice.
fn comment_before(line: &str, pos: usize) -> bool {
    let prefix = &line[..pos];
    let mut in_string = false;
    let mut chars = prefix.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if !in_string => in_string = true,
            '"' if in_string => in_string = false,
            '\\' if in_string => {
                // Consume the escaped character so `\"` does not close the string.
                chars.next();
            }
            '/' if !in_string => {
                if chars.peek() == Some(&'/') {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Position of a word-boundary `assert!(` on the line (rejects identifiers
/// merely ending in `assert`, e.g. `debug_assert!(`).
fn find_assert_macro(line: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = line[from..].find("assert!(") {
        let pos = from + rel;
        let boundary_ok = pos == 0
            || !line[..pos]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if boundary_ok {
            return Some(pos);
        }
        from = pos + "assert!(".len();
    }
    None
}

/// Collapse whitespace and detach it from punctuation so rustfmt-wrapped
/// call text matches the same substring patterns as a single-line call.
fn normalize(call: &str) -> String {
    call.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" (", "(")
        .replace("( ", "(")
        .replace(" )", ")")
        .replace(" .", ".")
        .replace(". ", ".")
}

// ── Scanner self-tests (the guard's guard) ─────────────────────────────────

#[test]
fn scanner_detects_single_line_hedge() {
    let src = "    assert!(result.is_err());\n";
    assert_eq!(find_is_err_hedges(src), vec![1]);
}

#[test]
fn scanner_detects_rustfmt_wrapped_hedge() {
    let src = "assert!(\n    builder.finish().is_err(),\n    \"message\"\n);\n";
    assert_eq!(find_is_err_hedges(src), vec![1]);
}

#[test]
fn scanner_detects_negated_form() {
    // `assert!(!x.is_err())` is an ok-check in disguise — equally value-blind;
    // write `assert!(matches!(x, Ok(..)))` against the concrete value instead.
    let src = "assert!(!result.is_err());\n";
    assert_eq!(find_is_err_hedges(src), vec![1]);
}

#[test]
fn scanner_ignores_debug_assert_comments_and_marker() {
    let src = "debug_assert!(x.is_err());\n\
               // assert!(quoted.is_err()) — mentioned in a comment only\n\
               assert!(x.is_err()); // lint:allow-is-err-hedge — justified: <reason>\n\
               let ok = result.is_err();\n";
    assert!(
        find_is_err_hedges(src).is_empty(),
        "debug_assert / comments / marked sites must not be flagged"
    );
}

#[test]
fn scanner_ignores_unwrap_err_and_variant_assertions() {
    let src = "    let e = result.unwrap_err();\n    assert!(matches!(e, MyError::Variant(_)));\n";
    assert!(
        find_is_err_hedges(src).is_empty(),
        "the sanctioned pattern must pass the gate"
    );
}

#[test]
fn scanner_accepts_preceding_line_marker_on_wrapped_call() {
    // rustfmt wraps long single-line asserts, splitting a trailing same-line
    // marker away from the `assert!(` opening line — the preceding-line form
    // is the fmt-stable spelling and must be honoured.
    let src = "    // lint:allow-is-err-hedge — justified: <reason>\n\
               assert!(\n\
                   x.is_err(),\n\
                   \"context message\"\n\
               );\n";
    assert!(
        find_is_err_hedges(src).is_empty(),
        "preceding-line marker must exempt a wrapped call"
    );
}

// ── Fix #12: `//` inside a string literal must NOT be treated as a comment ──

#[test]
fn scanner_not_fooled_by_url_in_string_before_assert() {
    // A `//` that appears inside a string literal (e.g. a URL) must not cause
    // the following `assert!(..is_err())` to be skipped as if it were commented
    // out.  This was the pre-fix bug: the prefix `let u="http://x"; ` contains
    // `//` and the whole line was wrongly exempted.
    let src = "    let u = \"http://example.com\"; assert!(parse(u).is_err());\n";
    assert_eq!(
        find_is_err_hedges(src),
        vec![1],
        "assert!(..is_err()) after a string-literal containing `//` must be flagged"
    );
}

// ── review4: a `)` inside a string/char literal must not balance the call ──

#[test]
fn scanner_sees_is_err_through_paren_in_string_literal() {
    // A `)` inside a string literal must not decrement paren depth and terminate
    // the join before `.is_err()` is seen (pre-fix false-negative).
    let src = "    assert!(run(\")\").is_err());\n";
    assert_eq!(
        find_is_err_hedges(src),
        vec![1],
        "is_err() after a string-literal paren must be flagged"
    );
}

#[test]
fn scanner_sees_is_err_through_paren_in_char_literal() {
    // A `)` inside a char literal `')'` must not balance the parens early.
    let src = "    assert!(parse_paren(')').is_err());\n";
    assert_eq!(
        find_is_err_hedges(src),
        vec![1],
        "is_err() after a char-literal paren must be flagged"
    );
}

#[test]
fn scanner_lifetime_does_not_false_positive() {
    // A lifetime `'a` is not a char literal and must not derail the scan: this
    // assert has no `.is_err()`, so nothing should be flagged.
    let src = "    assert!(make::<&'a str>(x).is_some());\n";
    assert!(
        find_is_err_hedges(src).is_empty(),
        "lifetime tick must not cause a spurious flag"
    );
}

// ── Fix #13: a second `assert!` on the same line must also be inspected ─────

#[test]
fn scanner_catches_second_assert_on_same_line() {
    // Two assert! calls on one line: the first is a safe assertion (assert_eq!
    // style, no .is_err()), the second is a value-blind hedge.  Pre-fix, the
    // scanner consumed the first assert!(...) and then jumped to the next line,
    // silently missing the second.
    let src = "    assert!(true); assert!(result.is_err());\n";
    assert_eq!(
        find_is_err_hedges(src),
        vec![1],
        "second assert!(..is_err()) on the same line must be flagged"
    );
}
