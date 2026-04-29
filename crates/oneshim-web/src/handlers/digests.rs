use axum::extract::{Query, State};
use axum::Json;
use oneshim_api_contracts::digests::DigestListQuery;
use tracing::debug;

use oneshim_core::models::weekly_digest::WeeklyDigest;

use crate::error::ApiError;
use crate::services::web_contexts::StorageWebContext;

/// GET /api/digests?limit=4 — list recent weekly digests.
pub async fn list_digests(
    State(context): State<StorageWebContext>,
    Query(params): Query<DigestListQuery>,
) -> Result<Json<Vec<WeeklyDigest>>, ApiError> {
    let limit = params.limit.unwrap_or(4).min(52);
    debug!("GET /api/digests limit={}", limit);

    let digests = context
        .storage
        .list_weekly_digests(limit)
        .map_err(ApiError::from)?;

    Ok(Json(digests))
}

/// GET /api/digests/current — get the current (partial) week digest.
pub async fn current_digest(
    State(context): State<StorageWebContext>,
) -> Result<Json<Option<WeeklyDigest>>, ApiError> {
    debug!("GET /api/digests/current");

    let digest = context
        .storage
        .get_current_week_digest()
        .map_err(ApiError::from)?;

    Ok(Json(digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_list_query_defaults() {
        let json = r#"{}"#;
        let query: DigestListQuery = serde_json::from_str(json).unwrap();
        assert!(query.limit.is_none());
    }

    #[test]
    fn digest_list_query_with_limit() {
        let json = r#"{"limit": 8}"#;
        let query: DigestListQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, Some(8));
    }
}
