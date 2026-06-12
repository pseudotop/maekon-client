use maekon_api_contracts::settings::AppSettings;

use crate::error::ApiError;
use crate::services::settings_update_flow::SettingsUpdateFlow;
use crate::services::settings_validation::validate_settings_input;
use crate::services::web_contexts::SettingsWebContext;

/// F-RR-C22-01: `SettingsUpdateFlow` is promoted to a field so the
/// `PolicyAuditWriter` background task (and its bounded mpsc channel) is
/// created once per service instance, not once per `update_settings` call.
///
/// The previous shape called `SettingsUpdateFlow::new(...)` inside
/// `update_settings()`, which called `PolicyAuditWriter::new` (spawning a
/// task) and then immediately dropped `SettingsUpdateFlow` at the end of the
/// request (aborting the task via `Drop`).  Rapid or concurrent settings saves
/// accumulated N concurrent spawn/abort cycles with no upper bound — exactly
/// the pattern that F-RR-38 was meant to prevent inside `SettingsUpdateFlow`
/// itself, but which was left unaddressed at the caller layer.
///
/// With the field-based design:
/// - `SettingsCommandService::new` creates one `SettingsUpdateFlow` (and one
///   `PolicyAuditWriter` task) per Axum state clone.
/// - `update_settings` reuses the long-lived writer via `flow.apply(settings)`.
/// - `SettingsCommandService` derives `Clone` because `SettingsUpdateFlow`
///   derives `Clone`; `PolicyAuditWriter` is behind `Arc` inside
///   `SettingsUpdateFlow`, so all clones share one writer task.
#[derive(Clone)]
pub struct SettingsCommandService {
    /// Long-lived update flow.  `None` when no config manager is configured.
    /// Initialized once in `new`; reused by every `update_settings` call.
    flow: Option<SettingsUpdateFlow>,
}

impl SettingsCommandService {
    pub fn new(ctx: SettingsWebContext) -> Self {
        let flow = ctx.config_manager.as_ref().map(|config_manager| {
            SettingsUpdateFlow::new(
                config_manager.clone(),
                ctx.default_secret_backend_kind,
                ctx.secret_store.clone(),
                ctx.secret_stores.clone(),
                ctx.audit_logger.clone(),
                // #5707: coaching 엔진 핸들을 flow 에 전달해 hot-reload 를 활성화한다.
                ctx.coaching_engine.clone(),
            )
        });
        Self { flow }
    }

    pub async fn update_settings(&self, settings: &AppSettings) -> Result<(), ApiError> {
        validate_settings_input(settings)?;

        if let Some(ref flow) = self.flow {
            flow.apply(settings).await?;
        }

        Ok(())
    }
}
