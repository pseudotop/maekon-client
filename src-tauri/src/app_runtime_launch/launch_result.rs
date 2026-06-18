use crate::runtime_state::ManagedStateBuilder;

pub(crate) struct AppRuntimeLaunchResult {
    pub(crate) frontend_web_port: u16,
    /// E20-41 (#4833): ephemeral per-session local-API auth token. setup.rs
    /// registers it as Tauri managed state (for the `get_local_auth_token` IPC
    /// fallback) and injects it into the main WebView so the legit dashboard can
    /// authenticate to `/api`. NEVER persisted; NEVER exposed over HTTP.
    pub(crate) local_auth_token: std::sync::Arc<str>,
    pub(crate) state_builder: ManagedStateBuilder,
    /// External gRPC supervisor task handle — must stay alive for the process
    /// lifetime. Dropping it stops the supervisor, silently halting the external
    /// gRPC server (F-RR-C36-01). Registered as Tauri managed state so it lives
    /// until app shutdown.
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) ext_grpc_supervisor: Option<tokio::task::JoinHandle<()>>,
    /// TLS certificate watcher + expiry monitor handle — both tasks abort on
    /// Drop. Must stay alive for the process lifetime for TLS certificate
    /// auto-renewal to work (F-RR-C28-02). Registered as Tauri managed state so
    /// it lives until app shutdown.
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
