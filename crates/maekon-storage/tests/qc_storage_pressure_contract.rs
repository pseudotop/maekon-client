use std::fs;
use std::path::PathBuf;

#[test]
fn qc_storage_pressure_fixture_is_debug_only_and_bounded() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let module_source = fs::read_to_string(root.join("src/sqlite/web_storage_impl/mod.rs"))
        .expect("read web-storage module source");
    let adapter_source =
        fs::read_to_string(root.join("src/sqlite/web_storage_impl/backup_segment_gui_storage.rs"))
            .expect("read backup adapter source");
    let fixture_source =
        fs::read_to_string(root.join("src/sqlite/web_storage_impl/qc_storage_pressure.rs"))
            .expect("read QC storage-pressure source");

    assert!(module_source.contains("#[cfg(debug_assertions)]\nmod qc_storage_pressure;"));
    assert_eq!(
        adapter_source
            .matches("#[cfg(debug_assertions)]\n        if let Some(fault) = super::qc_storage_pressure::export_fault_from_env()")
            .count(),
        3,
        "all three scoped export adapters must keep the debug-only call-site gate"
    );
    for gate in [
        "MAEKON_DEBUG_QC_FIXTURE_CLI",
        "MAEKON_TC_ISOLATED_PROFILE",
        "MAEKON_APP_FLAVOR",
        "MAEKON_DEBUG_QC_STORAGE_PRESSURE_FIXTURE",
        "MAEKON_QC_STORAGE_PRESSURE_MODE",
    ] {
        assert!(
            fixture_source.contains(gate),
            "missing fixture gate: {gate}"
        );
    }
    assert!(fixture_source.contains("low-disk-export"));
    assert!(fixture_source.contains("locked-export"));
    assert!(!fixture_source.contains("pub fn"));
    assert!(!fixture_source.contains("pub struct"));
}
