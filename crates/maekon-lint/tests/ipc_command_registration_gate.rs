//! Workspace gate: every `#[tauri::command]` / `#[command]` fn defined under
//! `src-tauri/src/commands/` must be registered in the
//! `.invoke_handler(tauri::generate_handler![...])` block in
//! `src-tauri/src/lib.rs` (#7718 H5). The invoke_handler + command surface
//! live in the `[lib]` target (#7734); `src-tauri/src/main.rs` is now a thin
//! binary shim that just calls `maekon_app::run()`.
//!
//! A defined-but-unregistered command function is either genuinely dead (the
//! Tauri IPC bridge only dispatches names present in that list, so the
//! webview can never reach it) or — the sharper case found by #7718 — an
//! orphaned source file that was never wired into the module tree at all via
//! a `mod` declaration (`src-tauri/src/commands/suggestions/stats.rs`: 4
//! `#[command]` fns on disk, 0 compiled, because no `mod stats;` line ever
//! existed). `cargo build`/`cargo clippy` are silent about both cases — an
//! orphaned file isn't part of the crate graph, so there's nothing to warn
//! about, and a compiled-but-unregistered fn only trips `dead_code` if
//! nothing else in the crate calls it (which is exactly the case here, since
//! Tauri commands are only ever called from the JS bridge). This gate closes
//! that blind spot with a direct source-level diff.
//!
//! It runs as a normal test of the `maekon-lint` package, so CI's
//! `cargo test --workspace` fires it without any extra pipeline wiring
//! (ADR-075 P-4: no dead gates).
//!
//! Escape hatch: a fn that is intentionally a Tauri command awaiting wiring
//! (e.g. mid-refactor) may carry a `lint:allow-unregistered-command` marker
//! comment on the `#[tauri::command]`/`#[command]` attribute line or the line
//! directly above it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const MARKER: &str = "lint:allow-unregistered-command";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

#[test]
fn every_defined_command_is_registered_in_invoke_handler() {
    let root = workspace_root();
    let commands_dir = root.join("src-tauri/src/commands");
    // #7734: invoke_handler moved from main.rs into the `[lib]` target.
    let lib_rs_path = root.join("src-tauri/src/lib.rs");

    let defined = collect_defined_commands(&commands_dir);
    let main_src = std::fs::read_to_string(&lib_rs_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", lib_rs_path.display(), e));
    let registered = extract_registered_commands(&main_src);

    let mut unregistered: Vec<String> = defined
        .iter()
        .filter(|(name, _)| !registered.contains(name.as_str()))
        .map(|(name, path)| format!("{name} ({})", path.display()))
        .collect();
    unregistered.sort();

    assert!(
        unregistered.is_empty(),
        "#[command] fn(s) defined under src-tauri/src/commands/ but never registered in \
         main.rs's tauri::generate_handler![...] block — dead or orphaned IPC handler. Remove \
         the handler (and, if the whole file is unreachable, the file itself) or register it, \
         or mark a justified in-progress site with `{MARKER}`: {unregistered:#?}"
    );
}

/// Recursively collects `(fn_name, file_path)` for every non-exempt
/// `#[tauri::command]` / `#[command]` fn under `dir`.
fn collect_defined_commands(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    visit_commands_dir(dir, &mut out);
    out
}

fn visit_commands_dir(dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // tolerated: caller passes a fixed known-good path in the real gate
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            visit_commands_dir(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let source = std::fs::read_to_string(&path).expect("read source file");
            for name in find_command_fn_names(&source) {
                out.push((name, path.clone()));
            }
        }
    }
}

/// Pure scanner: names of `pub [async] fn NAME` declarations immediately
/// preceded by a bare `#[tauri::command]` or `#[command]` attribute line
/// (skipping intervening attribute lines like `#[allow(...)]`, and blank
/// lines). Deliberately an EXACT line match on the bare attribute so this
/// never confuses a `clap`-derive `#[command(name = "...")]` (used by the
/// `generate_external_cert` CLI helper module) or a `` `#[command]` ``
/// mention inside a doc comment with a real Tauri command attribute.
fn find_command_fn_names(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut names = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed != "#[tauri::command]" && trimmed != "#[command]" {
            continue;
        }
        let exempt = trimmed.contains(MARKER) || (idx > 0 && lines[idx - 1].contains(MARKER));
        if exempt {
            continue;
        }

        let mut j = idx + 1;
        while j < lines.len() {
            let next = lines[j].trim();
            if next.is_empty() || next.starts_with("#[") {
                j += 1;
                continue;
            }
            if let Some(name) = extract_fn_name(next) {
                names.push(name);
            }
            break;
        }
    }
    names
}

/// Extracts `NAME` from a `pub [async ]fn NAME(...` line prefix. Returns
/// `None` for any other line shape (e.g. a doc comment or a non-`pub` fn —
/// Tauri commands must be `pub` to be referenced from `main.rs`).
fn extract_fn_name(line: &str) -> Option<String> {
    let after_pub = line.strip_prefix("pub ")?;
    let after_async = after_pub.strip_prefix("async ").unwrap_or(after_pub);
    let after_fn = after_async.strip_prefix("fn ")?;
    let name: String = after_fn
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Extracts the bare fn-name set from the
/// `.invoke_handler(tauri::generate_handler![...])` block in `lib.rs`.
/// Mirrors `extract_invoke_handler_commands` in
/// `src-tauri/tests/ipc_command_contract.rs` — duplicated rather than shared
/// because `maekon-lint` is a standalone workspace tool with no dependency on
/// `src-tauri` or `maekon-core` (see the crate's CLAUDE.md dependency graph).
fn extract_registered_commands(main_src: &str) -> BTreeSet<String> {
    let block = main_src
        .split(".invoke_handler(tauri::generate_handler![")
        .nth(1)
        .and_then(|tail| tail.split("])").next())
        .expect("main.rs must register commands through tauri::generate_handler![...]");

    block
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next().unwrap_or_default().trim();
            if line.is_empty() {
                return None;
            }
            let command_path = line.trim_end_matches(',');
            command_path.rsplit("::").next().map(str::to_owned)
        })
        .collect()
}

