//! Shared helpers for the `maekon-lint` workspace supply-chain gates.
//!
//! Both the toolchain-pin gate and the script/local-action export gate must
//! inspect the SAME set of CI YAML files: the flat top-level workflows AND the
//! composite actions under `.github/actions/`. Centralising the file-collection
//! logic keeps the two gates from drifting apart (#7081 MLINT-1).

// Not every gate uses every helper; the unused ones in a given test binary are
// expected.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Absolute path to the maekon-client workspace root (two levels above the
/// `maekon-lint` crate manifest).
pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

/// `true` for `*.yml` / `*.yaml` paths.
pub fn is_yaml(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "yml" || ext == "yaml")
}

/// Every CI YAML file that runs on the public runner:
///
/// 1. the flat `.github/workflows/*.yml|*.yaml` set (GitHub only reads workflow
///    definitions from this single directory), PLUS
/// 2. every `*.yml|*.yaml` nested at any depth under `.github/actions/`.
///
/// Composite actions execute on the public runner exactly like workflows, so
/// the same toolchain-pin and forgotten-export invariants must hold for them.
/// The pre-#7081 gates did a flat `read_dir(.github/workflows)` and never
/// descended into `.github/actions/**`, leaving a latent false-negative window
/// (#7081 MLINT-1). Returns a sorted list for deterministic violation output.
pub fn collect_ci_yaml_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let workflows = root.join(".github").join("workflows");
    if let Ok(entries) = std::fs::read_dir(&workflows) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_yaml(&path) {
                files.push(path);
            }
        }
    }
    collect_yaml_recursive(&root.join(".github").join("actions"), &mut files);
    files.sort();
    files
}

fn collect_yaml_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // `.github/actions` is optional (absent on sparse checkouts)
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_yaml_recursive(&path, out);
        } else if is_yaml(&path) {
            out.push(path);
        }
    }
}
