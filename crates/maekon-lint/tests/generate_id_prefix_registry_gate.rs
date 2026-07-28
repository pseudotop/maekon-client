//! Workspace gate: ADR-022 id-prefix registry completeness (#8047 E3).
//!
//! ## Background
//!
//! `maekon_core::id_generation::generate_id(prefix)` **panics** at runtime when
//! `prefix` is not one the ADR-022 rules accept, and the accepted set is tracked
//! by hand in the `USED_PREFIXES` allowlist that lives inside the
//! `#[cfg(test)] mod tests` of `crates/maekon-core/src/id_generation.rs` (#4344).
//! That allowlist only proves the *listed* prefixes do not panic — it does NOT
//! prove that every *call site* passes a listed prefix. So a typo'd literal at a
//! brand-new call site (`generate_id("sgu")`) compiles fine and only blows up the
//! first time that code path runs at runtime.
//!
//! ## Mechanism
//!
//! This gate closes that gap statically. It:
//!
//! 1. Reads the single source of truth — the `USED_PREFIXES` array in
//!    `crates/maekon-core/src/id_generation.rs` — directly from source (maekon-lint
//!    has no code dependency on maekon-core, so it is read as text). The registry is
//!    not duplicated here.
//! 2. Scans every `.rs` file under `crates/*/src/**` and `src-tauri/src/**` for
//!    `generate_id("<literal>")` and `generate_id_checked("<literal>")` call sites.
//! 3. Asserts each literal prefix is present in `USED_PREFIXES`.
//!
//! A call site that passes a **non-literal** prefix (a variable / expression decided
//! at runtime) cannot be checked statically; those must be listed in
//! `NON_LITERAL_CALLSITE_EXCEPTIONS` with a rationale.
//!
//! ## Scan scope
//!
//! Only the `src/**` trees are scanned (matching the ADR-013 LOC gate's scope), which
//! excludes `tests/` integration trees. The definition file
//! `crates/maekon-core/src/id_generation.rs` is additionally excluded: it holds the
//! function definitions plus unit tests that deliberately pass invalid prefixes
//! (`generate_id("Sug")`) to exercise the panic/`Err` paths, which would otherwise be
//! false positives. Every real prefix used there (`sug`, `req`) is exercised by other
//! in-scope call sites anyway.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Non-literal `generate_id`/`generate_id_checked` call sites that are allowed to bypass
/// the literal-prefix check, keyed by workspace-relative file path, each with a rationale.
///
/// Empty today: every in-scope call site passes a static string literal. When a legitimate
/// dynamic-prefix call site is introduced, add its `src/**` path here with a one-line reason.
const NON_LITERAL_CALLSITE_EXCEPTIONS: &[(&str, &str)] = &[];

/// Absolute path to the maekon-client workspace root (two levels above this crate manifest).
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

/// The registry file that owns `USED_PREFIXES` — also excluded from the call-site scan.
fn registry_definition_file(root: &Path) -> PathBuf {
    root.join("crates/maekon-core/src/id_generation.rs")
}

/// Extract the `USED_PREFIXES` allowlist from the maekon-core registry source.
fn read_used_prefixes(root: &Path) -> BTreeSet<String> {
    let source = std::fs::read_to_string(registry_definition_file(root))
        .expect("id_generation.rs must be readable");
    let anchor = "USED_PREFIXES: &[&str] = &[";
    let start = source
        .find(anchor)
        .expect("USED_PREFIXES declaration must exist in id_generation.rs")
        + anchor.len();
    let rest = &source[start..];
    let end = rest
        .find("];")
        .expect("USED_PREFIXES declaration must be closed with `];`");
    let body = &rest[..end];

    let prefixes = extract_string_literals(body);
    assert!(
        !prefixes.is_empty(),
        "USED_PREFIXES was parsed as empty — the registry format changed; update this gate"
    );
    prefixes.into_iter().collect()
}

/// Collect every `"..."` string literal in `text` (single-line array bodies only, which is
/// all `USED_PREFIXES` and any call-site argument use).
fn extract_string_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            if j < bytes.len() {
                out.push(text[i + 1..j].to_string());
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// A discovered `generate_id`/`generate_id_checked` call site.
struct CallSite {
    file: String,
    line: usize,
    /// `Some(prefix)` for a string-literal argument, `None` for a non-literal one.
    prefix: Option<String>,
}

/// Recursively collect `.rs` files under `dir`, skipping the excluded registry file.
fn collect_rs_files(dir: &Path, exclude: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // Optional/sparse directories are tolerated.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, exclude, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") && path != exclude {
            out.push(path);
        }
    }
}

/// All in-scope `src/**` roots: `crates/*/src` plus `src-tauri/src`.
fn scan_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
        for entry in entries.flatten() {
            let src = entry.path().join("src");
            if src.is_dir() {
                roots.push(src);
            }
        }
    }
    let tauri_src = root.join("src-tauri/src");
    if tauri_src.is_dir() {
        roots.push(tauri_src);
    }
    roots
}

/// `true` when the char immediately before a `generate_id` match is a valid left boundary —
/// i.e. NOT part of a longer identifier and NOT a method call (`foo.generate_id(...)`).
fn has_call_left_boundary(source: &str, match_start: usize) -> bool {
    match source[..match_start].chars().next_back() {
        None => true,
        Some(ch) => !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '.'),
    }
}

