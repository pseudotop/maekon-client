use oneshim_api_contracts::provider_specs::ProviderSurfaceCatalog;

use crate::error::ApiError;
use crate::services::ai_provider_spec_service;

#[derive(Clone, Default)]
pub struct AiProviderSpecQueryService;

impl AiProviderSpecQueryService {
    pub fn new() -> Self {
        Self
    }

    pub fn list_provider_surfaces(&self) -> Result<ProviderSurfaceCatalog, ApiError> {
        ai_provider_spec_service::list_provider_surface_specs()
    }
}
