//! Workspace gate: SHA-pinned `dtolnay/rust-toolchain` steps must pass an
//! explicit `toolchain:` input.
//!
//! When the action is referenced by a tag (`@stable`), it infers the toolchain
//! from the ref name. The supply-chain SHA-pinning campaign replaced tags with
//! commit SHAs, which removes that inference — the action then hard-fails with
//! "'toolchain' is a required input". Because these workflows never execute in
//! the parent monorepo (only in the public export), the breakage is invisible
//! until a public PR runs them (ADR-075 P-4 CI-gate dead spot; first fired on
//! public PR #83). This gate runs as a normal test of the `maekon-lint`
//! package, so `cargo test --workspace` catches the pattern before export.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

#[test]
fn sha_pinned_rust_toolchain_steps_declare_toolchain_input() {
    let workflows = workspace_root().join(".github").join("workflows");
    let mut violations = Vec::new();
    let entries = std::fs::read_dir(&workflows).expect("read .github/workflows");
    for entry in entries {
        let path = entry.expect("dir entry").path();
        let is_yaml = path
            .extension()
            .is_some_and(|ext| ext == "yml" || ext == "yaml");
        if !is_yaml {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read workflow file");
        for line_no in find_unpinned_toolchain_inputs(&source) {
            violations.push(format!("{}:{line_no}", path.display()));
        }
    }
    assert!(
        violations.is_empty(),
        "SHA-pinned dtolnay/rust-toolchain step(s) without an explicit `toolchain:` input — \
         the action cannot infer the toolchain from a commit SHA and fails at runtime in the \
         public repository (the parent monorepo never executes these workflows): {violations:#?}"
    );
}

/// Pure scanner: 1-based line numbers of SHA-pinned `dtolnay/rust-toolchain`
/// `uses:` lines whose step carries no `toolchain:` key. The step body is the
/// run of following lines that are blank or indented deeper than the `uses`
/// keyword (a sibling key such as `with:` aligns with `uses`, so it is part of
/// the step; the next `- ` list item or dedent ends it).
fn find_unpinned_toolchain_inputs(source: &str) -> Vec<usize> {
    let lines: Vec<&str> = source.lines().collect();
    let mut violations = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(uses_col) = sha_pinned_toolchain_uses_col(line) else {
            continue;
        };
        let mut has_toolchain = false;
        for follower in &lines[i + 1..] {
            let trimmed = follower.trim_start();
            if trimmed.is_empty() {
                continue;
            }
            let col = follower.len() - trimmed.len();
            // The step ends at the next list item or any dedent past `uses`.
            if col < uses_col || trimmed.starts_with("- ") {
                break;
            }
            if trimmed.starts_with("toolchain:") {
                has_toolchain = true;
                break;
            }
        }
        if !has_toolchain {
            violations.push(i + 1);
        }
    }
    violations
}

/// Column of the `uses` keyword when the line is a SHA-pinned
/// `dtolnay/rust-toolchain` reference (tag refs like `@stable` are exempt —
/// they self-infer the toolchain).
fn sha_pinned_toolchain_uses_col(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let mut col = line.len() - trimmed.len();
    let mut rest = trimmed;
    if let Some(stripped) = rest.strip_prefix("- ") {
        col += 2;
        rest = stripped;
    }
    let spec = rest.strip_prefix("uses:")?.trim_start();
    let reference = spec.strip_prefix("dtolnay/rust-toolchain@")?;
    let sha: String = reference
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .collect();
    (sha.len() == 40).then_some(col)
}

// ── Scanner self-tests (the guard's guard) ─────────────────────────────────

const SHA: &str = "3c5f7ea28cd621ae0bf5283f0e981fb97b8a7af9";

#[test]
fn scanner_flags_pinned_step_without_with_block() {
    let src = format!(
        "      - name: Install Rust toolchain\n        uses: dtolnay/rust-toolchain@{SHA}\n\n      - name: Next step\n"
    );
    assert_eq!(find_unpinned_toolchain_inputs(&src), vec![2]);
}

#[test]
fn scanner_flags_with_block_missing_toolchain_key() {
    let src = format!(
        "        uses: dtolnay/rust-toolchain@{SHA}\n        with:\n          targets: x86_64-unknown-linux-gnu\n"
    );
    assert_eq!(find_unpinned_toolchain_inputs(&src), vec![1]);
}

#[test]
fn scanner_accepts_explicit_toolchain_input() {
    let src = format!(
        "        uses: dtolnay/rust-toolchain@{SHA}\n        with:\n          toolchain: stable\n          components: rustfmt\n"
    );
    assert!(find_unpinned_toolchain_inputs(&src).is_empty());
}

#[test]
fn scanner_accepts_inline_list_item_form() {
    let src = format!(
        "      - uses: dtolnay/rust-toolchain@{SHA}\n        with:\n          toolchain: stable\n      - name: Next\n"
    );
    assert!(find_unpinned_toolchain_inputs(&src).is_empty());
}

#[test]
fn scanner_ignores_tag_refs_and_other_actions() {
    let src = "        uses: dtolnay/rust-toolchain@stable\n      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5\n";
    assert!(find_unpinned_toolchain_inputs(src).is_empty());
}

#[test]
fn scanner_does_not_credit_toolchain_from_a_following_step() {
    // The `toolchain:` key must belong to the pinned step itself, not to a
    // later sibling step.
    let src = format!(
        "      - uses: dtolnay/rust-toolchain@{SHA}\n      - uses: ./.github/actions/rust-cache\n        with:\n          toolchain: stable\n"
    );
    assert_eq!(find_unpinned_toolchain_inputs(&src), vec![1]);
}
