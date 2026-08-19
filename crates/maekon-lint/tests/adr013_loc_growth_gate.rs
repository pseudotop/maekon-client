//! Workspace gate: ADR-013 file-split marker discipline — LOC growth cap +
//! new-giant detection (#7732 D2+B5).
//!
//! ## Background
//!
//! Several source files across the workspace carry an in-source
//! `// OOS-TBD: ADR-013 file split (cycle N+) — LOC: <n>` comment recording
//! the line count at the moment a maintainer flagged the file as a
//! directory-module-split candidate (`rg 'ADR-013 file split'
//! clients/maekon-client` finds them). The client's own local ADR for this
//! pattern is ADR-003 ("Directory Module Pattern"), not ADR-013 — the
//! `ADR-013` shorthand in these marker comments echoes the analogous
//! server-side ADR-013 ("Domain Service Folder Pattern") that inspired
//! ADR-003, per `docs/architecture/ADR-003-directory-module-pattern.md`.
//!
//! Those markers were purely documentary before this gate: nothing enforced
//! the recorded LOC as a cap, so marked files kept growing well past their
//! at-marking size (`feature_capabilities.rs` 869 -> 1,827 as of this gate's
//! introduction; `subprocess_session.rs` 847 -> 1,675), and several
//! *unmarked* files grew even larger without ever being flagged at all
//! (`scheduler/loops/system.rs` 2,305; `sync_engine.rs` 2,139;
//! `commands/audio.rs` 1,740).
//!
//! ## Mechanism
//!
//! This gate does NOT parse the in-source marker comments at test time —
//! they remain human documentation, and a file does not need one to be
//! covered here. The single source of truth is the checked-in baseline
//! registry at `crates/maekon-lint/adr013_loc_baseline.json`: a flat
//! `{ "workspace/relative/path.rs": <max_loc> }` map. Two rules, both driven
//! by that one registry:
//!
//! 1. **Growth cap** — a file listed in the baseline may not exceed its
//!    recorded cap. Shrinking a file below its cap fails nothing; trim
//!    (lower or remove) the entry by hand as part of a real split PR
//!    (list-shrinks-only discipline, matching the other baseline-driven
//!    gates in this repo, e.g. the ADR-065 lint baseline).
//! 2. **New-giant guard** — any `.rs` file under `src-tauri/src/**` or
//!    `crates/*/src/**` that exceeds `GIANT_FILE_THRESHOLD_LOC` (900) and has
//!    NO baseline entry is an error. This is what actually blocks *new* tech
//!    debt: a freshly-written or freshly-grown file that crosses 900 lines
//!    must get a deliberate baseline entry — ideally alongside an
//!    `// OOS-TBD: ADR-013 file split` marker comment, and better yet an
//!    actual directory-module split per ADR-003 — rather than silently
//!    accumulating unnoticed the way the six files named above did.
//!
//! ## Baseline philosophy (why the registry has 74 entries, not 6 or 19)
//!
//! At gate-introduction time (#7732), a full scan of the two scanned trees
//! (excluding `@generated` files — see below) found 74 files already over
//! the 900-line threshold — far more than the handful of "top offenders"
//! the originating tech-debt review named by example. Every one of the 74
//! was baselined at `ceil(current_loc * 1.05)` (5% headroom), not only the
//! handful explicitly named, because a partial baseline would have shipped
//! this gate already red for the ~55 files nobody had asked to fix as part
//! of this issue. The 5% headroom absorbs incidental formatting/comment
//! churn without licensing a second real growth spurt. This stops the
//! existing debt from getting *worse* without demanding an immediate split
//! of any of the 74 files — splits are tracked and prioritized separately
//! (see the epic referenced in the module path below).
//!
//! `@generated` files (prost-build output under `**/proto/generated/`, e.g.
//! `crates/maekon-network/src/proto/generated/oneshim.client.v1.rs`) are
//! excluded from both the scan and the baseline: their size tracks the proto
//! RPC surface, not hand-maintained tech debt, and splitting generated code
//! is not a meaningful action. The exclusion is header-marker-based (any of
//! the first 5 lines containing `@generated`), not path-based, so it holds
//! for any future generated-code location using the same convention.
//!
//! ## Bump protocol (#9636)
//!
//! NEVER hand-edit a cap to the file's exact current LOC. That erases the
//! designed 5% headroom; by 2026-07 the practice had left 28/92 entries at
//! zero margin, where any unrelated one-line edit broke CI (#9539 was the
//! first firing). When this gate fails on legitimate growth, run
//! `python3 scripts/regen-adr013-loc-baseline.py` — it recomputes EVERY
//! entry at `ceil(loc * 1.05)`, restoring headroom for the grown file and
//! ratcheting shrunken files down in the same pass. A cap raise in the diff
//! is then a deliberate, reviewable event — and the right moment to ask
//! whether the file wants an actual ADR-003 split instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Files at or under this LOC are not tracked at all — grow past it and the
/// file needs a baseline entry (rule 2 in the module doc above).
const GIANT_FILE_THRESHOLD_LOC: usize = 900;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

