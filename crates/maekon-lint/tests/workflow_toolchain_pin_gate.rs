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
//!
//! #7081 MLINT-1: the scan covers BOTH the flat top-level workflows AND the
//! composite actions under `.github/actions/**` (a composite action that adds a
//! SHA-pinned `dtolnay/rust-toolchain` step without a `toolchain:` input would
//! fail on the public runner exactly like a workflow). See
//! `tests/common::collect_ci_yaml_files`.

mod common;

use common::{collect_ci_yaml_files, workspace_root};

#[test]
fn sha_pinned_rust_toolchain_steps_declare_toolchain_input() {
    let mut violations = Vec::new();
    for path in collect_ci_yaml_files(&workspace_root()) {
        let source = std::fs::read_to_string(&path).expect("read workflow/action file");
        for line_no in find_unpinned_toolchain_inputs(&source) {
            violations.push(format!("{}:{line_no}", path.display()));
        }
    }
    assert!(
        violations.is_empty(),
        "SHA-pinned dtolnay/rust-toolchain step(s) without an explicit `toolchain:` input — \
         the action cannot infer the toolchain from a commit SHA and fails at runtime in the \
         public repository (the parent monorepo never executes these workflows / composite \
         actions): {violations:#?}"
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

// ── #7081 MLINT-1: composite-action recursion regression ───────────────────

#[test]
fn collector_recurses_into_composite_actions_and_scanner_flags_them() {
    // A SHA-pinned `dtolnay/rust-toolchain` step inside a NESTED composite
    // action (`.github/actions/<name>/action.yml`) must be both collected and
    // flagged. The pre-#7081 flat `read_dir(.github/workflows)` never saw this
    // path, so the latent break shipped to the public runner undetected.
    use std::fs;
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();

    fs::create_dir_all(root.join(".github/workflows")).expect("mkdir workflows");
    fs::write(root.join(".github/workflows/ci.yml"), "name: ci\n").expect("write workflow");

    fs::create_dir_all(root.join(".github/actions/evil")).expect("mkdir composite action");
    let action = root.join(".github/actions/evil/action.yml");
    fs::write(
        &action,
        format!("runs:\n  using: composite\n  steps:\n    - uses: dtolnay/rust-toolchain@{SHA}\n"),
    )
    .expect("write action");

    let collected = collect_ci_yaml_files(root);
    assert!(
        collected.contains(&action),
        "nested composite action must be collected (it was invisible to the flat read): {collected:#?}"
    );

    // The planted unpinned step (line 4 of the action.yml) must be flagged.
    let src = fs::read_to_string(&action).expect("read planted action");
    assert_eq!(find_unpinned_toolchain_inputs(&src), vec![4]);
}
