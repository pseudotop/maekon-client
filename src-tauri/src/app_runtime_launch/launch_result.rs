use crate::runtime_state::ManagedStateBuilder;

pub(crate) struct AppRuntimeLaunchResult {
    pub(crate) frontend_web_port: u16,
    /// E20-41 (#4833): ephemeral per-session local-API auth token. setup.rs
    /// registers it as Tauri managed state (for the `get_local_auth_token` IPC
    /// fallback) and injects it into the main WebView so the legit dashboard can
    /// authenticate to `/api`. NEVER persisted; NEVER exposed over HTTP.
    pub(crate) local_auth_token: std::sync::Arc<str>,
    pub(crate) state_builder: ManagedStateBuilder,
    /// 외부 gRPC 슈퍼바이저 태스크 핸들 — 프로세스 수명 동안 살아있어야 함.
    /// 드롭 시 슈퍼바이저가 중단되어 외부 gRPC 서버가 조용히 멈춤 (F-RR-C36-01).
    /// Tauri managed state 로 등록해 앱 종료 시까지 유지.
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) ext_grpc_supervisor: Option<tokio::task::JoinHandle<()>>,
    /// TLS 인증서 감시자 + 만료 모니터 핸들 — Drop 시 두 태스크 모두 abort.
    /// 프로세스 수명 동안 유지해야 TLS 인증서 자동 갱신이 동작함 (F-RR-C28-02).
    /// Tauri managed state 로 등록해 앱 종료 시까지 유지.
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) ext_cert_watcher: Option<maekon_web::grpc::external::tls_config::CertWatcherHandle>,
}

/// E20-41 (#4833): mint a 256-bit hex per-session local-API auth token from the
/// OS CSPRNG. Generated once per launch; never persisted, never logged, never
/// passed via env/argv (which are readable by other local users via /proc & ps).
pub(super) fn generate_local_auth_token() -> std::sync::Arc<str> {
    use std::fmt::Write;
    let bytes: [u8; 32] = rand::random();
    let mut token = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(token, "{byte:02x}");
    }
    std::sync::Arc::from(token)
}