fn baseline_path(root: &Path) -> PathBuf {
    root.join("crates")
        .join("maekon-lint")
        .join("adr013_loc_baseline.json")
}

fn load_baseline(root: &Path) -> BTreeMap<String, u64> {
    let path = baseline_path(root);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read ADR-013 LOC baseline {}: {e}", path.display()));
    serde_json::from_str::<BTreeMap<String, u64>>(&raw)
        .unwrap_or_else(|e| panic!("parse ADR-013 LOC baseline {}: {e}", path.display()))
}

/// Recursively collects every `.rs` file's line count under `dir`, skipping
/// `@generated` files (checks the first 5 lines — matches prost-build's
/// `// This file is @generated by prost-build.` header convention). Returns
/// `(workspace-relative forward-slash path, loc)` pairs.
fn collect_rust_files(root: &Path, dir: &Path, out: &mut Vec<(String, usize)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // tolerated: a crate without a `src/` dir, sparse checkout, etc.
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(root, &path, out);
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue; // non-UTF8 or unreadable — not a source file we can measure
        };
        let lines: Vec<&str> = text.lines().collect();
        let head_is_generated = lines.iter().take(5).any(|l| l.contains("@generated"));
        if head_is_generated {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((rel, lines.len()));
    }
}

/// The two scan roots this gate covers: `src-tauri/src` and every
/// `crates/<name>/src`. Deliberately does NOT include `tests/`, `frontend/`,
/// `examples/`, or `benches/` directories — this gate tracks hand-maintained
/// production/library source, matching the directory scope named in #7732.
fn collect_scanned_files(root: &Path) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    collect_rust_files(root, &root.join("src-tauri").join("src"), &mut out);
    let crates_dir = root.join("crates");
    if let Ok(entries) = std::fs::read_dir(&crates_dir) {
        for entry in entries.flatten() {
            let crate_src = entry.path().join("src");
            if crate_src.is_dir() {
                collect_rust_files(root, &crate_src, &mut out);
            }
        }
    }
    out.sort();
    out
}

/// Pure scanner: given already-collected `(path, loc)` pairs and the
/// baseline map, returns violation messages. A file above the threshold that
/// is absent from the baseline is a "new giant" violation; a baselined file
/// whose current LOC exceeds its recorded cap is a "grown past cap"
/// violation.
fn find_violations(
    scanned: &[(String, usize)],
    baseline: &BTreeMap<String, u64>,
    threshold: usize,
) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, loc) in scanned {
        match baseline.get(path) {
            Some(&cap) => {
                if *loc as u64 > cap {
                    violations.push(format!(
                        "GROWTH past baselined cap: {path} is {loc} lines, baseline cap is \
                         {cap} — either this growth is a real split-worthy regression (split \
                         the file per ADR-003) or the growth is legitimate and the cap in \
                         crates/maekon-lint/adr013_loc_baseline.json must be deliberately \
                         raised in this PR's diff"
                    ));
                }
            }
            None => {
                if *loc > threshold {
                    violations.push(format!(
                        "NEW unbaselined giant: {path} is {loc} lines (> {threshold}) and has \
                         no entry in crates/maekon-lint/adr013_loc_baseline.json — add an entry \
                         (and ideally an `// OOS-TBD: ADR-013 file split` marker comment), or \
                         split the file per ADR-003 before it grows further"
                    ));
                }
            }
        }
    }
    violations
}

