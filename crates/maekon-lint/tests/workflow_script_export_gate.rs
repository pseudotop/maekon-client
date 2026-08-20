//! Workspace gate: every script AND local action a workflow runs must exist
//! AND be exported.
//!
//! The public repository is an allowlist export (`scripts/public-repo-include.txt`)
//! of this tree, and the workflows under `.github/workflows/` execute ONLY there
//! (the parent monorepo never runs them). A workflow step that references a
//! script missing from the allowlist passes every local check and then dies
//! with exit 127 on the public runner — public PR #83 hit this twice in one
//! run (`check-tauri-csp-sync.sh`, `check-consent-erasure-barrier.sh`; the
//! same forgotten-registration class also hid `pnpm-workspace.yaml`).
//!
//! ADR-075 P-4 (no dead gates): this gate is named explicitly in the parent's
//! `Run hedge + LOC + bundle capability + orphan IPC gates` step. It has to be
//! — that step passes `--test` filters, so a gate not on that list runs only in
//! the public export's `continue-on-error` sweep and can sit red for months
//! with nobody looking. #11234 is that story: this file was orphaned, and the
//! red it was holding turned out to be its own false positive.
//!
//! #7081 covers two scope holes in this gate:
//!  - MLINT-1: both checks now also recurse `.github/actions/**` composite
//!    actions (they run on the public runner too).
//!  - MLINT-2: the forgotten-registration class is NOT script-only — it also
//!    hid a non-script file (`pnpm-workspace.yaml`). A sibling check
//!    (`workflow_referenced_local_actions_exist_and_are_exported`) now validates
//!    `uses: ./<path>` LOCAL-ACTION references against the include manifest, so
//!    a forgotten composite-action export is caught here instead of on a public
//!    PR.

use std::collections::BTreeSet;
use std::path::Path;

mod common;

use common::{collect_ci_yaml_files, workspace_root};

#[test]
fn workflow_referenced_scripts_exist_and_are_exported() {
    let root = workspace_root();
    let manifest_path = root.join("scripts/public-repo-include.txt");
    // The manifest exists only in the parent monorepo (it drives the export);
    // on a PUBLIC checkout everything is already exported and there is nothing
    // to validate against — skip instead of failing the public test legs.
    let Ok(include_manifest) = std::fs::read_to_string(&manifest_path) else {
        eprintln!("workflow_script_export_gate: no include manifest (public checkout) — skipping");
        return;
    };
    let include_entries = parse_include_entries(&include_manifest);

    let mut missing_files = Vec::new();
    let mut unexported = Vec::new();
    for path in collect_ci_yaml_files(&root) {
        let source = std::fs::read_to_string(&path).expect("read workflow/action file");
        let workflow = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        for script in find_script_references(&source) {
            if !root.join(&script).exists() {
                missing_files.push(format!("{workflow}: {script}"));
            }
            if !is_exported(&script, &include_entries) {
                unexported.push(format!("{workflow}: {script}"));
            }
        }
    }

    assert!(
        missing_files.is_empty(),
        "workflow(s) reference scripts that do not exist in the repository: {missing_files:#?}"
    );
    assert!(
        unexported.is_empty(),
        "workflow(s) reference scripts missing from scripts/public-repo-include.txt — they will \
         exit 127 on the public runner (the only place these workflows execute): {unexported:#?}"
    );
}

/// #7081 MLINT-2: every `uses: ./<path>` LOCAL-ACTION reference must exist AND
/// be exported. A forgotten composite-action entry resolves fine locally and
/// then fails to resolve on the public runner — the same forgotten-export class
/// the script check covers, but for the non-script file class the gate
/// previously left unguarded.
#[test]
fn workflow_referenced_local_actions_exist_and_are_exported() {
    let root = workspace_root();
    let manifest_path = root.join("scripts/public-repo-include.txt");
    let Ok(include_manifest) = std::fs::read_to_string(&manifest_path) else {
        eprintln!("workflow_script_export_gate: no include manifest (public checkout) — skipping");
        return;
    };
    let include_entries = parse_include_entries(&include_manifest);

    let mut missing = Vec::new();
    let mut unexported = Vec::new();
    for path in collect_ci_yaml_files(&root) {
        let source = std::fs::read_to_string(&path).expect("read workflow/action file");
        let workflow = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        for action in find_local_action_references(&source) {
            match classify_local_action(&root, &action, &include_entries) {
                ActionExport::Skipped | ActionExport::Ok => {}
                ActionExport::Missing => missing.push(format!("{workflow}: ./{action}")),
                ActionExport::Unexported => unexported.push(format!("{workflow}: ./{action}")),
            }
        }
    }

    assert!(
        missing.is_empty(),
        "workflow(s) reference local actions that do not exist in the repository: {missing:#?}"
    );
    assert!(
        unexported.is_empty(),
        "workflow(s) reference local actions missing from scripts/public-repo-include.txt — they \
         will fail to resolve on the public runner (the only place these workflows execute): \
         {unexported:#?}"
    );
}

