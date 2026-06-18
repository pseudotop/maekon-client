use chrono::Utc;

use maekon_api_contracts::automation::{
    AuditQuery, AutomationContractsDto, AutomationStatsDto, AutomationStatusDto, PoliciesDto,
    PolicyEventQuery, PresetListDto,
};

use crate::error::ApiError;
use crate::services::automation_assembler::map_audit_entry;
use crate::services::web_contexts::AutomationWebContext;

use super::helpers::{
    default_automation_status, default_policies, evaluate_scene_action_override,
    parse_audit_status, resolve_ai_runtime_status,
};
use super::{AUTOMATION_AUDIT_SCHEMA_VERSION, AUTOMATION_SCENE_ACTION_SCHEMA_VERSION};

#[derive(Clone)]
pub struct AutomationQueryService {
    ctx: AutomationWebContext,
}

impl AutomationQueryService {
    pub fn new(ctx: AutomationWebContext) -> Self {
        Self { ctx }
    }

    pub fn contract_versions() -> AutomationContractsDto {
        AutomationContractsDto {
            audit_schema_version: AUTOMATION_AUDIT_SCHEMA_VERSION.to_string(),
            scene_schema_version: maekon_core::models::ui_scene::UI_SCENE_SCHEMA_VERSION
                .to_string(),
            scene_action_schema_version: AUTOMATION_SCENE_ACTION_SCHEMA_VERSION.to_string(),
        }
    }

    pub async fn automation_status(&self) -> Result<AutomationStatusDto, ApiError> {
        let pending = if let Some(ref logger) = self.ctx.audit_logger {
            logger.pending_count().await
        } else {
            0
        };

        // #5734: read the live per-call LLM health at request time, outside
        // the config_manager branch, so the value is always visible regardless
        // of whether a config manager is wired (covers both production and
        // minimal test/CLI contexts).  `None` when the handle is not wired.
        let llm_healthy = self
            .ctx
            .llm_call_health
            .as_ref()
            .and_then(|h| h.as_option_bool());

        if let Some(ref config_manager) = self.ctx.config_manager {
            let config = config_manager.get();
            let runtime_status = resolve_ai_runtime_status(
                &self.ctx,
                config.ai_provider.access_mode,
                config.ai_provider.ocr_provider,
                config.ai_provider.llm_provider,
            );
            Ok(AutomationStatusDto {
                enabled: config.automation.enabled,
                sandbox_enabled: config.automation.sandbox.enabled,
                sandbox_profile: config.automation.sandbox.profile.to_string(),
                ocr_provider: config.ai_provider.ocr_provider.to_string(),
                llm_provider: config.ai_provider.llm_provider.to_string(),
                ocr_source: runtime_status.ocr_source,
                llm_source: runtime_status.llm_source,
                ocr_fallback_reason: runtime_status.ocr_fallback_reason,
                llm_fallback_reason: runtime_status.llm_fallback_reason,
                external_data_policy: config.ai_provider.external_data_policy.to_string(),
                pending_audit_entries: pending,
                llm_healthy,
                // ConfirmationRequirement's Display impl returns a SCREAMING_SNAKE_CASE token
                // (e.g. "AUTO" / "CONFIRM" / "BLOCK") — used for frontend caption branching.
                confirmation_policy: config.automation.confirmation_policy.to_string(),
            })
        } else {
            let mut dto = default_automation_status(pending);
            dto.llm_healthy = llm_healthy;
            Ok(dto)
        }
    }

    pub async fn audit_logs(
        &self,
        query: AuditQuery,
    ) -> Result<Vec<maekon_api_contracts::automation::AuditEntryDto>, ApiError> {
        let Some(ref logger) = self.ctx.audit_logger else {
            return Ok(Vec::new());
        };

        let entries = if let Some(ref status_filter) = query.status {
            let status = parse_audit_status(status_filter)?;
            logger.entries_by_status(&status, query.limit).await
        } else {
            logger.recent_entries(query.limit).await
        };

        Ok(entries.into_iter().map(map_audit_entry).collect())
    }

