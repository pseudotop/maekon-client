use oneshim_api_contracts::sessions::SessionResponse;

use crate::error::ApiError;
use crate::services::sessions_assembler::assemble_session_response;
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct SessionsQueryService {
    ctx: StorageWebContext,
}

impl SessionsQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub fn list_sessions(&self) -> Result<Vec<SessionResponse>, ApiError> {
        self.ctx
            .storage
            .list_session_stats(50)
            .map_err(ApiError::from)
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(assemble_session_response)
                    .collect()
            })
    }

    pub async fn get_session(&self, session_id: &str) -> Result<SessionResponse, ApiError> {
        let session = self
            .ctx
            .storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("session '{session_id}'")))?;

        Ok(assemble_session_response(session))
    }
}
