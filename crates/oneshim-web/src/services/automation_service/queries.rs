use chrono::Utc;

use oneshim_api_contracts::automation::{
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
            scene_schema_version: oneshim_core::models::ui_scene::UI_SCENE_SCHEMA_VERSION
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
                sandbox_profile: format!("{:?}", config.automation.sandbox.profile),
                ocr_provider: format!("{:?}", config.ai_provider.ocr_provider),
                llm_provider: format!("{:?}", config.ai_provider.llm_provider),
                ocr_source: runtime_status.ocr_source,
                llm_source: runtime_status.llm_source,
                ocr_fallback_reason: runtime_status.ocr_fallback_reason,
                llm_fallback_reason: runtime_status.llm_fallback_reason,
                external_data_policy: format!("{:?}", config.ai_provider.external_data_policy),
                pending_audit_entries: pending,
            })
        } else {
            Ok(default_automation_status(pending))
        }
    }

    pub async fn audit_logs(
        &self,
        query: AuditQuery,
    ) -> Result<Vec<oneshim_api_contracts::automation::AuditEntryDto>, ApiError> {
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
    ) -> Result<Vec<oneshim_api_contracts::automation::AuditEntryDto>, ApiError> {
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
                sandbox_profile: format!("{:?}", config.automation.sandbox.profile),
                sandbox_enabled: config.automation.sandbox.enabled,
                allow_network: config.automation.sandbox.allow_network,
                external_data_policy: format!("{:?}", config.ai_provider.external_data_policy),
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
        let mut presets = oneshim_core::models::intent::builtin_presets();
        if let Some(ref config_manager) = self.ctx.config_manager {
            let config = config_manager.get();
            presets.extend(config.automation.custom_presets.clone());
        }
        PresetListDto { presets }
    }
}
