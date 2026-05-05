pub(crate) mod commands;
pub(crate) mod helpers;
pub(crate) mod queries;
pub(crate) mod scene;

pub(crate) const AUTOMATION_AUDIT_SCHEMA_VERSION: &str = "automation.audit.v1";
pub(crate) const AUTOMATION_SCENE_ACTION_SCHEMA_VERSION: &str = "automation.scene_action.v1";
pub(crate) const AUTOMATION_SCENE_CALIBRATION_SCHEMA_VERSION: &str =
    "automation.scene_calibration.v1";

pub use commands::AutomationCommandService;
pub use queries::AutomationQueryService;
pub use scene::AutomationSceneQueryService;
