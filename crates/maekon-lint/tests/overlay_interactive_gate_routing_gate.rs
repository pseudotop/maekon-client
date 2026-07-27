//! Workspace gate: every INTERACTIVE overlay-open path routes through the ONE
//! native fullscreen-policy gate (#8858), and the suggestions shortcut is
//! native-first (#8847).
//!
//! ## Why a source gate
//!
//! #8858 root cause: `activate_suggestions_shortcut` / `set_panel_mode(true)` /
//! `toggle_panel_mode()` reached `apply_window_layout()` and showed the
//! interactive overlay WITHOUT calling `evaluate_fullscreen_policy()`, so a
//! fullscreen external app (#8849) did not suppress the overlay. The fix funnels
//! every interactive open through `MagicOverlayHandle::gate_interactive_open`,
//! which evaluates the policy BEFORE any cold `ensure_window()`. A future
//! refactor could silently reintroduce a bypass (add a new open path, or make an
//! existing one show the surface before the gate) — `cargo build` would stay
//! green. This gate pins the routing at the source level, the same "sibling
//! blind spot" defense the other maekon-lint gates provide (ADR-075 P-4: no dead
//! gates — it runs under `cargo test -p maekon-lint`).
//!
//! It intentionally asserts STRUCTURE, not behavior; the behavioral decision is
//! unit-tested by `magic_overlay::tests::decide_*` and the platform probe is
//! validated on real desktop targets.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("maekon-lint sits two levels under the workspace root")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    let path = root.join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Extract a function body by brace-matching from the first `{` after the given
/// signature marker to its matching `}`. Panics if the marker is absent so a
/// rename surfaces loudly rather than silently passing an empty slice.
fn fn_body<'a>(src: &'a str, signature_marker: &str) -> &'a str {
    let start = src.find(signature_marker).unwrap_or_else(|| {
        panic!("signature `{signature_marker}` not found — did it get renamed?")
    });
    let after = &src[start..];
    let open = after
        .find('{')
        .unwrap_or_else(|| panic!("no opening brace after `{signature_marker}`"));
    let bytes = after.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &after[open..=i];
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced braces after `{signature_marker}`");
}

// Paths are relative to the workspace root (`clients/maekon-client`), matching
// the other maekon-lint gates (e.g. the ADR-013 LOC baseline keys).
const MOD_RS: &str = "src-tauri/src/magic_overlay/mod.rs";
const SHORTCUTS_RS: &str = "src-tauri/src/setup/shortcuts.rs";

#[test]
fn the_single_gate_method_exists() {
    let root = workspace_root();
    let src = read(&root, MOD_RS);
    assert!(
        src.contains("fn gate_interactive_open("),
        "the single native fullscreen-policy gate `gate_interactive_open` must exist"
    );
}

/// The policy evaluation must have EXACTLY ONE call site, and it must be inside
/// `gate_interactive_open` — this is what makes the gate the single decision
/// point (a second call site anywhere else is a bypass by definition).
#[test]
fn evaluate_fullscreen_policy_is_only_called_by_the_gate() {
    let root = workspace_root();
    let src = read(&root, MOD_RS);

    let call = "self.evaluate_fullscreen_policy()";
    let call_count = src.matches(call).count();
    assert_eq!(
        call_count, 1,
        "`{call}` must be called exactly once (only by the gate); found {call_count} call sites"
    );

    let gate = fn_body(&src, "fn gate_interactive_open(");
    assert!(
        gate.contains(call),
        "the sole `evaluate_fullscreen_policy` call must live inside `gate_interactive_open`"
    );
}

/// `set_interactive(true)` must consult the gate BEFORE `ensure_window()` — the
/// exact ordering #8858 requires (evaluate before a cold window create).
#[test]
fn set_interactive_gates_before_ensure_window() {
    let root = workspace_root();
    let src = read(&root, MOD_RS);
    let body = fn_body(&src, "pub fn set_interactive(");

    let gate_at = body
        .find("gate_interactive_open()")
        .expect("set_interactive must route through gate_interactive_open");
    let ensure_at = body
        .find("ensure_window()")
        .expect("set_interactive must still ensure the window on the allowed path");
    assert!(
        gate_at < ensure_at,
        "set_interactive must evaluate the fullscreen gate BEFORE ensure_window() (#8858)"
    );
}

/// `set_panel_mode` must route an OPEN through the gate; `toggle_panel_mode`
/// must delegate to `set_panel_mode` (so it inherits the gate rather than
/// touching the window directly).
#[test]
fn panel_mode_paths_route_through_the_gate() {
    let root = workspace_root();
    let src = read(&root, MOD_RS);

    let set_panel = fn_body(&src, "pub async fn set_panel_mode(");
    assert!(
        set_panel.contains("gate_interactive_open()"),
        "set_panel_mode must route the OPEN transition through the gate (#8858)"
    );

    let toggle = fn_body(&src, "pub async fn toggle_panel_mode(");
    assert!(
        toggle.contains("set_panel_mode("),
        "toggle_panel_mode must delegate to set_panel_mode so it inherits the gate"
    );
}

/// The suggestions shortcut must be native-first (#8847): toggle the native
/// panel state via `toggle_panel_mode` and emit the resolved state — never the
/// retired emit-only `emit_toggle_suggestions` (lost when the WebView is gone).
#[test]
fn suggestions_shortcut_is_native_first() {
    let root = workspace_root();
    let src = read(&root, SHORTCUTS_RS);
    let body = fn_body(&src, "fn activate_suggestions_shortcut(");

    assert!(
        body.contains("toggle_panel_mode()"),
        "the suggestions shortcut must toggle the AUTHORITATIVE native state (#8847)"
    );
    assert!(
        body.contains("emit_suggestions_panel_state("),
        "the suggestions shortcut must emit the resolved explicit state (#8847)"
    );
}

/// The retired emit-only bypass must not reappear anywhere in the overlay open
/// paths (it lost the toggle when the WebView had been destroyed by the idle
/// policy — the #8847 root cause).
#[test]
fn retired_toggle_bypass_is_gone() {
    let root = workspace_root();
    for rel in [MOD_RS, SHORTCUTS_RS] {
        let src = read(&root, rel);
        assert!(
            !src.contains("emit_toggle_suggestions"),
            "{rel} still references the retired emit-only `emit_toggle_suggestions` bypass (#8847)"
        );
    }
}
