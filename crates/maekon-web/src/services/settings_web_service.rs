use maekon_api_contracts::settings::AppSettings;

use crate::error::ApiError;
use crate::services::settings_update_flow::SettingsUpdateFlow;
use crate::services::settings_validation::validate_settings_input;
use crate::services::web_contexts::SettingsWebContext;

/// #6117: `SettingsCommandService` is still rebuilt per `POST /api/settings`
/// (it is constructed from the per-request `SettingsWebContext`), but it no
/// longer builds — and therefore no longer aborts — its own
/// `PolicyAuditWriter`.  Instead it threads the SHARED, server-lifetime
/// `Arc<PolicyAuditWriter>` (built once in `WebServer::build_router`, carried on
/// `SettingsWebContext`) into the `SettingsUpdateFlow`.
///
/// The previous shape (F-RR-C22-01/F-RR-38) created a fresh writer inside
/// `SettingsUpdateFlow::new` on every request.  Because the whole context is
/// dropped at request end, that writer's bounded-channel drain task was
/// `abort()`ed before the just-enqueued audit event could flush — so the
/// security-policy audit event was deterministically lost on every save.  The
/// shared writer's drain task lives for the whole server lifetime, and the save
/// path now AWAITS the enqueue, so the event is durably handed off before the
/// HTTP response returns.
#[derive(Clone)]
pub struct SettingsCommandService {
    /// Per-request update flow.  `None` when no config manager is configured.
    /// Holds an `Arc` to the shared writer — no writer task is spawned here.
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
                // #6117: share the single, server-lifetime writer — do NOT
                // construct a new one per request.
                ctx.policy_audit_writer.clone(),
                // #5707: pass the coaching engine handle to the flow to enable
                // hot-reload.
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