    pub async fn policy_events(
        &self,
        query: PolicyEventQuery,
    ) -> Result<Vec<maekon_api_contracts::automation::AuditEntryDto>, ApiError> {
        let Some(ref logger) = self.ctx.audit_logger else {
            return Ok(Vec::new());
        };

        let limit = query.limit.clamp(1, 500);
        Ok(logger
            .entries_by_action_prefix("policy.", limit)
            .await
            .into_iter()
            .map(map_audit_entry)
            .collect())
    }

    pub fn policies(&self) -> PoliciesDto {
        if let Some(ref config_manager) = self.ctx.config_manager {
            let config = config_manager.get();
            let (override_active, override_issue) = evaluate_scene_action_override(
                &config.ai_provider.scene_action_override,
                Utc::now(),
            );
            PoliciesDto {
                automation_enabled: config.automation.enabled,
                sandbox_profile: config.automation.sandbox.profile.to_string(),
                sandbox_enabled: config.automation.sandbox.enabled,
                allow_network: config.automation.sandbox.allow_network,
                external_data_policy: config.ai_provider.external_data_policy.to_string(),
                scene_action_override_enabled: config.ai_provider.scene_action_override.enabled,
                scene_action_override_active: override_active,
                scene_action_override_reason: config
                    .ai_provider
                    .scene_action_override
                    .reason
                    .clone(),
                scene_action_override_approved_by: config
                    .ai_provider
                    .scene_action_override
                    .approved_by
                    .clone(),
                scene_action_override_expires_at: config
                    .ai_provider
                    .scene_action_override
                    .expires_at
                    .map(|v| v.to_rfc3339()),
                scene_action_override_issue: override_issue,
            }
        } else {
            default_policies()
        }
    }

    pub async fn automation_stats(&self) -> AutomationStatsDto {
        let Some(ref logger) = self.ctx.audit_logger else {
            return AutomationStatsDto {
                total_executions: 0,
                successful: 0,
                failed: 0,
                denied: 0,
                timeout: 0,
                avg_elapsed_ms: 0.0,
                success_rate: 0.0,
                blocked_rate: 0.0,
                p95_elapsed_ms: 0.0,
                timing_samples: 0,
            };
        };

        let stats = logger.stats().await;
        let all_entries = logger.recent_entries(1000).await;
        let elapsed_values: Vec<u64> = all_entries
            .iter()
            .filter_map(|e| e.execution_time_ms)
            .collect();
        let avg_elapsed = if elapsed_values.is_empty() {
            0.0
        } else {
            elapsed_values.iter().sum::<u64>() as f64 / elapsed_values.len() as f64
        };
        let p95_elapsed_ms = if elapsed_values.is_empty() {
            0.0
        } else {
            let mut sorted = elapsed_values.clone();
            sorted.sort_unstable();
            let idx = ((sorted.len() as f64) * 0.95).ceil() as usize;
            sorted[idx.saturating_sub(1).min(sorted.len() - 1)] as f64
        };
        let total_f64 = stats.total as f64;
        let success_rate = if stats.total > 0 {
            stats.completed as f64 / total_f64
        } else {
            0.0
        };
        let blocked_rate = if stats.total > 0 {
            stats.denied as f64 / total_f64
        } else {
            0.0
        };

        AutomationStatsDto {
            total_executions: stats.total,
            successful: stats.completed,
            failed: stats.failed,
            denied: stats.denied,
            timeout: stats.timeout,
            avg_elapsed_ms: avg_elapsed,
            success_rate,
            blocked_rate,
            p95_elapsed_ms,
            timing_samples: elapsed_values.len(),
        }
    }

    pub fn list_presets(&self) -> PresetListDto {
        let mut presets = maekon_core::models::intent::builtin_presets();
        if let Some(ref config_manager) = self.ctx.config_manager {
            let config = config_manager.get();
            presets.extend(config.automation.custom_presets.clone());
        }
        PresetListDto { presets }
    }
}

