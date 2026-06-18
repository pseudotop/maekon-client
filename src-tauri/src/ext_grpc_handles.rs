/// F-RR-C36-01: keep the external gRPC supervisor + TLS watcher handles in
/// Tauri managed state.
///
/// Wrapping the handles returned by `build_and_spawn` in this struct and
/// registering it with `app.manage()` lets Tauri retain ownership until the app
/// shuts down, so the tasks are cleaned up properly on Drop.
///
/// ## Background
/// - `ext_grpc_supervisor`: JoinHandle — on Drop the supervisor task is aborted,
///   which stops the external gRPC accept loop.
/// - `ext_cert_watcher`: CertWatcherHandle — on Drop the two cert-watch + expiry
///   monitor tasks are aborted (F-RR-C28-02).
/// In the previous implementation both handles were dropped at the end of the
/// `if config.web.enabled { ... }` block, which caused a regression where the
/// server stopped silently right after `build_and_spawn` returned.
#[cfg(feature = "grpc-dashboard-external")]
#[allow(dead_code)]
pub(crate) struct ExtGrpcHandles {
    /// External gRPC supervisor JoinHandle — Some means the server is running.
    pub(crate) supervisor: Option<tokio::task::JoinHandle<()>>,
    /// TLS certificate watcher handle — on Drop the cert + expiry tasks are aborted.
    pub(crate) cert_watcher: Option<maekon_web::grpc::external::tls_config::CertWatcherHandle>,
}
