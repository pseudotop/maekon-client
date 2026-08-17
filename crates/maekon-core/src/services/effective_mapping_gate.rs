//! Live-gate orchestration for local TMD workbook writes (#10358).
//!
//! This use case never reads the disk cache. A successful live response updates
//! the revalidation candidate; a live domain rejection invalidates it. Network
//! failure therefore cannot fall through to stale data and authorize a write.

use chrono::Utc;

use crate::error::CoreError;
use crate::models::effective_mapping::EffectiveMappingResolution;
use crate::ports::effective_mapping_cache::EffectiveMappingCache;
use crate::ports::effective_mapping_client::EffectiveMappingClient;

pub async fn resolve_live(
    client: &dyn EffectiveMappingClient,
    cache: &dyn EffectiveMappingCache,
    organization_id: &str,
    mapping_id: &str,
    assignment_id: &str,
) -> Result<EffectiveMappingResolution, CoreError> {
    let resolution = client
        .resolve_effective_mapping(organization_id, mapping_id, assignment_id)
        .await?;
    match &resolution {
        EffectiveMappingResolution::Effective(mapping) => {
            cache
                .store_server_validated(mapping, &Utc::now().to_rfc3339())
                .await?;
        }
        EffectiveMappingResolution::Rejected(_) => {
            cache
                .invalidate(organization_id, mapping_id, assignment_id)
                .await?;
        }
    }
    Ok(resolution)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::error_codes::NetworkCode;
    use crate::models::effective_mapping::{
        CachedEffectiveMappingCandidate, EffectiveMapping, MappingResolutionReason,
        MappingResolutionRejection,
    };
    use async_trait::async_trait;

    enum ClientResult {
        Effective,
        Rejected,
        Offline,
    }

    struct FakeClient(ClientResult);

    #[async_trait]
    impl EffectiveMappingClient for FakeClient {
        async fn resolve_effective_mapping(
            &self,
            _organization_id: &str,
            _mapping_id: &str,
            _assignment_id: &str,
        ) -> Result<EffectiveMappingResolution, CoreError> {
            match self.0 {
                ClientResult::Effective => Ok(EffectiveMappingResolution::Effective(mapping())),
                ClientResult::Rejected => Ok(EffectiveMappingResolution::Rejected(
                    MappingResolutionRejection {
                        reason_code: MappingResolutionReason::TemplateStale,
                        mapping_id: "map-1".into(),
                        assignment_id: "asg-1".into(),
                        message: "template changed".into(),
                        expected: None,
                        actual: None,
                    },
                )),
                ClientResult::Offline => Err(CoreError::Network {
                    code: NetworkCode::Generic,
                    message: "offline".into(),
                }),
            }
        }
    }

    #[derive(Default)]
    struct FakeCache {
        stored: Mutex<usize>,
        invalidated: Mutex<usize>,
        loaded: Mutex<usize>,
    }

    #[async_trait]
    impl EffectiveMappingCache for FakeCache {
        async fn store_server_validated(
            &self,
            _mapping: &EffectiveMapping,
            _server_validated_at: &str,
        ) -> Result<(), CoreError> {
            *self.stored.lock().unwrap() += 1;
            Ok(())
        }

        async fn load_candidate(
            &self,
            _organization_id: &str,
            _mapping_id: &str,
            _assignment_id: &str,
        ) -> Result<Option<CachedEffectiveMappingCandidate>, CoreError> {
            *self.loaded.lock().unwrap() += 1;
            Ok(None)
        }

        async fn invalidate(
            &self,
            _organization_id: &str,
            _mapping_id: &str,
            _assignment_id: &str,
        ) -> Result<(), CoreError> {
            *self.invalidated.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn mapping() -> EffectiveMapping {
        let content = "{\"fields\":[]}".to_string();
        EffectiveMapping {
            mapping_id: "map-1".into(),
            organization_id: "org-1".into(),
            version_id: "ver-1".into(),
            version_seq: 1,
            content_hash: EffectiveMapping::hash_content(&content),
            content,
            approval_seq: 1,
            approved_at: "2026-08-15T00:00:00Z".into(),
            approved_by_user_id: "user-1".into(),
            approved_template_hash: "b".repeat(64),
            assignment_id: "asg-1".into(),
            assignment_hash: "c".repeat(64),
            source_snapshot_hash: "d".repeat(64),
        }
    }

    #[tokio::test]
    async fn live_success_updates_candidate() {
        let cache = FakeCache::default();
        let result = resolve_live(
            &FakeClient(ClientResult::Effective),
            &cache,
            "org-1",
            "map-1",
            "asg-1",
        )
        .await
        .unwrap();
        assert!(matches!(result, EffectiveMappingResolution::Effective(_)));
        assert_eq!(*cache.stored.lock().unwrap(), 1);
        assert_eq!(*cache.invalidated.lock().unwrap(), 0);
        assert_eq!(*cache.loaded.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn live_rejection_invalidates_candidate() {
        let cache = FakeCache::default();
        let result = resolve_live(
            &FakeClient(ClientResult::Rejected),
            &cache,
            "org-1",
            "map-1",
            "asg-1",
        )
        .await
        .unwrap();
        assert!(matches!(result, EffectiveMappingResolution::Rejected(_)));
        assert_eq!(*cache.stored.lock().unwrap(), 0);
        assert_eq!(*cache.invalidated.lock().unwrap(), 1);
        assert_eq!(*cache.loaded.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn offline_failure_never_loads_or_promotes_cached_data() {
        let cache = FakeCache::default();
        let error = resolve_live(
            &FakeClient(ClientResult::Offline),
            &cache,
            "org-1",
            "map-1",
            "asg-1",
        )
        .await
        .expect_err("offline must fail closed");
        assert_eq!(error.code(), "network.generic");
        assert_eq!(*cache.stored.lock().unwrap(), 0);
        assert_eq!(*cache.invalidated.lock().unwrap(), 0);
        assert_eq!(*cache.loaded.lock().unwrap(), 0);
    }
}
