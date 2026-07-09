//! Regression test — F-RR-C36-01: guarantee the lifetime of the
//! ext_grpc_supervisor + ext_cert_watcher handles.
//!
//! Problem: both handles were dropped at the end of the
//! `if config.web.enabled { ... }` block, so right after `build_and_spawn`
//! returned the external gRPC server and TLS watch tasks were silently aborted
//! (defeating the intent of F-RR-C28-02).
//!
//! Fix: return the handles as fields of the `AppRuntimeLaunchResult` struct and
//! bind them to the Tauri app lifetime via `app.manage()` in `setup.rs`.
//!
//! Run: `cargo test -p maekon-app --test ext_grpc_handle_lifetime`

/// Check that the `_`-prefixed local binding pattern has been removed from
/// mod.rs. If `let _ext_grpc_supervisor = ...` or `let _ext_cert_watcher = ...`
/// is present, that variable is dropped at the end of the block, re-introducing
/// the F-RR-C36-01 regression.
#[test]
fn ext_grpc_handles_not_dropped_at_block_end() {
    let src = include_str!("../src/app_runtime_launch/mod.rs");

    // There must be no incorrect pattern binding the handles to a `_`-prefixed local.
    assert!(
        !src.contains("let _ext_grpc_supervisor"),
        "F-RR-C36-01 regression: `let _ext_grpc_supervisor` found in mod.rs — \
         underscore-prefixed local drops at block end, aborting the supervisor task. \
         Handle must be assigned to the outer `ext_grpc_supervisor` variable."
    );
    assert!(
        !src.contains("let _ext_cert_watcher"),
        "F-RR-C36-01 regression: `let _ext_cert_watcher` found in mod.rs — \
         underscore-prefixed local drops at block end, aborting cert + expiry tasks. \
         Handle must be assigned to the outer `ext_cert_watcher` variable."
    );
}

/// Check that both handle fields are declared on the `AppRuntimeLaunchResult`
/// struct. If a field is missing, the handle is dropped before `build_and_spawn`
/// returns.
#[test]
fn app_runtime_launch_result_carries_ext_grpc_fields() {
    let src = include_str!("../src/app_runtime_launch/launch_result.rs");

    assert!(
        src.contains("pub(crate) ext_grpc_supervisor"),
        "F-RR-C36-01: AppRuntimeLaunchResult must declare `pub(crate) ext_grpc_supervisor` field"
    );
    assert!(
        src.contains("pub(crate) ext_cert_watcher"),
        "F-RR-C36-01: AppRuntimeLaunchResult must declare `pub(crate) ext_cert_watcher` field"
    );
}

/// Check that the outer declaration pattern (outside the if block) is present.
/// The handles must first be initialized to `None`, then filled in inside the
/// if block.
#[test]
fn ext_grpc_handles_declared_before_if_block() {
    let src = include_str!("../src/app_runtime_launch/mod.rs");

    // Outer declaration pattern: `let mut ext_grpc_supervisor: Option<...> = None;`
    assert!(
        src.contains("let mut ext_grpc_supervisor"),
        "F-RR-C36-01: outer `let mut ext_grpc_supervisor` declaration not found in mod.rs — \
         handle must be declared before the `if config.web.enabled` block"
    );
    assert!(
        src.contains("let mut ext_cert_watcher"),
        "F-RR-C36-01: outer `let mut ext_cert_watcher` declaration not found in mod.rs — \
         handle must be declared before the `if config.web.enabled` block"
    );
}

/// Check that the setup module registers the handles into Tauri managed state
/// via `app.manage`. Without registering them in managed state, the handles
/// vanish once AppRuntimeLaunchResult is dropped.
///
/// Hygiene (#7909 PR): #7823 moved `src/setup.rs` into the `src/setup/`
/// directory module but left this `include_str!` pointing at the deleted flat
/// file, breaking `--all-targets` compilation. The `app.manage(ExtGrpcHandles`
/// call now lives in `src/setup/mod.rs`.
#[test]
fn setup_registers_ext_grpc_handles_as_managed_state() {
    let src = include_str!("../src/setup/mod.rs");

    assert!(
        src.contains("ExtGrpcHandles"),
        "F-RR-C36-01: setup/mod.rs must pass ext_grpc_supervisor + ext_cert_watcher to \
         `app.manage(ExtGrpcHandles {{ ... }})` so Tauri owns the handles for the app lifetime"
    );
}

/// Test-harness discovery anchor.
#[test]
fn ext_grpc_handle_lifetime_harness_is_wired() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ext_grpc_handle_lifetime.rs");
    assert!(
        path.is_file(),
        "ext_grpc_handle_lifetime test file should exist at {}",
        path.display()
    );
}