/// Extract the call site's argument classification from the text right after the `(`.
/// Returns `Some(literal)` for a `"..."` argument, `None` for a non-literal argument.
fn classify_argument(after_paren: &str) -> Option<String> {
    let trimmed = after_paren.trim_start();
    let bytes = trimmed.as_bytes();
    if bytes.first() == Some(&b'"') {
        let end = trimmed[1..].find('"')? + 1;
        return Some(trimmed[1..end].to_string());
    }
    None
}

/// Scan one file's source for `generate_id(` and `generate_id_checked(` call sites.
fn scan_file(rel_path: &str, source: &str, sites: &mut Vec<CallSite>) {
    // `generate_id_checked(` and `generate_id(` are disjoint substrings: the char after
    // `generate_id` in a checked call is `_`, never `(`, so a checked call is never matched
    // by the base pattern.
    for pattern in ["generate_id_checked(", "generate_id("] {
        for (idx, _) in source.match_indices(pattern) {
            if !has_call_left_boundary(source, idx) {
                continue;
            }
            let line = source[..idx].bytes().filter(|&b| b == b'\n').count() + 1;
            // Skip pure comment lines so doc/comment examples never trip the gate.
            let line_text = source.lines().nth(line - 1).unwrap_or("").trim_start();
            if line_text.starts_with("//") {
                continue;
            }
            let after_paren = &source[idx + pattern.len()..];
            sites.push(CallSite {
                file: rel_path.to_string(),
                line,
                prefix: classify_argument(after_paren),
            });
        }
    }
}

#[test]
fn every_generate_id_prefix_is_registered() {
    let root = workspace_root();
    let used_prefixes = read_used_prefixes(&root);
    let exclude = registry_definition_file(&root);

    let mut files = Vec::new();
    for src_root in scan_roots(&root) {
        collect_rs_files(&src_root, &exclude, &mut files);
    }
    files.sort();
    assert!(
        !files.is_empty(),
        "no source files were scanned — scan roots are wrong"
    );

    let mut sites = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
        if !source.contains("generate_id") {
            continue;
        }
        scan_file(&rel, &source, &mut sites);
    }

    assert!(
        !sites.is_empty(),
        "scan found zero generate_id call sites — the scanner regressed"
    );

    let exceptions: BTreeSet<&str> = NON_LITERAL_CALLSITE_EXCEPTIONS
        .iter()
        .map(|(path, _)| *path)
        .collect();

    let mut unregistered_literals = Vec::new();
    let mut unlisted_non_literals = Vec::new();
    for site in &sites {
        match &site.prefix {
            Some(prefix) => {
                if !used_prefixes.contains(prefix) {
                    unregistered_literals.push(format!(
                        "{}:{} generate_id(\"{}\") — prefix not in USED_PREFIXES",
                        site.file, site.line, prefix
                    ));
                }
            }
            None => {
                if !exceptions.contains(site.file.as_str()) {
                    unlisted_non_literals.push(format!(
                        "{}:{} generate_id(<non-literal>) — add to NON_LITERAL_CALLSITE_EXCEPTIONS with a rationale",
                        site.file, site.line
                    ));
                }
            }
        }
    }

    assert!(
        unregistered_literals.is_empty(),
        "generate_id call sites use prefixes missing from the USED_PREFIXES registry in \
         crates/maekon-core/src/id_generation.rs (add the prefix there to avoid a runtime panic): {unregistered_literals:#?}"
    );
    assert!(
        unlisted_non_literals.is_empty(),
        "non-literal generate_id call sites cannot be statically verified — list them in \
         NON_LITERAL_CALLSITE_EXCEPTIONS: {unlisted_non_literals:#?}"
    );
}

#[test]
fn scanner_extracts_literal_and_non_literal_arguments() {
    // Pins the scanner's own behavior (fixtures, not real source).
    let mut sites = Vec::new();
    let fixture = "let a = generate_id(\"sug\");\n\
                   let b = generate_id_checked(\"req\");\n\
                   let c = obj.generate_id(\"ignored\");\n\
                   // let d = generate_id(\"comment\");\n\
                   let e = generate_id(dynamic_prefix);\n";
    scan_file("fixture.rs", fixture, &mut sites);

    // Method call (`obj.generate_id`) and the commented line are excluded.
    assert_eq!(sites.len(), 3, "unexpected call sites: {}", sites.len());

    // Order-independent: the two patterns are scanned in separate passes, so compare as a set.
    let literals: BTreeSet<&str> = sites.iter().filter_map(|s| s.prefix.as_deref()).collect();
    assert_eq!(
        literals,
        BTreeSet::from(["sug", "req"]),
        "both the base and `_checked` literal calls must be captured"
    );

    let non_literal = sites.iter().filter(|s| s.prefix.is_none()).count();
    assert_eq!(
        non_literal, 1,
        "the dynamic-prefix call must be non-literal"
    );
}

#[test]
fn used_prefixes_registry_parses() {
    let root = workspace_root();
    let prefixes = read_used_prefixes(&root);
    // Spot-check a couple of well-known prefixes so a parse regression is obvious.
    assert!(prefixes.contains("sug"), "registry must contain `sug`");
    assert!(prefixes.contains("req"), "registry must contain `req`");
}
