//! Workspace gate: every script a workflow runs must exist AND be exported.
//!
//! The public repository is an allowlist export (`scripts/public-repo-include.txt`)
//! of this tree, and the workflows under `.github/workflows/` execute ONLY there
//! (the parent monorepo never runs them). A workflow step that references a
//! script missing from the allowlist passes every local check and then dies
//! with exit 127 on the public runner — public PR #83 hit this twice in one
//! run (`check-tauri-csp-sync.sh`, `check-consent-erasure-barrier.sh`; the
//! same forgotten-registration class also hid `pnpm-workspace.yaml`). This
//! gate runs via `cargo test --workspace` (ADR-075 P-4: no dead gates).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

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

    let workflows = root.join(".github").join("workflows");
    let mut missing_files = Vec::new();
    let mut unexported = Vec::new();
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

/// Repository-relative `scripts/...` paths (with a file extension) referenced
/// anywhere in the workflow text. `./scripts/foo.sh` and `scripts/foo.sh`
/// forms both count; extensionless tokens (directories) are ignored.
fn find_script_references(source: &str) -> BTreeSet<String> {
    let mut refs = BTreeSet::new();
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
    refs
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
fn exported_accepts_direct_and_directory_coverage() {
    let entries = vec!["scripts/ci".to_string(), "scripts/check-a.sh".to_string()];
    assert!(is_exported("scripts/check-a.sh", &entries));
    assert!(is_exported("scripts/ci/nested.sh", &entries));
    assert!(!is_exported("scripts/check-b.sh", &entries));
}
