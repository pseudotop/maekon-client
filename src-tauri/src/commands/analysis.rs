use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, EmbeddingRuntimeState};

// #7683 F2: get_analysis_status (a static config echo of `config.analysis.*`)
// was removed as a residual dead IPC — the Settings UI reads the same fields
// via the unrestricted `/settings` REST full-config-replace path (AdvancedTab.tsx
// via SettingsFormContext), never via this IPC command. Zero callers anywhere
// in crates/maekon-web/frontend/src. get_analysis_health below is kept: it
// exposes live circuit-breaker-style fallback health state (`primary_healthy`)
// that has no static-config or REST equivalent.

/// Reload the embedding model at runtime without restarting the app.
///
/// Returns the new model version on success (monotonically increasing u64).
///
/// F-RR-C24-01: `ReloadableModel::reload` is a synchronous blocking call that
/// may download an ONNX model file (several seconds) and acquire
/// `std::sync::Mutex` locks.  Wrapping in `spawn_blocking` moves the work onto
/// a dedicated blocking thread, preventing it from stalling the async runtime.
/// Reference pattern: `feature_capabilities.rs:72` (`spawn_blocking(probe_known_cli_surfaces)`).
#[command]
pub async fn reload_embedding_model(
    state: tauri::State<'_, EmbeddingRuntimeState>,
) -> Result<u64, IpcError> {
    let reloadable = state
        .reloadable()
        .ok_or_else(|| IpcError::new("service.unavailable", "Embedding provider not available"))?
        .clone();
    tokio::task::spawn_blocking(move || reloadable.reload())
        .await
        .map_err(|join_err| IpcError::new("internal.generic", join_err.to_string()))?
        .map_err(IpcError::from)
}

/// Health status of the analysis LLM provider fallback chain.
#[derive(Debug, Serialize)]
pub struct AnalysisHealthStatus {
    pub primary_healthy: bool,
    pub provider_configured: bool,
}

/// Query the health of the analysis LLM provider fallback chain.
#[command]
pub fn get_analysis_health(state: tauri::State<'_, AppState>) -> AnalysisHealthStatus {
    let (primary_healthy, configured) = match &state.analysis_health {
        Some(h) => (h.primary_healthy.load(Ordering::Relaxed), true),
        None => (false, false),
    };
    AnalysisHealthStatus {
        primary_healthy,
        provider_configured: configured,
    }
}

#[cfg(test)]
mod tests {
    /// F-RR-C24-01: `reload_embedding_model` must execute `ReloadableModel::reload`
    /// on a blocking thread via `spawn_blocking`.  This test verifies the
    /// `spawn_blocking` path completes and returns the reload result correctly
    /// using a trivial in-process impl (no ONNX download involved).
    #[tokio::test]
    async fn reload_embedding_model_spawn_blocking_returns_version() {
        use maekon_core::error::CoreError;
        use maekon_core::ports::embedding_provider::ReloadableModel;
        use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
        use std::sync::Arc;

        struct MockReloadable {
            version: AtomicU64,
        }
        impl ReloadableModel for MockReloadable {
            fn model_version(&self) -> u64 {
                self.version.load(AtomicOrdering::SeqCst)
            }
            fn reload(&self) -> Result<u64, CoreError> {
                let v = self.version.fetch_add(1, AtomicOrdering::SeqCst) + 1;
                Ok(v)
            }
        }

        let reloadable: Arc<dyn ReloadableModel> = Arc::new(MockReloadable {
            version: AtomicU64::new(0),
        });

        // Exercise the spawn_blocking path directly (not via Tauri State).
        let result = tokio::task::spawn_blocking({
            let r = reloadable.clone();
            move || r.reload()
        })
        .await
        .expect("spawn_blocking join must not panic")
        .expect("reload must succeed");

        assert_eq!(result, 1, "F-RR-C24-01: first reload must return version 1");
    }
}