/// Repository-relative `scripts/...` paths (with a file extension) referenced
/// anywhere in the workflow text. `./scripts/foo.sh` and `scripts/foo.sh`
/// forms both count; extensionless tokens (directories) are ignored.
///
/// Comment-only lines are skipped (#11234). What this gate is for is "the
/// public runner will hit exit 127 on a script we did not export", and a
/// comment never runs. `ci.yml` explains its release-build step by naming the
/// script that pins it (`# … pinned against it by scripts/…-governance.sh`),
/// which is prose about a deliberately parent-only script — flagging it
/// demanded either exporting a private script or rewording documentation,
/// and both are the wrong answer. The sibling `find_local_action_references`
/// already treats `#` as ending a path; only this scanner lacked it.
fn find_script_references(source: &str) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    for line in source
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
    {
        collect_script_references_in_line(line, &mut refs);
    }
    refs
}

/// Scan a single non-comment line. Split out so the comment filter above reads
/// as one decision rather than an index dance inside the scan loop.
fn collect_script_references_in_line(source: &str, refs: &mut BTreeSet<String>) {
    let bytes = source.as_bytes();
    let mut i = 0;
    while let Some(rel) = source[i..].find("scripts/") {
        let start = i + rel;
        // Word boundary: reject e.g. `frontend/scripts/...` (not repo-root) by
        // requiring the preceding char to not be a path/word character other
        // than `./` — accept start-of-line, whitespace, quotes, `(`, and `./`.
        let boundary_ok = {
            if start == 0 {
                true
            } else {
                let prev = bytes[start - 1] as char;
                prev.is_whitespace()
                    || matches!(prev, '"' | '\'' | '(' | '|' | ';' | '=' | ':')
                    || (prev == '/'
                        && start >= 2
                        && bytes[start - 2] as char == '.'
                        && (start == 2 || !(bytes[start - 3] as char).is_ascii_alphanumeric()))
            }
        };
        let mut end = start;
        while end < source.len()
            && source[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
        {
            end += source[end..].chars().next().map_or(0, char::len_utf8);
        }
        let candidate = source[start..end].trim_end_matches('.');
        let has_extension = candidate
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'));
        if boundary_ok && has_extension {
            refs.insert(candidate.to_string());
        }
        i = end.max(start + 1);
    }
}

fn parse_include_entries(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .map(|l| l.trim().trim_end_matches('/'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Covered when listed directly or nested under a listed directory entry.
fn is_exported(script: &str, include_entries: &[String]) -> bool {
    include_entries
        .iter()
        .any(|e| script == e || script.starts_with(&format!("{e}/")))
}

/// Repo-relative LOCAL-ACTION references (`uses: ./<path>`), with the leading
/// `./` stripped. These are GitHub local composite/JS actions that must be
/// EXPORTED so they resolve on the public runner — the non-script half of the
/// forgotten-export class (#7081 MLINT-2). Both `- uses: ./x` and a bare
/// `uses: ./x` form are recognised; an inline `# comment` or quote ends the
/// path. Remote references (`uses: owner/repo@ref`) carry no `./` and are
/// ignored.
fn find_local_action_references(source: &str) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
    for raw in source.lines() {
        let line = raw.trim_start();
        let line = line.strip_prefix("- ").unwrap_or(line);
        let Some(spec) = line.strip_prefix("uses:") else {
            continue;
        };
        let Some(rel) = spec.trim().strip_prefix("./") else {
            continue;
        };
        let path: String = rel
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '#' | '"' | '\''))
            .collect();
        let path = path.trim_end_matches('/');
        if !path.is_empty() {
            refs.insert(path.to_string());
        }
    }
    refs
}

/// Export status of a single `uses: ./<path>` local-action reference.
#[derive(Debug, PartialEq, Eq)]
enum ActionExport {
    /// First path segment is not a repo entry — a runtime-checkout indirection
    /// (e.g. `./.workflow-tools/...`, a second `actions/checkout` target created
    /// at job time), not an exportable repo path. Not our concern.
    Skipped,
    /// The referenced local action does not exist in the repository.
    Missing,
    /// The action exists but is absent from the include manifest.
    Unexported,
    /// Present and exported.
    Ok,
}

/// Classify a `uses: ./<action>` reference against the repo tree + manifest.
///
/// The first-segment repo-existence check is what distinguishes a genuine local
/// action (`.github/...`, which exists) from a runtime-checkout indirection
/// (`.workflow-tools/...`, which is synthesised at job time and so is absent
/// from the parent tree).
fn classify_local_action(root: &Path, action: &str, include_entries: &[String]) -> ActionExport {
    let first_seg = action.split('/').next().unwrap_or("");
    if first_seg.is_empty() || !root.join(first_seg).exists() {
        return ActionExport::Skipped;
    }
    if !root.join(action).exists() {
        return ActionExport::Missing;
    }
    if !is_exported(action, include_entries) {
        return ActionExport::Unexported;
    }
    ActionExport::Ok
}

// ── Scanner self-tests (the guard's guard) ─────────────────────────────────

#[test]
fn scanner_finds_dot_slash_and_bare_forms() {
    let src = "      - run: ./scripts/check-a.sh\n      - run: scripts/check-b.sh --flag\n";
    let refs = find_script_references(src);
    assert!(refs.contains("scripts/check-a.sh"), "{refs:?}");
    assert!(refs.contains("scripts/check-b.sh"), "{refs:?}");
}

#[test]
fn scanner_ignores_directories_and_nested_scripts_dirs() {
    let src = "      - run: ls scripts/ci\n      - run: node frontend/scripts/test.mjs\n";
    let refs = find_script_references(src);
    assert!(refs.is_empty(), "{refs:?}");
}

#[test]
fn scanner_ignores_comment_lines(/* #11234 */) {
    // The real regression: `ci.yml` documents its release-build step by naming
    // the parent-only script that pins it. Prose is not an invocation, so the
    // public runner can never hit exit 127 on it.
    let src = "        # pinned against it by scripts/test-release-workflow-governance.sh.\n";
    assert!(
        find_script_references(src).is_empty(),
        "comment must not count"
    );

    // A shell comment inside a `run:` block is equally inert.
    let src = "        run: |\n          # helper lives at scripts/dev-only.sh\n";
    assert!(
        find_script_references(src).is_empty(),
        "shell comment must not count"
    );
}

#[test]
fn scanner_still_sees_a_real_reference_next_to_commented_ones(/* #11234 */) {
    // The comment filter must not swallow the line that actually runs — that
    // would turn a false positive into a false negative, which is worse: an
    // unexported script would reach the public runner unflagged.
    let src = "      # scripts/decoy.sh is only mentioned\n      - run: ./scripts/real.sh\n";
    let refs = find_script_references(src);
    assert!(refs.contains("scripts/real.sh"), "{refs:?}");
    assert!(!refs.contains("scripts/decoy.sh"), "{refs:?}");
}

#[test]
fn exported_accepts_direct_and_directory_coverage() {
    let entries = vec!["scripts/ci".to_string(), "scripts/check-a.sh".to_string()];
    assert!(is_exported("scripts/check-a.sh", &entries));
    assert!(is_exported("scripts/ci/nested.sh", &entries));
    assert!(!is_exported("scripts/check-b.sh", &entries));
}

// ── #7081 MLINT-2: local-action export-completeness ─────────────────────────

#[test]
fn local_action_scanner_finds_dot_slash_uses_and_ignores_remote() {
    let src =
        "      - uses: ./.github/actions/rust-cache\n        uses: ./.github/actions/setup-pnpm\n\
               \n      - uses: actions/checkout@v4\n      - uses: dtolnay/rust-toolchain@stable\n";
    let refs = find_local_action_references(src);
    assert!(refs.contains(".github/actions/rust-cache"), "{refs:?}");
    assert!(refs.contains(".github/actions/setup-pnpm"), "{refs:?}");
    // Remote `owner/repo@ref` references carry no `./` and must be ignored.
    assert_eq!(refs.len(), 2, "{refs:?}");
}

#[test]
fn local_action_scanner_strips_trailing_comment_and_runtime_checkout_prefix() {
    let src = "      - uses: ./.github/actions/x  # inline comment\n\
               \n      - uses: ./.workflow-tools/.github/actions/download-artifact\n";
    let refs = find_local_action_references(src);
    assert!(refs.contains(".github/actions/x"), "{refs:?}");
    // The runtime-checkout-prefixed form is collected here but classified as
    // `Skipped` later (its first segment does not exist in the repo).
    assert!(
        refs.contains(".workflow-tools/.github/actions/download-artifact"),
        "{refs:?}"
    );
}

#[test]
fn classify_flags_forgotten_export_skips_runtime_checkout_and_detects_missing() {
    // Fixture repo: two composite actions exist, only one is in the manifest.
    use std::fs;
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join(".github/actions/exported")).expect("mkdir exported");
    fs::create_dir_all(root.join(".github/actions/forgotten")).expect("mkdir forgotten");
    fs::write(
        root.join(".github/actions/exported/action.yml"),
        "name: x\n",
    )
    .expect("write");
    fs::write(
        root.join(".github/actions/forgotten/action.yml"),
        "name: y\n",
    )
    .expect("write");

    let entries = vec![".github/actions/exported".to_string()];

    // Exported action → Ok.
    assert_eq!(
        classify_local_action(root, ".github/actions/exported", &entries),
        ActionExport::Ok
    );
    // Exists but not in the manifest → Unexported (the regression #7081 closes).
    assert_eq!(
        classify_local_action(root, ".github/actions/forgotten", &entries),
        ActionExport::Unexported
    );
    // Referenced but absent from the repo → Missing.
    assert_eq!(
        classify_local_action(root, ".github/actions/typo", &entries),
        ActionExport::Missing
    );
    // Runtime-checkout indirection (first segment absent in repo) → Skipped.
    assert_eq!(
        classify_local_action(root, ".workflow-tools/.github/actions/dl", &entries),
        ActionExport::Skipped
    );
}
