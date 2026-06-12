//! Contract coverage for macOS AX tree private TC IPC.
//!
//! The macOS AX hierarchy/focus observer TCs need deterministic IPC surfaces
//! without relying on an external operator-only AX dumper.

use std::fs;
use std::path::PathBuf;

fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn crt_prv_axtree_001_has_registered_extract_ax_tree_ipc() {
    let capture_path = src_dir().join("commands/capture.rs");
    let capture_src = fs::read_to_string(&capture_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", capture_path.display(), e));

    assert!(
        capture_src.contains("pub async fn extract_ax_tree"),
        "CRT-PRV-AXTREE-001 requires an extract_ax_tree IPC command"
    );
    assert!(
        capture_src.contains("extract_window_elements("),
        "extract_ax_tree must call AccessibilityExtractor::extract_window_elements"
    );
    assert!(
        capture_src.contains("extract_application_elements("),
        "extract_ax_tree must use app-targeted AX extraction when app_name is provided"
    );
    assert!(
        capture_src.contains("AccessibilityTreeSnapshotResponse")
            && capture_src.contains("AccessibilityTreeElementDto"),
        "extract_ax_tree must return a structured AX tree snapshot DTO"
    );
    assert!(
        capture_src.contains("requested_app_name") && capture_src.contains("matches_requested_app"),
        "extract_ax_tree must report whether the active app matches the requested app"
    );

    let main_path = src_dir().join("main.rs");
    let main_src = fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", main_path.display(), e));
    assert!(
        main_src.contains("commands::capture::extract_ax_tree"),
        "extract_ax_tree must be registered in the Tauri invoke handler"
    );
    assert!(
        main_src.contains("debug_ax_tree_cli_command_from")
            && main_src.contains("MAEKON_DEBUG_AX_TREE_CLI"),
        "AX permission-denial TC needs a gated debug CLI that can run outside the trusted GUI app"
    );
}

#[test]
fn crt_prv_axtree_002_has_registered_focus_observer_ipc() {
    let capture_path = src_dir().join("commands/capture.rs");
    let capture_src = fs::read_to_string(&capture_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", capture_path.display(), e));

    assert!(
        capture_src.contains("pub async fn start_ax_focus_observer")
            && capture_src.contains("pub async fn poll_ax_focus_observer")
            && capture_src.contains("pub async fn stop_ax_focus_observer"),
        "CRT-PRV-AXTREE-002 requires debug IPC to start, poll, and stop the AX focus observer"
    );
    assert!(
        capture_src.contains("FocusObserverHandle::start")
            && capture_src.contains("AX_FOCUS_OBSERVER")
            && capture_src.contains("focus_changed"),
        "AX focus observer IPC must own the observer lifecycle and expose focus_changed state"
    );
    assert!(
        capture_src.contains("permission.permission_denied")
            && capture_src.contains("accessibility.observer_not_running"),
        "AX focus observer IPC must return structured permission and lifecycle errors"
    );

    let observer_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../crates/maekon-vision/src/accessibility/macos/observer.rs");
    let observer_src = fs::read_to_string(&observer_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", observer_path.display(), e));
    assert!(
        observer_src.contains("AX_FOCUSED_UI_ELEMENT_CHANGED_NOTIFICATION")
            && observer_src.contains("AX_FOCUSED_WINDOW_CHANGED_NOTIFICATION"),
        "AX focus observer must subscribe to both UI-element and focused-window notifications"
    );

    let main_path = src_dir().join("main.rs");
    let main_src = fs::read_to_string(&main_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", main_path.display(), e));
    assert!(
        main_src.contains("commands::capture::start_ax_focus_observer")
            && main_src.contains("commands::capture::poll_ax_focus_observer")
            && main_src.contains("commands::capture::stop_ax_focus_observer"),
        "AX focus observer debug IPC must be registered in the Tauri invoke handler"
    );
    assert!(
        main_src.contains("commands::capture_status::debug_focus_window"),
        "AX focus observer TC needs a mouse-free debug IPC to shift focus between app windows"
    );
}