#[test]
fn adr013_loc_baseline_gate() {
    let root = workspace_root();
    let baseline = load_baseline(&root);
    let scanned = collect_scanned_files(&root);
    let violations = find_violations(&scanned, &baseline, GIANT_FILE_THRESHOLD_LOC);
    assert!(
        violations.is_empty(),
        "ADR-013 LOC gate violation(s) (#7732 B5):\n{}",
        violations.join("\n")
    );
}

// ── Scanner self-tests (the guard's guard) ─────────────────────────────────

#[test]
fn scanner_flags_growth_past_baselined_cap() {
    let mut baseline = BTreeMap::new();
    baseline.insert("crates/foo/src/big.rs".to_string(), 1000u64);
    let scanned = vec![("crates/foo/src/big.rs".to_string(), 1001usize)];
    let violations = find_violations(&scanned, &baseline, 900);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("GROWTH past baselined cap"));
    assert!(violations[0].contains("crates/foo/src/big.rs"));
}

#[test]
fn scanner_allows_baselined_file_at_or_under_cap() {
    let mut baseline = BTreeMap::new();
    baseline.insert("crates/foo/src/big.rs".to_string(), 1000u64);
    let scanned = vec![("crates/foo/src/big.rs".to_string(), 1000usize)];
    assert!(find_violations(&scanned, &baseline, 900).is_empty());
}

#[test]
fn scanner_flags_new_unbaselined_giant() {
    let baseline = BTreeMap::new();
    let scanned = vec![("crates/foo/src/new_giant.rs".to_string(), 901usize)];
    let violations = find_violations(&scanned, &baseline, 900);
    assert_eq!(violations.len(), 1);
    assert!(violations[0].contains("NEW unbaselined giant"));
    assert!(violations[0].contains("crates/foo/src/new_giant.rs"));
}

#[test]
fn scanner_allows_unbaselined_file_at_or_under_threshold() {
    let baseline = BTreeMap::new();
    let scanned = vec![("crates/foo/src/small.rs".to_string(), 900usize)];
    assert!(find_violations(&scanned, &baseline, 900).is_empty());
}

#[test]
fn scanner_ignores_generated_marker_files_in_real_collection() {
    // Real end-to-end proof (not just the pure `find_violations` function):
    // a file whose head carries the prost-build `@generated` marker is
    // excluded from `collect_rust_files`'s output entirely, even though its
    // LOC would otherwise trip the new-giant rule.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let src = root.join("crates").join("fakecrate").join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    let body = format!(
        "// This file is @generated by prost-build.\n{}",
        "pub struct X;\n".repeat(950)
    );
    std::fs::write(src.join("generated.rs"), body).expect("write generated fixture");

    let mut out = Vec::new();
    collect_rust_files(root, &src, &mut out);
    assert!(
        out.is_empty(),
        "a file with an @generated header must be excluded from the scan: {out:?}"
    );
}

#[test]
fn scanner_collects_a_real_oversized_non_generated_file() {
    // Companion to the previous test: a same-size file WITHOUT the
    // `@generated` header is collected (proves the exclusion is
    // marker-driven, not merely "everything under a fake crate is skipped").
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let src = root.join("crates").join("fakecrate").join("src");
    std::fs::create_dir_all(&src).expect("mkdir src");
    let body = "pub struct X;\n".repeat(950);
    std::fs::write(src.join("handwritten.rs"), body).expect("write handwritten fixture");

    let mut out = Vec::new();
    collect_rust_files(root, &src, &mut out);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "crates/fakecrate/src/handwritten.rs");
    assert_eq!(out[0].1, 950);
}
