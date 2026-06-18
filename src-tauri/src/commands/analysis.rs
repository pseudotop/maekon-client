use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{AppState, ConfigRuntimeState, EmbeddingRuntimeState};

use super::deep_merge;

// Note: `validate_analysis_config` returns Result<(), String> (preserved for
// existing tests that substring-match). Its errors flow through
// `ConfigManager::update_with`, which wraps them as CoreError::Config{Invalid}
// — then From<CoreError> for IpcError produces `{code: "config.invalid", ...}`.
// No local helper wrapper needed.

/// Get the analysis configuration.
///
/// AnalysisConfig contains no sensitive fields (no API keys, credentials).
/// If sensitive fields are added in the future, apply redact_sensitive_fields().
#[command]
pub async fn get_analysis_config(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<maekon_core::config::AnalysisConfig, IpcError> {
    let config = state.config_manager().get();
    Ok(config.analysis.clone())
}

/// Validate an AnalysisConfig, returning Err(String) on constraint violation.
pub(crate) fn validate_analysis_config(
    config: &maekon_core::config::AnalysisConfig,
) -> Result<(), String> {
    if config.min_confidence < 0.0 || config.min_confidence > 1.0 {
        return Err("min_confidence must be between 0.0 and 1.0".to_string());
    }
    if config.max_suggestions == 0 {
        return Err("max_suggestions must be at least 1".to_string());
    }
    if config.throttle_secs == 0 {
        return Err("throttle_secs must be at least 1".to_string());
    }
    if config.interval_secs < 10 {
        return Err("interval_secs must be at least 10".to_string());
    }
    if config.full_interval_secs < config.interval_secs {
        return Err("full_interval_secs must be >= interval_secs".to_string());
    }
    Ok(())
}

/// Partially update the analysis configuration (patch merge).
///
/// Uses `update_with` to hold the write lock for the entire read-modify-write
/// cycle, preventing TOCTOU races between concurrent callers.
#[command]
pub async fn update_analysis_config(
    state: tauri::State<'_, ConfigRuntimeState>,
    patch: serde_json::Value,
) -> Result<maekon_core::config::AnalysisConfig, IpcError> {
    let updated = state
        .config_manager()
        .update_with(|config| {
            // Deep-merge patch into current analysis section
            let mut analysis_json =
                serde_json::to_value(&config.analysis).map_err(|e| e.to_string())?;
            deep_merge(&mut analysis_json, patch.clone());

            // Deserialize back and validate
            let new_analysis: maekon_core::config::AnalysisConfig =
                serde_json::from_value(analysis_json)
                    .map_err(|e| format!("Invalid config: {e}"))?;
            validate_analysis_config(&new_analysis)?;

            config.analysis = new_analysis;
            Ok(())
        })
        .map_err(IpcError::from)?;

    Ok(updated.analysis)
}

/// Analysis pipeline status response.
#[derive(Serialize)]
pub struct AnalysisStatusResponse {
    pub enabled: bool,
    pub provider_configured: bool,
    pub provider_name: Option<String>,
    pub throttle_secs: u64,
    pub interval_secs: u64,
    pub full_interval_secs: u64,
    pub min_confidence: f64,
    pub max_suggestions: usize,
}

/// Query the analysis pipeline status (enabled, whether a provider is configured, etc.).
#[command]
pub async fn get_analysis_status(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<AnalysisStatusResponse, IpcError> {
    let config = state.config_manager().get();
    let provider_name = config
        .ai_provider
        .llm_api
        .as_ref()
        .map(|api| format!("{:?}", api.provider_type));
    Ok(AnalysisStatusResponse {
        enabled: config.analysis.enabled,
        provider_configured: config.ai_provider.llm_api.is_some(),
        provider_name,
        throttle_secs: config.analysis.throttle_secs,
        interval_secs: config.analysis.interval_secs,
        full_interval_secs: config.analysis.full_interval_secs,
        min_confidence: config.analysis.min_confidence,
        max_suggestions: config.analysis.max_suggestions,
    })
}

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
    use super::*;

    fn default_analysis() -> maekon_core::config::AnalysisConfig {
        maekon_core::config::AnalysisConfig::default()
    }

    #[test]
    fn validate_analysis_rejects_min_confidence_above_one() {
        let mut cfg = default_analysis();
        cfg.min_confidence = 1.1;
        let err = validate_analysis_config(&cfg).unwrap_err();
        assert!(err.contains("min_confidence"), "got: {err}");
    }

    #[test]
    fn validate_analysis_rejects_min_confidence_below_zero() {
        let mut cfg = default_analysis();
        cfg.min_confidence = -0.1;
        let err = validate_analysis_config(&cfg).unwrap_err();
        assert!(err.contains("min_confidence"), "got: {err}");
    }

    #[test]
    fn validate_analysis_rejects_zero_max_suggestions() {
        let mut cfg = default_analysis();
        cfg.max_suggestions = 0;
        let err = validate_analysis_config(&cfg).unwrap_err();
        assert!(err.contains("max_suggestions"), "got: {err}");
    }

    #[test]
    fn validate_analysis_rejects_interval_below_ten() {
        let mut cfg = default_analysis();
        cfg.interval_secs = 9;
        let err = validate_analysis_config(&cfg).unwrap_err();
        assert!(err.contains("interval_secs"), "got: {err}");
    }

    #[test]
    fn validate_analysis_rejects_full_interval_below_interval() {
        let mut cfg = default_analysis();
        cfg.interval_secs = 60;
        cfg.full_interval_secs = 30;
        let err = validate_analysis_config(&cfg).unwrap_err();
        assert!(err.contains("full_interval_secs"), "got: {err}");
    }

    #[test]
    fn validate_analysis_rejects_zero_throttle() {
        let mut cfg = default_analysis();
        cfg.throttle_secs = 0;
        let err = validate_analysis_config(&cfg).unwrap_err();
        assert!(err.contains("throttle_secs"), "got: {err}");
    }

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

    #[test]
    fn validate_analysis_accepts_valid_defaults() {
        // AnalysisConfig::default() must satisfy all validate_analysis_config
        // checks: 0.0<=min_confidence<=1.0, max_suggestions>=1,
        // throttle_secs>=1, interval_secs>=10, full_interval_secs>=interval_secs.
        let cfg = default_analysis();
        validate_analysis_config(&cfg)
            .expect("default AnalysisConfig must pass all validate_analysis_config checks");
    }
}
