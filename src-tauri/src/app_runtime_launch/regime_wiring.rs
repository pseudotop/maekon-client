use maekon_core::{config::AppConfig, ports::regime_storage::RegimeStoragePort};
use maekon_storage::sqlite::SqliteStorage;
use std::sync::Arc;
use tokio::runtime::Handle;

type RegimeManagerHandle = Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>;
type RegimeClassifierHandle = Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>;
type RegimeStorageHandle = Arc<dyn RegimeStoragePort>;

pub(super) fn build_regime_wiring(
    config: &AppConfig,
    handle: &Handle,
    sqlite_storage: Arc<SqliteStorage>,
) -> (
    RegimeManagerHandle,
    RegimeClassifierHandle,
    RegimeStorageHandle,
) {
    let regime_manager_arc = Arc::new(parking_lot::Mutex::new(
        maekon_analysis::RegimeManager::new(&config.analysis.tiered_memory),
    ));
    let regime_classifier_arc = Arc::new(parking_lot::Mutex::new(
        maekon_analysis::RegimeClassifier::new(1.5),
    ));
    let regime_storage: RegimeStorageHandle = Arc::new(
        maekon_storage::regime_manager_state_store::SqliteRegimeManagerStateStore::new(
            sqlite_storage.connection_arc(),
        ),
    );
    match handle.block_on(regime_storage.load_all()) {
        Ok(regimes) if !regimes.is_empty() => {
            let count = regimes.len();
            regime_manager_arc.lock().hydrate_from(regimes);
            tracing::info!(count, "regime manager hydrated from storage");
        }
        Ok(_) => tracing::info!("regime manager: no persisted state, starting fresh"),
        Err(e) => tracing::warn!(
            error = %e,
            "regime manager hydrate failed; starting fresh"
        ),
    }

    (regime_manager_arc, regime_classifier_arc, regime_storage)
}