// ── #5734 status assembly tests ───────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_bindings::LlmCallHealth;
    use crate::services::web_contexts::AutomationWebContext;
    use crate::storage_port::WebStorage;
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;

    fn test_ctx(llm_call_health: Option<Arc<LlmCallHealth>>) -> AutomationWebContext {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"))
            as Arc<dyn WebStorage>;
        AutomationWebContext {
            storage,
            frames_dir: None,
            config_manager: None,
            audit_logger: None,
            automation_controller: None,
            ai_runtime_status: None,
            llm_call_health,
        }
    }

    /// `automation_status()` returns `confirmation_policy = "AUTO"` when no config manager
    /// is wired (fast-path fallback via `default_automation_status`).
    #[tokio::test]
    async fn automation_status_confirmation_policy_defaults_to_auto_when_no_config() {
        let svc = AutomationQueryService::new(test_ctx(None));
        let status = svc.automation_status().await.expect("should succeed");
        assert_eq!(
            status.confirmation_policy, "AUTO",
            "no config manager → confirmation_policy must default to AUTO"
        );
    }

    /// When no health handle is wired, `automation_status()` returns `llm_healthy: None`.
    #[tokio::test]
    async fn automation_status_llm_healthy_none_when_handle_absent() {
        let svc = AutomationQueryService::new(test_ctx(None));
        let status = svc.automation_status().await.expect("should succeed");
        assert_eq!(
            status.llm_healthy, None,
            "no handle wired → llm_healthy must be None"
        );
    }

    /// When a handle is wired with UNKNOWN state, `llm_healthy` is `None`.
    #[tokio::test]
    async fn automation_status_llm_healthy_none_when_unknown() {
        let handle = Arc::new(LlmCallHealth::default()); // UNKNOWN
        let svc = AutomationQueryService::new(test_ctx(Some(handle)));
        let status = svc.automation_status().await.expect("should succeed");
        assert_eq!(
            status.llm_healthy, None,
            "UNKNOWN handle → llm_healthy must be None"
        );
    }

    /// After `record_ok()`, `automation_status()` returns `llm_healthy: Some(true)`.
    #[tokio::test]
    async fn automation_status_llm_healthy_some_true_after_record_ok() {
        let handle = Arc::new(LlmCallHealth::default());
        handle.record_ok();
        let svc = AutomationQueryService::new(test_ctx(Some(handle)));
        let status = svc.automation_status().await.expect("should succeed");
        assert_eq!(
            status.llm_healthy,
            Some(true),
            "record_ok() → llm_healthy must be Some(true)"
        );
    }

    /// After `record_failed()`, `automation_status()` returns `llm_healthy: Some(false)`.
    #[tokio::test]
    async fn automation_status_llm_healthy_some_false_after_record_failed() {
        let handle = Arc::new(LlmCallHealth::default());
        handle.record_failed();
        let svc = AutomationQueryService::new(test_ctx(Some(handle)));
        let status = svc.automation_status().await.expect("should succeed");
        assert_eq!(
            status.llm_healthy,
            Some(false),
            "record_failed() → llm_healthy must be Some(false)"
        );
    }

    /// Live state is read at request time: changing the handle after service
    /// construction is still reflected in the response.
    #[tokio::test]
    async fn automation_status_llm_healthy_reflects_live_state() {
        let handle = Arc::new(LlmCallHealth::default());
        let svc = AutomationQueryService::new(test_ctx(Some(Arc::clone(&handle))));

        // Before any call: None.
        let s1 = svc.automation_status().await.expect("should succeed");
        assert_eq!(s1.llm_healthy, None, "before call → None");

        // First OK call.
        handle.record_ok();
        let s2 = svc.automation_status().await.expect("should succeed");
        assert_eq!(s2.llm_healthy, Some(true), "after ok → Some(true)");

        // Subsequent failure overwrites.
        handle.record_failed();
        let s3 = svc.automation_status().await.expect("should succeed");
        assert_eq!(s3.llm_healthy, Some(false), "after fail → Some(false)");
    }
}