// ── Scanner self-tests (the guard's guard) ─────────────────────────────────

#[test]
fn scanner_finds_bare_tauri_command_attribute() {
    let src = "#[tauri::command]\npub async fn do_thing(x: u32) -> Result<(), IpcError> {\n}\n";
    assert_eq!(find_command_fn_names(src), vec!["do_thing".to_owned()]);
}

#[test]
fn scanner_finds_bare_command_attribute() {
    let src = "#[command]\npub fn do_thing() {}\n";
    assert_eq!(find_command_fn_names(src), vec!["do_thing".to_owned()]);
}

#[test]
fn scanner_ignores_clap_derive_command_with_args() {
    // The `generate_external_cert` CLI helper module uses clap's derive
    // `#[command(name = "...")]` — same token, unrelated macro. Must not be
    // mistaken for a Tauri IPC command.
    let src = "#[command(name = \"generate-external-cert\")]\npub fn run() {}\n";
    assert!(find_command_fn_names(src).is_empty());
}

#[test]
fn scanner_ignores_doc_comment_mention() {
    let src =
        "/// Extracted from the `#[command]` so the caller can reuse it.\npub fn helper() {}\n";
    assert!(find_command_fn_names(src).is_empty());
}

#[test]
fn scanner_skips_intervening_attributes_and_blank_lines() {
    let src =
        "#[tauri::command]\n\n#[allow(clippy::too_many_arguments)]\npub async fn do_thing() {}\n";
    assert_eq!(find_command_fn_names(src), vec!["do_thing".to_owned()]);
}

#[test]
fn scanner_strips_generic_bounds_from_fn_name() {
    let src = "#[command]\npub async fn get_status<R: Runtime>(app: AppHandle<R>) {}\n";
    assert_eq!(find_command_fn_names(src), vec!["get_status".to_owned()]);
}

#[test]
fn scanner_respects_allow_marker_on_attribute_line() {
    let src = "#[tauri::command] // lint:allow-unregistered-command — awaiting wiring\npub async fn future_command() {}\n";
    assert!(find_command_fn_names(src).is_empty());
}

#[test]
fn scanner_respects_allow_marker_on_preceding_line() {
    let src = "// lint:allow-unregistered-command — awaiting wiring\n#[tauri::command]\npub async fn future_command() {}\n";
    assert!(find_command_fn_names(src).is_empty());
}

#[test]
fn scanner_finds_multiple_commands_in_one_file() {
    let src = "#[command]\npub async fn one() {}\n\n#[command]\npub async fn two() {}\n";
    assert_eq!(
        find_command_fn_names(src),
        vec!["one".to_owned(), "two".to_owned()]
    );
}

#[test]
fn extract_registered_commands_reads_module_qualified_paths() {
    let src = "        .invoke_handler(tauri::generate_handler![\n            \
                commands::audit::verify_audit_log,\n            \
                commands::consent::get_consent,\n        \
                ])\n        .build(tauri::generate_context!())\n";
    let registered = extract_registered_commands(src);
    assert_eq!(
        registered,
        BTreeSet::from(["verify_audit_log".to_owned(), "get_consent".to_owned()])
    );
}

#[test]
#[should_panic(expected = "generate_handler")]
fn extract_registered_commands_panics_without_invoke_handler_block() {
    let _ = extract_registered_commands("fn main() {}\n");
}

// ── End-to-end fixture: a defined-but-unregistered command is caught ───────

#[test]
fn end_to_end_flags_unregistered_command_via_tempdir_fixture() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let commands_dir = tmp.path().join("commands");
    std::fs::create_dir_all(&commands_dir).expect("mkdir commands");
    std::fs::write(
        commands_dir.join("widget.rs"),
        "#[tauri::command]\npub async fn get_widget() {}\n\n\
         #[tauri::command]\npub async fn delete_widget() {}\n",
    )
    .expect("write widget.rs");

    let main_src = "        .invoke_handler(tauri::generate_handler![\n            \
                     commands::widget::get_widget,\n        \
                     ])\n";

    let defined = collect_defined_commands(&commands_dir);
    let registered = extract_registered_commands(main_src);
    let unregistered: Vec<&String> = defined
        .iter()
        .filter(|(name, _)| !registered.contains(name.as_str()))
        .map(|(name, _)| name)
        .collect();

    assert_eq!(
        unregistered,
        vec![&"delete_widget".to_owned()],
        "delete_widget is defined but not in the invoke_handler list; get_widget is registered \
         and must not be flagged"
    );
}

#[test]
fn end_to_end_clean_fixture_reports_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let commands_dir = tmp.path().join("commands");
    std::fs::create_dir_all(&commands_dir).expect("mkdir commands");
    std::fs::write(
        commands_dir.join("widget.rs"),
        "#[tauri::command]\npub async fn get_widget() {}\n",
    )
    .expect("write widget.rs");

    let main_src = "        .invoke_handler(tauri::generate_handler![\n            \
                     commands::widget::get_widget,\n        \
                     ])\n";

    let defined = collect_defined_commands(&commands_dir);
    let registered = extract_registered_commands(main_src);
    let unregistered: Vec<&String> = defined
        .iter()
        .filter(|(name, _)| !registered.contains(name.as_str()))
        .map(|(name, _)| name)
        .collect();

    assert!(unregistered.is_empty());
}
