//! Live server-gate port for effective TMD mappings (#10358).

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::effective_mapping::EffectiveMappingResolution;

#[async_trait]
pub trait EffectiveMappingClient: Send + Sync {
    /// Resolve the mapping against the current approval, template, and
    /// assignment anchors. A cached candidate is never accepted here.
    async fn resolve_effective_mapping(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<EffectiveMappingResolution, CoreError>;
}
