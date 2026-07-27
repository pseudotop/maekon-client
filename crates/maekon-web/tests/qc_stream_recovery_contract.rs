use std::fs;
use std::path::PathBuf;

#[test]
fn qc_stream_recovery_fixture_is_debug_only_and_bounded() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let lib_source = fs::read_to_string(root.join("src/lib.rs")).expect("read web lib source");
    let fixture_source = fs::read_to_string(root.join("src/qc_stream_recovery.rs"))
        .expect("read QC stream-recovery source");
    let call_sites = [
        "src/services/update_service.rs",
        "src/services/stream_service.rs",
        "src/services/automation_gui_service.rs",
    ];

    assert!(lib_source.contains("#[cfg(debug_assertions)]\nmod qc_stream_recovery;"));
    for relative_path in call_sites {
        let source = fs::read_to_string(root.join(relative_path)).expect("read stream call site");
        assert!(
            source.contains("#[cfg(debug_assertions)]\n        let "),
            "{relative_path} must keep the debug-only call-site gate"
        );
        assert!(source.contains("crate::qc_stream_recovery::stream_limit("));
    }

    for gate in [
        "MAEKON_DEBUG_QC_FIXTURE_CLI",
        "MAEKON_TC_ISOLATED_PROFILE",
        "MAEKON_APP_FLAVOR",
        "MAEKON_DEBUG_QC_STREAM_RECOVERY_FIXTURE",
        "MAEKON_QC_STREAM_RECOVERY_MODE",
    ] {
        assert!(fixture_source.contains(gate), "missing runtime gate {gate}");
    }
    assert!(fixture_source.contains("drop-first-two"));
}
