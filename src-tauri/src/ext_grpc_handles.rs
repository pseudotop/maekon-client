/// F-RR-C36-01: 외부 gRPC 슈퍼바이저 + TLS 감시자 핸들을 Tauri managed state 로 유지.
///
/// `build_and_spawn`이 반환한 핸들을 이 구조체로 감싸 `app.manage()` 에 등록하면
/// Tauri가 앱 종료 시까지 소유권을 유지하고 Drop 시 태스크가 정상 정리된다.
///
/// ## 배경
/// - `ext_grpc_supervisor`: JoinHandle — Drop 시 슈퍼바이저 태스크가 abort 되어
///   외부 gRPC accept loop 이 멈춤.
/// - `ext_cert_watcher`: CertWatcherHandle — Drop 시 cert 감시 + 만료 모니터 태스크
///   두 개가 abort 됨 (F-RR-C28-02).
/// 이전 구현에서는 두 핸들이 `if config.web.enabled { ... }` 블록 끝에서 Drop되어
/// `build_and_spawn` 반환 직후 서버가 조용히 중단되는 regression 이 있었다.
#[cfg(feature = "grpc-dashboard-external")]
#[allow(dead_code)]
pub(crate) struct ExtGrpcHandles {
    /// 외부 gRPC 슈퍼바이저 JoinHandle — Some 이면 서버가 실행 중.
    pub(crate) supervisor: Option<tokio::task::JoinHandle<()>>,
    /// TLS 인증서 감시자 핸들 — Drop 시 cert + 만료 태스크 abort.
    pub(crate) cert_watcher: Option<maekon_web::grpc::external::tls_config::CertWatcherHandle>,
}
