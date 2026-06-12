//! Regression test — F-RR-C36-01: ext_grpc_supervisor + ext_cert_watcher 핸들 수명 보장.
//!
//! 문제: 두 핸들이 `if config.web.enabled { ... }` 블록 끝에서 Drop 되어
//! `build_and_spawn` 반환 직후 외부 gRPC 서버와 TLS 감시 태스크가 조용히 중단됨
//! (F-RR-C28-02 의도 무력화).
//!
//! 수정: 핸들을 `AppRuntimeLaunchResult` 구조체 필드로 반환하고,
//! `setup.rs` 에서 `app.manage()` 로 Tauri 앱 수명에 바인딩.
//!
//! Run: `cargo test -p maekon-app --test ext_grpc_handle_lifetime`

/// `_`-접두사 로컬 바인딩 패턴이 mod.rs 에서 제거됐는지 확인.
/// `let _ext_grpc_supervisor = ...` 또는 `let _ext_cert_watcher = ...` 형태가
/// 존재하면 해당 변수는 블록 끝에서 Drop 되어 F-RR-C36-01 regression 재발.
#[test]
fn ext_grpc_handles_not_dropped_at_block_end() {
    let src = include_str!("../src/app_runtime_launch/mod.rs");

    // `_`-접두사 로컬로 핸들을 바인딩하는 잘못된 패턴이 없어야 함.
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

/// `AppRuntimeLaunchResult` 구조체에 두 핸들 필드가 선언됐는지 확인.
/// 필드 누락 시 핸들이 `build_and_spawn` 반환 전에 Drop 됨.
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

/// if 블록 밖 외부 선언 패턴이 존재하는지 확인.
/// 핸들을 먼저 `None` 으로 초기화 후 if 블록 내에서 채워야 함.
#[test]
fn ext_grpc_handles_declared_before_if_block() {
    let src = include_str!("../src/app_runtime_launch/mod.rs");

    // 외부 선언 패턴: `let mut ext_grpc_supervisor: Option<...> = None;`
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

/// setup.rs 에서 `app.manage` 로 핸들을 Tauri managed state 에 등록하는지 확인.
/// managed state 에 등록하지 않으면 AppRuntimeLaunchResult 가 drop 된 후 핸들도 사라짐.
#[test]
fn setup_registers_ext_grpc_handles_as_managed_state() {
    let src = include_str!("../src/setup.rs");

    assert!(
        src.contains("ExtGrpcHandles"),
        "F-RR-C36-01: setup.rs must pass ext_grpc_supervisor + ext_cert_watcher to \
         `app.manage(ExtGrpcHandles {{ ... }})` so Tauri owns the handles for the app lifetime"
    );
}

/// 테스트 하네스 발견 앵커.
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
