//! SQLite adapter for non-authoritative effective mapping candidates (#10358).

use async_trait::async_trait;
use chrono::DateTime;
use maekon_core::error::CoreError;
use maekon_core::models::effective_mapping::{CachedEffectiveMappingCandidate, EffectiveMapping};
use maekon_core::ports::effective_mapping_cache::EffectiveMappingCache;
use rusqlite::{params, OptionalExtension, Row};

use crate::error::StorageError;

use super::SqliteStorage;

#[derive(Debug)]
struct CacheRow {
    organization_id: String,
    mapping_id: String,
    assignment_id: String,
    version_id: String,
    version_seq: i64,
    content_hash: String,
    content: String,
    approval_seq: i64,
    approved_at: String,
    approved_by_user_id: String,
    approved_template_hash: String,
    assignment_hash: String,
    source_snapshot_hash: String,
    server_validated_at: String,
}

impl CacheRow {
    fn from_sql(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            organization_id: row.get(0)?,
            mapping_id: row.get(1)?,
            assignment_id: row.get(2)?,
            version_id: row.get(3)?,
            version_seq: row.get(4)?,
            content_hash: row.get(5)?,
            content: row.get(6)?,
            approval_seq: row.get(7)?,
            approved_at: row.get(8)?,
            approved_by_user_id: row.get(9)?,
            approved_template_hash: row.get(10)?,
            assignment_hash: row.get(11)?,
            source_snapshot_hash: row.get(12)?,
            server_validated_at: row.get(13)?,
        })
    }

    fn into_candidate(self) -> Result<CachedEffectiveMappingCandidate, StorageError> {
        let mapping = EffectiveMapping {
            mapping_id: self.mapping_id,
            organization_id: self.organization_id,
            version_id: self.version_id,
            version_seq: self.version_seq,
            content_hash: self.content_hash,
            content: self.content,
            approval_seq: self.approval_seq,
            approved_at: self.approved_at,
            approved_by_user_id: self.approved_by_user_id,
            approved_template_hash: self.approved_template_hash,
            assignment_id: self.assignment_id,
            assignment_hash: self.assignment_hash,
            source_snapshot_hash: self.source_snapshot_hash,
        };
        validate_cache_value(&mapping, &self.server_validated_at)?;
        Ok(CachedEffectiveMappingCandidate {
            mapping,
            server_validated_at: self.server_validated_at,
        })
    }
}

