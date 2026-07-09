//! Workspace gate: forbid raw `.effective_permissions()` composition in
//! `src-tauri` (#7728 — ctd-W2 E7).
//!
//! Before `ConsentGate` (`maekon_core::ports::consent_manager::ConsentGate`),
//! 17 non-test src-tauri files hand-composed
//! `consent_manager.as_ref().map(|cm| cm.effective_permissions()...).unwrap_or(...)`
//! (or an equivalent), and answered "what happens when there is no
//! ConsentManager installed at all?" THREE different ways for the telemetry
//! permission alone: `scheduler/config.rs` defaulted OPEN (`is_none_or` →
//! `true`, the actual bug), `integration_runtime.rs` defaulted CLOSED, and
//! several other sites defaulted to an all-false `ConsentPermissions`
//! snapshot. `ConsentGate` centralizes this into ONE, tested, fail-closed
//! answer per permission tier — this gate keeps that count at zero going
//! forward so the divergence cannot silently return.
//!
//! It runs as a normal test of the `maekon-lint` package, so CI's
//! `cargo test --workspace` fires it without any extra pipeline wiring
//! (ADR-075 P-4: no dead gates).
//!
//! Scope: `src-tauri/src` only. Whole-file test modules (the ADR-003
//! `tests.rs` convention) and the trailing `#[cfg(test)] mod ... { ... }`
//! block of every other file are exempt — legitimately testing the port's OWN
//! `effective_permissions()` accessor against a constructed/mock manager is
//! not "composition" in the sense this gate forbids (there is no
//! `Option`-defaulting question at a test call site that already holds a
//! concrete manager). `maekon-core` (where `ConsentManagerPort` and
//! `ConsentGate` are DEFINED, not called) is out of scope entirely.
//!
//! Escape hatch: a genuinely-justified direct call (e.g. diagnostic tooling
//! that already holds a guaranteed-present manager and needs the raw,
//! non-named-query snapshot) may carry a
//! `lint:allow-effective-permissions-composition` marker comment on the call
//! line or the line directly above.

use std::path::{Path, PathBuf};

const MARKER: &str = "lint:allow-effective-permissions-composition";
const TARGET: &str = "effective_permissions(";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

#[test]
fn no_raw_effective_permissions_composition_in_src_tauri() {
    let root = workspace_root();
    let mut violations = Vec::new();
    visit(&root.join("src-tauri/src"), &mut violations);
    assert!(
        violations.is_empty(),
        "raw `.effective_permissions()` call(s) found in src-tauri outside the sanctioned \
         ConsentGate (#7728) — route through `maekon_core::ports::consent_manager::ConsentGate` \
         (Option-defaulting call sites) or a `*_permitted()` default method on \
         `ConsentManagerPort` (call sites that already hold a guaranteed-present manager), or \
         mark a justified direct call with `{MARKER}`: {violations:#?}"
    );
}

fn visit(dir: &Path, violations: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // tolerated: caller passes a fixed known-good path in the real gate
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        // Non-source trees.
        if name == "target" || name == "node_modules" || name == "gen" {
            continue;
        }
        if path.is_dir() {
            visit(&path, violations);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            // ADR-003 whole-file test modules (`tests.rs`) legitimately
            // construct manager fixtures and assert on `effective_permissions()`
            // directly — not a composition site.
            if name == "tests.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source file");
            for line_no in find_violations(&source) {
                violations.push(format!("{}:{line_no}", path.display()));
            }
        }
    }
}

/// Pure scanner: 1-based line numbers of a raw `effective_permissions(` call
/// found OUTSIDE:
/// - a `//` line comment (doc or otherwise; string-literal `//` aware, see
///   `comment_before`)
/// - the trailing `#[cfg(test)] mod ... { ... }` region of the file (this
///   codebase's documented convention places ALL test modules there — see
///   `clients/maekon-client/CLAUDE.md` "Testing: Write in `#[cfg(test)] mod
///   tests` at the bottom of each module" — so truncating the scan at the
///   FIRST bare `#[cfg(test)]` line is safe for every file in this workspace)
/// - a line carrying the `lint:allow-effective-permissions-composition`
///   marker (same line or the line directly above)
fn find_violations(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let test_boundary = lines
        .iter()
        .position(|l| l.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());

    let mut violations = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx >= test_boundary {
            break;
        }
        let Some(pos) = line.find(TARGET) else {
            continue;
        };
        if comment_before(line, pos) {
            continue;
        }
        let exempt = line.contains(MARKER) || (idx > 0 && lines[idx - 1].contains(MARKER));
        if exempt {
            continue;
        }
        violations.push(idx + 1);
    }
    violations
}

