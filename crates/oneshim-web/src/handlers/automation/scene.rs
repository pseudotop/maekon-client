use axum::{
    extract::{Query, State},
    Json,
};

use oneshim_api_contracts::automation::{SceneCalibrationDto, SceneCalibrationQuery, SceneQuery};
use oneshim_core::models::ui_scene::UiScene;

use crate::error::ApiError;
use crate::services::automation_service::AutomationSceneQueryService;
use crate::services::web_contexts::AutomationWebContext;

pub async fn get_automation_scene(
    State(context): State<AutomationWebContext>,
    Query(query): Query<SceneQuery>,
) -> Result<Json<UiScene>, ApiError> {
    Ok(Json(
        AutomationSceneQueryService::new(context)
            .get_scene(query)
            .await?,
    ))
}

pub async fn get_automation_scene_calibration(
    State(context): State<AutomationWebContext>,
    Query(query): Query<SceneCalibrationQuery>,
) -> Result<Json<SceneCalibrationDto>, ApiError> {
    Ok(Json(
        AutomationSceneQueryService::new(context)
            .get_scene_calibration(query)
            .await?,
    ))
}