#[async_trait]
impl EffectiveMappingCache for SqliteStorage {
    async fn store_server_validated(
        &self,
        mapping: &EffectiveMapping,
        server_validated_at: &str,
    ) -> Result<(), CoreError> {
        validate_cache_value(mapping, server_validated_at)?;
        let mapping = mapping.clone();
        let server_validated_at = server_validated_at.to_owned();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO effective_mapping_cache (
                    organization_id, mapping_id, assignment_id, version_id,
                    version_seq, content_hash, content, approval_seq,
                    approved_at, approved_by_user_id, approved_template_hash,
                    assignment_hash, source_snapshot_hash, server_validated_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
                 ) ON CONFLICT(organization_id, mapping_id, assignment_id) DO UPDATE SET
                    version_id = excluded.version_id,
                    version_seq = excluded.version_seq,
                    content_hash = excluded.content_hash,
                    content = excluded.content,
                    approval_seq = excluded.approval_seq,
                    approved_at = excluded.approved_at,
                    approved_by_user_id = excluded.approved_by_user_id,
                    approved_template_hash = excluded.approved_template_hash,
                    assignment_hash = excluded.assignment_hash,
                    source_snapshot_hash = excluded.source_snapshot_hash,
                    server_validated_at = excluded.server_validated_at",
                params![
                    mapping.organization_id,
                    mapping.mapping_id,
                    mapping.assignment_id,
                    mapping.version_id,
                    mapping.version_seq,
                    mapping.content_hash,
                    mapping.content,
                    mapping.approval_seq,
                    mapping.approved_at,
                    mapping.approved_by_user_id,
                    mapping.approved_template_hash,
                    mapping.assignment_hash,
                    mapping.source_snapshot_hash,
                    server_validated_at,
                ],
            )
            .map_err(|error| {
                StorageError::Internal(format!("store effective mapping candidate: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn load_candidate(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<Option<CachedEffectiveMappingCandidate>, CoreError> {
        let organization_id = organization_id.to_owned();
        let mapping_id = mapping_id.to_owned();
        let assignment_id = assignment_id.to_owned();
        self.with_conn_read(move |conn| {
            conn.query_row(
                "SELECT organization_id, mapping_id, assignment_id, version_id,
                        version_seq, content_hash, content, approval_seq,
                        approved_at, approved_by_user_id, approved_template_hash,
                        assignment_hash, source_snapshot_hash, server_validated_at
                 FROM effective_mapping_cache
                 WHERE organization_id = ?1 AND mapping_id = ?2 AND assignment_id = ?3",
                params![organization_id, mapping_id, assignment_id],
                CacheRow::from_sql,
            )
            .optional()
            .map_err(|error| {
                StorageError::Internal(format!("load effective mapping candidate: {error}"))
            })?
            .map(CacheRow::into_candidate)
            .transpose()
        })
        .await
        .map_err(Into::into)
    }

    async fn invalidate(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<(), CoreError> {
        let organization_id = organization_id.to_owned();
        let mapping_id = mapping_id.to_owned();
        let assignment_id = assignment_id.to_owned();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM effective_mapping_cache
                 WHERE organization_id = ?1 AND mapping_id = ?2 AND assignment_id = ?3",
                params![organization_id, mapping_id, assignment_id],
            )
            .map_err(|error| {
                StorageError::Internal(format!("invalidate effective mapping candidate: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

fn validate_cache_value(
    mapping: &EffectiveMapping,
    server_validated_at: &str,
) -> Result<(), StorageError> {
    if !mapping.content_hash_matches() {
        return Err(StorageError::Validation {
            field: "content_hash".into(),
            message: "cached effective mapping content hash mismatch".into(),
        });
    }
    for (field, value) in [
        ("mapping_id", &mapping.mapping_id),
        ("organization_id", &mapping.organization_id),
        ("assignment_id", &mapping.assignment_id),
        ("version_id", &mapping.version_id),
    ] {
        if value.is_empty() {
            return Err(StorageError::Validation {
                field: field.into(),
                message: "cached effective mapping identifier is empty".into(),
            });
        }
    }
    DateTime::parse_from_rfc3339(&mapping.approved_at).map_err(|error| {
        StorageError::Validation {
            field: "approved_at".into(),
            message: format!("cached effective mapping timestamp is invalid: {error}"),
        }
    })?;
    DateTime::parse_from_rfc3339(server_validated_at).map_err(|error| {
        StorageError::Validation {
            field: "server_validated_at".into(),
            message: format!("cache validation timestamp is invalid: {error}"),
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mapping() -> EffectiveMapping {
        let content = "{\"fields\":[]}".to_string();
        EffectiveMapping {
            mapping_id: "map-1".into(),
            organization_id: "org-1".into(),
            version_id: "ver-1".into(),
            version_seq: 3,
            content_hash: EffectiveMapping::hash_content(&content),
            content,
            approval_seq: 2,
            approved_at: "2026-08-15T00:00:00Z".into(),
            approved_by_user_id: "user-1".into(),
            approved_template_hash: "b".repeat(64),
            assignment_id: "asg-1".into(),
            assignment_hash: "c".repeat(64),
            source_snapshot_hash: "d".repeat(64),
        }
    }

    #[tokio::test]
    async fn round_trips_then_invalidates_candidate_without_granting_authority() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .store_server_validated(&mapping(), "2026-08-15T01:00:00Z")
            .await
            .unwrap();
        let candidate = storage
            .load_candidate("org-1", "map-1", "asg-1")
            .await
            .unwrap()
            .expect("candidate must exist");
        assert_eq!(candidate.mapping.version_seq, 3);
        assert_eq!(candidate.server_validated_at, "2026-08-15T01:00:00Z");

        storage.invalidate("org-1", "map-1", "asg-1").await.unwrap();
        assert!(storage
            .load_candidate("org-1", "map-1", "asg-1")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn mutation_control_detects_tampered_cached_content() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .store_server_validated(&mapping(), "2026-08-15T01:00:00Z")
            .await
            .unwrap();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "UPDATE effective_mapping_cache SET content = '{\"tampered\":true}'",
                    [],
                )
                .unwrap();
        }

        let error = storage
            .load_candidate("org-1", "map-1", "asg-1")
            .await
            .expect_err("tampered cache must fail loudly");
        assert_eq!(error.code(), "validation.invalid_field");
    }

    #[tokio::test]
    async fn candidate_does_not_survive_full_local_erasure() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .store_server_validated(&mapping(), "2026-08-15T01:00:00Z")
            .await
            .unwrap();

        storage.delete_all_data().unwrap();

        assert!(storage
            .load_candidate("org-1", "map-1", "asg-1")
            .await
            .unwrap()
            .is_none());
    }
}