/// Returns `true` when there is a `//` comment-start before byte offset `pos`
/// on `line` that is NOT inside a double-quoted string literal. Mirrors the
/// identically-named helper in `is_err_hedge_gate.rs`.
fn comment_before(line: &str, pos: usize) -> bool {
    let prefix = &line[..pos];
    let mut in_string = false;
    let mut chars = prefix.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '"' if !in_string => in_string = true,
            '"' if in_string => in_string = false,
            '\\' if in_string => {
                chars.next();
            }
            '/' if !in_string && chars.peek() == Some(&'/') => return true,
            _ => {}
        }
    }
    false
}

// ── Scanner self-tests (the guard's guard) ─────────────────────────────────

#[test]
fn scanner_flags_raw_map_unwrap_or_default_composition() {
    let src = "let consent = consent_manager.as_ref().map(|cm| cm.effective_permissions()).unwrap_or_default();\n";
    assert_eq!(find_violations(src), vec![1]);
}

#[test]
fn scanner_flags_fail_open_is_none_or_composition() {
    // The exact pre-fix `scheduler/config.rs` shape — this is THE bug this
    // gate exists to make revert-proof.
    let src =
        "self.consent_manager.as_ref().is_none_or(|cm| cm.effective_permissions().telemetry)\n";
    assert_eq!(find_violations(src), vec![1]);
}

#[test]
fn scanner_ignores_line_comment_mention() {
    let src = "// effective_permissions() returns permissions only in the Valid state\n";
    assert!(find_violations(src).is_empty());
}

#[test]
fn scanner_ignores_doc_comment_mention() {
    let src = "/// `consent` snapshot (`ConsentManager::effective_permissions()`), which is\n";
    assert!(find_violations(src).is_empty());
}

#[test]
fn scanner_ignores_trailing_cfg_test_region() {
    let src =
        "let x = 1;\n#[cfg(test)]\nmod tests {\n    fn t() { mgr.effective_permissions(); }\n}\n";
    assert!(find_violations(src).is_empty());
}

#[test]
fn scanner_flags_production_code_before_cfg_test_boundary() {
    let src = "fn gate() -> bool { mgr.effective_permissions().telemetry }\n\n#[cfg(test)]\nmod tests {}\n";
    assert_eq!(find_violations(src), vec![1]);
}

#[test]
fn scanner_respects_marker_on_same_line() {
    let src = "manager.effective_permissions(), // lint:allow-effective-permissions-composition — guaranteed-present manager\n";
    assert!(find_violations(src).is_empty());
}

#[test]
fn scanner_respects_marker_on_preceding_line() {
    let src = "// lint:allow-effective-permissions-composition — guaranteed-present manager\nmanager.effective_permissions(),\n";
    assert!(find_violations(src).is_empty());
}

#[test]
fn scanner_not_fooled_by_string_literal_containing_double_slash() {
    let src = "let u = \"http://x\"; let p = mgr.effective_permissions();\n";
    assert_eq!(
        find_violations(src),
        vec![1],
        "a `//` inside a string literal earlier on the line must not be treated as a comment start"
    );
}

// ── End-to-end fixture: a fresh violation is caught via the real file walk ──

#[test]
fn end_to_end_flags_violation_via_tempdir_fixture() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_dir = tmp.path().join("src-tauri").join("src");
    std::fs::create_dir_all(&src_dir).expect("mkdir src-tauri/src");
    std::fs::write(
        src_dir.join("example.rs"),
        "fn gate(cm: Option<&std::sync::Arc<dyn Trait>>) -> bool {\n    \
         cm.map(|c| c.effective_permissions().telemetry).unwrap_or(false)\n\
         }\n",
    )
    .expect("write example.rs");

    let mut violations = Vec::new();
    visit(&src_dir, &mut violations);
    assert_eq!(
        violations.len(),
        1,
        "the raw composition line must be flagged"
    );
    assert!(violations[0].ends_with("example.rs:2"));
}

#[test]
fn end_to_end_clean_fixture_reports_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_dir = tmp.path().join("src-tauri").join("src");
    std::fs::create_dir_all(&src_dir).expect("mkdir src-tauri/src");
    std::fs::write(
        src_dir.join("example.rs"),
        "fn gate(cm: Option<&std::sync::Arc<dyn Trait>>) -> bool {\n    \
         ConsentGate::from_ref(cm).may_upload_telemetry()\n\
         }\n",
    )
    .expect("write example.rs");

    let mut violations = Vec::new();
    visit(&src_dir, &mut violations);
    assert!(violations.is_empty());
}

#[test]
fn end_to_end_whole_file_tests_rs_is_exempt() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_dir = tmp
        .path()
        .join("src-tauri")
        .join("src")
        .join("provider_adapters");
    std::fs::create_dir_all(&src_dir).expect("mkdir");
    std::fs::write(
        src_dir.join("tests.rs"),
        "fn effective_permissions(&self) -> ConsentPermissions { self.perms.clone() }\n",
    )
    .expect("write tests.rs");

    let mut violations = Vec::new();
    visit(&src_dir, &mut violations);
    assert!(
        violations.is_empty(),
        "a whole-file `tests.rs` module (ADR-003) must be exempt"
    );
}
