//! Workspace gate: no new value-blind `assert!(x.is_ok())` hedges (#5630).
//!
//! #5594 strengthened all 119 such hedges to `.expect()` + value assertions;
//! this gate keeps the count at zero. It runs as a normal test of the
//! `maekon-lint` package, so CI's `cargo test --workspace` fires it without
//! any extra pipeline wiring (ADR-075 P-4: no dead gates).
//!
//! Escape hatch: a genuinely-justified site may carry a
//! `lint:allow-is-ok-hedge` marker comment on the call; prefer
//! `.expect("intent")` plus value assertions instead.

use std::path::{Path, PathBuf};

const MARKER: &str = "lint:allow-is-ok-hedge";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

#[test]
fn no_is_ok_hedge_assertions_in_workspace() {
    let root = workspace_root();
    let mut violations = Vec::new();
    for dir in ["crates", "src-tauri"] {
        visit(&root.join(dir), &mut violations);
    }
    assert!(
        violations.is_empty(),
        "value-blind assert!(..is_ok()) hedge(s) found — use .expect(\"intent\") plus value \
         assertions (see #5594), or mark a justified site with `{MARKER}`: {violations:#?}"
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
        // This gate holds raw `assert!(..is_ok())` fixtures for its own
        // scanner tests — exempt it (scanner behaviour is pinned below).
        if name == "is_ok_hedge_gate.rs" {
            continue;
        }
        if path.is_dir() {
            visit(&path, violations);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source file");
            for line_no in find_is_ok_hedges(&source) {
                violations.push(format!("{}:{line_no}", path.display()));
            }
        }
    }
}

/// Pure scanner: 1-based line numbers of `assert!(...)` calls whose argument
/// text contains `.is_ok()`. Multi-line aware (joins the call until its
/// parentheses balance, mirroring the maekon-monitor log_privacy guard).
/// Skips `debug_assert!` (word boundary), commented-out call sites, and
/// calls carrying the allow marker.
fn find_is_ok_hedges(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let mut violations = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(pos) = find_assert_macro(lines[i]) else {
            i += 1;
            continue;
        };
        if lines[i][..pos].contains("//") {
            // Commented-out call (or one quoted inside a trailing comment).
            i += 1;
            continue;
        }
        // Join the call text from the macro name until parens balance.
        let mut call = String::new();
        let mut depth = 0i32;
        let mut opened = false;
        let mut j = i;
        'call: while j < lines.len() {
            let segment = if j == i { &lines[i][pos..] } else { lines[j] };
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
        // Marker accepted inside the call span, on the opening line, or on the
        // line directly above — the preceding-line form survives rustfmt
        // re-wrapping a single-line assert (a trailing same-line marker does not).
        let exempt = call.contains(MARKER)
            || lines[i].contains(MARKER)
            || (i > 0 && lines[i - 1].contains(MARKER));
        if normalize(&call).contains(".is_ok()") && !exempt {
            violations.push(i + 1);
        }
        i = j + 1;
    }
    violations
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
    let src = "    assert!(result.is_ok());\n";
    assert_eq!(find_is_ok_hedges(src), vec![1]);
}

#[test]
fn scanner_detects_rustfmt_wrapped_hedge() {
    let src = "assert!(\n    builder.finish().is_ok(),\n    \"message\"\n);\n";
    assert_eq!(find_is_ok_hedges(src), vec![1]);
}

#[test]
fn scanner_detects_negated_form() {
    // `assert!(!x.is_ok())` is an err-check in disguise — equally value-blind;
    // write `assert!(matches!(x, Err(..)))` against the concrete variant instead.
    let src = "assert!(!result.is_ok());\n";
    assert_eq!(find_is_ok_hedges(src), vec![1]);
}

#[test]
fn scanner_ignores_debug_assert_comments_and_marker() {
    let src = "debug_assert!(x.is_ok());\n\
               // assert!(quoted.is_ok()) — mentioned in a comment only\n\
               assert!(x.is_ok()); // lint:allow-is-ok-hedge — justified: <reason>\n\
               let ok = result.is_ok();\n";
    assert!(
        find_is_ok_hedges(src).is_empty(),
        "debug_assert / comments / marked sites must not be flagged"
    );
}

#[test]
fn scanner_ignores_expect_and_value_assertions() {
    let src = "    let v = result.expect(\"must parse\");\n    assert_eq!(v.len(), 3);\n";
    assert!(
        find_is_ok_hedges(src).is_empty(),
        "the sanctioned pattern must pass the gate"
    );
}

#[test]
fn scanner_accepts_preceding_line_marker_on_wrapped_call() {
    // rustfmt wraps long single-line asserts, splitting a trailing same-line
    // marker away from the `assert!(` opening line — the preceding-line form
    // is the fmt-stable spelling and must be honoured.
    let src = "    // lint:allow-is-ok-hedge — justified: <reason>\n\
               assert!(\n\
                   x.is_ok(),\n\
                   \"context message\"\n\
               );\n";
    assert!(
        find_is_ok_hedges(src).is_empty(),
        "preceding-line marker must exempt a wrapped call"
    );
}
