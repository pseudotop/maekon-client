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

/// 분석 설정 조회
///
/// AnalysisConfig contains no sensitive fields (no API keys, credentials).
/// If sensitive fields are added in the future, apply redact_sensitive_fields().
#[command]
pub async fn get_analysis_config(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<oneshim_core::config::AnalysisConfig, IpcError> {
    let config = state.config_manager().get();
    Ok(config.analysis.clone())
}

/// Validate an AnalysisConfig, returning Err(String) on constraint violation.
pub(crate) fn validate_analysis_config(
    config: &oneshim_core::config::AnalysisConfig,
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

/// 분석 설정 부분 업데이트 (patch merge)
///
/// Uses `update_with` to hold the write lock for the entire read-modify-write
/// cycle, preventing TOCTOU races between concurrent callers.
#[command]
pub async fn update_analysis_config(
    state: tauri::State<'_, ConfigRuntimeState>,
    patch: serde_json::Value,
) -> Result<oneshim_core::config::AnalysisConfig, IpcError> {
    let updated = state
        .config_manager()
        .update_with(|config| {
            // Deep-merge patch into current analysis section
            let mut analysis_json =
                serde_json::to_value(&config.analysis).map_err(|e| e.to_string())?;
            deep_merge(&mut analysis_json, patch.clone());

            // Deserialize back and validate
            let new_analysis: oneshim_core::config::AnalysisConfig =
                serde_json::from_value(analysis_json)
                    .map_err(|e| format!("Invalid config: {e}"))?;
            validate_analysis_config(&new_analysis)?;

            config.analysis = new_analysis;
            Ok(())
        })
        .map_err(IpcError::from)?;

    Ok(updated.analysis)
}

/// 분석 파이프라인 상태 응답
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

/// 분석 파이프라인 상태 조회 (enabled, provider 설정 여부 등)
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
#[command]
pub async fn reload_embedding_model(
    state: tauri::State<'_, EmbeddingRuntimeState>,
) -> Result<u64, IpcError> {
    let reloadable = state
        .reloadable()
        .ok_or_else(|| IpcError::new("service.unavailable", "Embedding provider not available"))?;
    reloadable.reload().map_err(IpcError::from)
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

    fn default_analysis() -> oneshim_core::config::AnalysisConfig {
        oneshim_core::config::AnalysisConfig::default()
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

    #[test]
    fn validate_analysis_accepts_valid_defaults() {
        let cfg = default_analysis();
        assert!(validate_analysis_config(&cfg).is_ok());
    }
}
