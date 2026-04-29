#[cfg(feature = "server")]
use oneshim_storage::sqlite::SqliteStorage;
use oneshim_web::RealtimeEvent;
#[cfg(feature = "server")]
use std::sync::Arc;
use tauri::AppHandle;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, watch};

#[cfg(feature = "server")]
use crate::integration_prompt_delivery::TauriIntegrationPromptPresenter;
#[cfg(feature = "server")]
use crate::integration_runtime::IntegrationRuntimeBundle;
use crate::runtime_bridges::RuntimeBridgeSpawner;

pub(crate) struct BackgroundRuntimeCoordinator<'a> {
    runtime_handle: &'a Handle,
    app_handle: AppHandle,
    event_tx: broadcast::Sender<RealtimeEvent>,
    shutdown_tx: watch::Sender<bool>,
}

impl<'a> BackgroundRuntimeCoordinator<'a> {
    pub(crate) fn new(runtime_handle: &'a Handle, app_handle: AppHandle) -> Self {
        let (event_tx, _event_rx) = broadcast::channel::<RealtimeEvent>(256);
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        Self {
            runtime_handle,
            app_handle,
            event_tx,
            shutdown_tx,
        }
    }

    pub(crate) fn event_tx(&self) -> broadcast::Sender<RealtimeEvent> {
        self.event_tx.clone()
    }

    pub(crate) fn shutdown_tx(&self) -> watch::Sender<bool> {
        self.shutdown_tx.clone()
    }

    pub(crate) fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub(crate) fn agent_event_tx(
        &self,
        web_enabled: bool,
    ) -> Option<broadcast::Sender<RealtimeEvent>> {
        web_enabled.then(|| self.event_tx())
    }

    #[cfg(feature = "server")]
    pub(crate) fn spawn_integration_loops(
        &self,
        integration_runtime: &IntegrationRuntimeBundle,
        sqlite_storage: Arc<SqliteStorage>,
    ) {
        let suggestion_query =
            sqlite_storage as Arc<dyn oneshim_core::ports::integration::LocalSuggestionQueryPort>;
        let prompt_presenter = Arc::new(TauriIntegrationPromptPresenter::new(
            self.app_handle.clone(),
        ))
            as Arc<dyn oneshim_core::ports::integration::IntegrationPromptPresenterPort>;
        let integration_background_loops =
            integration_runtime.background_loops(suggestion_query, prompt_presenter);
        integration_background_loops.spawn_on(self.runtime_handle, &self.shutdown_tx);
    }

    pub(crate) fn spawn_runtime_bridges(&self) {
        RuntimeBridgeSpawner::spawn_os_signal_bridge(self.runtime_handle, &self.shutdown_tx);
        RuntimeBridgeSpawner::spawn_realtime_event_bridge(
            self.runtime_handle,
            &self.app_handle,
            &self.event_tx,
        );
    }
}
