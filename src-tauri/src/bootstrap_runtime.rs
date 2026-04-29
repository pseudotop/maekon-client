use anyhow::Result;
use oneshim_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use oneshim_core::config::AppConfig;
use oneshim_core::config_manager::ConfigManager;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::bootstrap_preflight::BootstrapPreflightCoordinator;
#[cfg(feature = "server")]
use crate::server_runtime_context::ServerBootstrapContext;

pub(crate) struct ManagedBackgroundRuntime {
    handle: Handle,
    shutdown_tx: watch::Sender<bool>,
    join_handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl ManagedBackgroundRuntime {
    pub(crate) fn handle(&self) -> Handle {
        self.handle.clone()
    }

    pub(crate) fn shutdown_blocking(&self) {
        if let Err(e) = self.shutdown_tx.send(true) {
            debug!("channel send failed: {e}");
        }
        let mut guard = match self.join_handle.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                warn!("join handle lock poisoned — recovering inner data");
                poisoned.into_inner()
            }
        };
        if let Some(join_handle) = guard.take() {
            if let Err(e) = join_handle.join() {
                debug!("join failed: {e:?}");
            }
        }
    }
}

impl Drop for ManagedBackgroundRuntime {
    fn drop(&mut self) {
        self.shutdown_blocking();
    }
}

pub(crate) struct BootstrapRuntimeBundle {
    pub(crate) db_path: PathBuf,
    pub(crate) data_dir_path: PathBuf,
    pub(crate) config_manager: ConfigManager,
    pub(crate) config: AppConfig,
    pub(crate) runtime_handle: Handle,
    pub(crate) background_runtime: Arc<ManagedBackgroundRuntime>,
    pub(crate) web_port: Arc<AtomicU16>,
    #[cfg(feature = "server")]
    pub(crate) server: ServerBootstrapContext,
    #[cfg(not(feature = "server"))]
    pub(crate) integration_runtime_status: IntegrationOutboundRuntimeStatus,
}

impl BootstrapRuntimeBundle {
    pub(crate) fn frontend_web_port(&self) -> u16 {
        self.web_port.load(Ordering::Relaxed)
    }

    #[cfg(feature = "server")]
    pub(crate) fn integration_runtime_status(&self) -> IntegrationOutboundRuntimeStatus {
        self.server.integration_runtime_status()
    }

    #[cfg(not(feature = "server"))]
    pub(crate) fn integration_runtime_status(&self) -> IntegrationOutboundRuntimeStatus {
        self.integration_runtime_status.clone()
    }
}

pub(crate) struct BootstrapRuntimeBuilder {
    data_dir_override: Option<PathBuf>,
}

impl BootstrapRuntimeBuilder {
    pub(crate) fn new() -> Self {
        Self {
            data_dir_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_data_dir_override(mut self, data_dir_override: PathBuf) -> Self {
        self.data_dir_override = Some(data_dir_override);
        self
    }

    pub(crate) fn build(&self) -> Result<BootstrapRuntimeBundle> {
        let db_path = resolve_db_path(self.data_dir_override.as_deref());
        let data_dir_path = db_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&data_dir_path)?;
        info!("data directory: {}", data_dir_path.display());

        let config_manager = match ConfigManager::new() {
            Ok(cm) => cm,
            Err(error) => {
                warn!("settings init failure, using defaults: {error}");
                let fallback_path = data_dir_path.join("config.json");
                ConfigManager::with_path(fallback_path)?
            }
        };
        info!("settings file: {:?}", config_manager.config_path());

        let background_runtime = spawn_background_runtime()?;
        let runtime_handle = background_runtime.handle();
        let config = config_manager.get();
        let web_port = Arc::new(AtomicU16::new(config.web.port));
        BootstrapPreflightCoordinator::run(&config, &data_dir_path);

        #[cfg(feature = "server")]
        {
            let server = ServerBootstrapContext::build(&config, &data_dir_path)
                .map_err(|error| std::io::Error::other(error.to_string()))?;

            Ok(BootstrapRuntimeBundle {
                db_path,
                data_dir_path,
                config_manager,
                config,
                runtime_handle,
                background_runtime,
                web_port,
                server,
            })
        }

        #[cfg(not(feature = "server"))]
        {
            Ok(BootstrapRuntimeBundle {
                db_path,
                data_dir_path,
                config_manager,
                config,
                runtime_handle,
                background_runtime,
                web_port,
                integration_runtime_status: IntegrationOutboundRuntimeStatus::default(),
            })
        }
    }
}

fn resolve_db_path(data_dir: Option<&Path>) -> PathBuf {
    data_dir
        .map(|directory| directory.join("oneshim.db"))
        .or_else(|| {
            ConfigManager::data_dir()
                .ok()
                .map(|directory| directory.join("oneshim.db"))
        })
        .unwrap_or_else(|| PathBuf::from("./oneshim.db"))
}

pub(crate) fn spawn_background_runtime() -> Result<Arc<ManagedBackgroundRuntime>> {
    let runtime = Runtime::new()?;
    let handle = runtime.handle().clone();
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let join_handle = std::thread::spawn(move || {
        runtime.block_on(async move {
            while !*shutdown_rx.borrow() {
                if shutdown_rx.changed().await.is_err() {
                    break;
                }
            }
        });
    });
    Ok(Arc::new(ManagedBackgroundRuntime {
        handle,
        shutdown_tx,
        join_handle: Mutex::new(Some(join_handle)),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex as StdMutex, OnceLock};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(())).lock().unwrap()
    }

    fn restore_env_var(key: &str, original: Option<OsString>) {
        match original {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn resolve_db_path_default() {
        let path = resolve_db_path(None);
        assert!(path.to_string_lossy().contains("oneshim.db"));
    }

    #[test]
    fn resolve_db_path_custom() {
        let path = resolve_db_path(Some(Path::new("/tmp/test_data")));
        assert_eq!(path, PathBuf::from("/tmp/test_data/oneshim.db"));
    }

    #[test]
    fn resolve_db_path_uses_app_flavored_data_dir() {
        let _guard = env_lock();
        let temp_dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        let original_appdata = std::env::var_os("APPDATA");
        let original_local_appdata = std::env::var_os("LOCALAPPDATA");
        let original_flavor = std::env::var_os("ONESHIM_APP_FLAVOR");

        std::env::set_var("HOME", temp_dir.path());
        std::env::set_var("APPDATA", temp_dir.path());
        std::env::set_var("LOCALAPPDATA", temp_dir.path());
        std::env::set_var("ONESHIM_APP_FLAVOR", "dev");

        let path = resolve_db_path(None);

        restore_env_var("ONESHIM_APP_FLAVOR", original_flavor);
        restore_env_var("LOCALAPPDATA", original_local_appdata);
        restore_env_var("APPDATA", original_appdata);
        restore_env_var("HOME", original_home);

        assert!(
            path.to_string_lossy().contains("oneshim-dev"),
            "database path should use the flavored app data directory: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("oneshim.db")
        );
    }

    #[test]
    fn builder_uses_override_directory_for_database_path() {
        let bundle = BootstrapRuntimeBuilder::new()
            .with_data_dir_override(PathBuf::from("/tmp/bootstrap_override"))
            .build()
            .expect("bootstrap builder should succeed");

        assert_eq!(
            bundle.db_path,
            PathBuf::from("/tmp/bootstrap_override/oneshim.db")
        );
    }

    #[test]
    fn managed_background_runtime_shuts_down_cleanly() {
        let runtime = spawn_background_runtime().expect("background runtime");
        runtime.shutdown_blocking();
    }
}
