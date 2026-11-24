//! Actual external-server startup and connection helpers.

use std::sync::Arc;
use std::time::Duration;

use super::super::{spawn_config::ExternalGrpcSpawnConfig, ServeExternalError};
use super::{install_rustls_crypto_provider, FixtureError};

pub type ServerHandle = tokio::task::JoinHandle<Result<(), ServeExternalError>>;
type ServerWithReload = (ServerHandle, tokio::task::JoinHandle<()>, u16);

/// Connect to an already-ready, plaintext loopback server without hidden retries.
pub async fn connect_loopback(port: u16) -> Result<tonic::transport::Channel, FixtureError> {
    Ok(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))?
            .connect()
            .await?,
    )
}

struct AbortStartupOnDrop {
    task: tokio::task::AbortHandle,
    armed: bool,
}

impl Drop for AbortStartupOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.task.abort();
        }
    }
}

async fn await_server_readiness(
    mut server_handle: ServerHandle,
    ready_rx: tokio::sync::oneshot::Receiver<std::net::SocketAddr>,
) -> Result<(ServerHandle, std::net::SocketAddr), FixtureError> {
    let mut guard = AbortStartupOnDrop {
        task: server_handle.abort_handle(),
        armed: true,
    };
    // The deadline includes recovering the task outcome after a closed ready
    // channel. Closing that channel alone does not prove the task has exited.
    let startup = async {
        match ready_rx.await {
            Ok(address) => Ok(address),
            Err(_) => {
                let outcome = (&mut server_handle).await?;
                Err(outcome
                    .err()
                    .map(FixtureError::Server)
                    .unwrap_or(FixtureError::StoppedBeforeReady))
            }
        }
    };
    let address = match tokio::time::timeout(Duration::from_secs(5), startup).await {
        Ok(result) => result?,
        Err(_) => {
            server_handle.abort();
            if let Err(error) = server_handle.await {
                if !error.is_cancelled() {
                    return Err(FixtureError::Task(error));
                }
            }
            return Err(FixtureError::StartupTimeout);
        }
    };
    guard.armed = false;
    Ok((server_handle, address))
}

/// Start the real server and config reload task, returning their actual outcomes.
/// Port zero is supported: the server reports its own OS-assigned bound port.
/// Bind/TLS/startup errors are returned, and canceling startup aborts its task.
/// The caller owns both returned handles and must signal shutdown and join them.
pub async fn spawn_server_with_config_manager(
    cfg: ExternalGrpcSpawnConfig,
    cfg_mgr: Arc<maekon_core::config_manager::ConfigManager>,
) -> Result<ServerWithReload, FixtureError> {
    install_rustls_crypto_provider();
    if *cfg.shutdown_rx.borrow() {
        return Err(FixtureError::StoppedBeforeReady);
    }
    let live = cfg.live.clone();
    let metrics = cfg.metrics.clone();
    let shutdown_rx = cfg.shutdown_rx.clone();
    // Subscribe before startup so config changes during TLS preparation are seen.
    let config_rx = cfg_mgr.subscribe();
    let service = crate::grpc::DashboardServiceImpl::from_external_spawn_config(&cfg);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server_handle = tokio::spawn(super::super::serve_external_inner(
        cfg,
        service,
        "external_grpc(test)",
        Some(ready_tx),
    ));
    let (server_handle, address) = await_server_readiness(server_handle, ready_rx).await?;
    // No reload task is created on a failed startup. A watch subscription keeps
    // any intervening update ready for the newly started task.
    let reload_handle = tokio::spawn(super::super::config_reload::run_config_reload(
        live,
        metrics,
        config_rx,
        shutdown_rx,
    ));
    Ok((server_handle, reload_handle, address.port()))
}

/// Build a test gRPC request with a placeholder bearer token header.
///
/// Inserts `Authorization: Bearer TEST_TOKEN_PLACEHOLDER` into the request
/// metadata.
///
/// **IMPORTANT**: The current token is a literal `"TEST_TOKEN_PLACEHOLDER"` that will
/// fail validation against any real `JwtVerifier`. This helper exists to let Task 9.4
/// G3-test scaffolding compile; the actual JWT minting belongs to Task 9.4 itself via
/// [`super::test_mint_jwt`] (present in `tests/external_grpc_integration.rs`).
///
/// Callers: **replace the placeholder with a real minted token** before running the
/// test against a configured server, OR use this only with a bypass-auth server setup.
pub fn req_with_valid_auth<T>(body: T) -> tonic::Request<T> {
    let mut req = tonic::Request::new(body);
    req.metadata_mut().insert(
        "authorization",
        tonic::metadata::MetadataValue::from_static("Bearer TEST_TOKEN_PLACEHOLDER"),
    );
    req
}

/// Use a fixture handler through the same production TLS/auth/audit layer stack.
pub async fn serve_external_with_service<T>(
    cfg: ExternalGrpcSpawnConfig,
    service: T,
) -> Result<(), ServeExternalError>
where
    T: crate::proto::dashboard::v1::dashboard_service_server::DashboardService,
{
    super::super::serve_external_inner(cfg, service, "external_grpc(test)", None).await
}

#[cfg(test)]
mod tests;
