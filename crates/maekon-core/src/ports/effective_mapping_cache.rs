//! Non-authoritative local cache port for effective TMD mappings (#10358).

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::effective_mapping::{CachedEffectiveMappingCandidate, EffectiveMapping};

#[async_trait]
pub trait EffectiveMappingCache: Send + Sync {
    async fn store_server_validated(
        &self,
        mapping: &EffectiveMapping,
        server_validated_at: &str,
    ) -> Result<(), CoreError>;

    /// Load a candidate for UI/offline explanation or a future live
    /// revalidation. Returning this value never authorizes a workbook write.
    async fn load_candidate(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<Option<CachedEffectiveMappingCandidate>, CoreError>;

    async fn invalidate(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<(), CoreError>;
}
