//! SQLite adapter for the Skill Pack catalog + activation ports (#8588).
//!
//! Mirrors `extension_registry_impl`: the core model owns every decision, this
//! file only executes an already-decided transition inside a transaction.
//!
//! Two things here are load-bearing rather than incidental:
//!
//! * The catalog stores `body_sha256`, never the body. Re-hashing the presented
//!   body at activation is what makes an on-disk swap detectable.
//! * `remove_for_install` deletes children before parents explicitly. The
//!   `REFERENCES ... ON DELETE CASCADE` clauses in v52 DO fire — this workspace
//!   runs with `foreign_keys` ON (#9735 CORRECTED — the old claim of OFF was
//!   wrong, and an earlier pass at this sentence left the causality backwards).
//!   The explicit deletes are kept for order control, redundant with the engine
//!   rather than the only thing preventing orphans.
//
// OOS-TBD: ADR-013 file split — command port, query port, and row helpers can
// move into a `skill_pack_registry_impl/` directory module if this grows.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeMap;

use maekon_core::error::CoreError;
use maekon_core::models::extension::ContributionKind;
use maekon_core::models::skill_pack::{
    SkillActivationAudit, SkillPackEntry, SkillSelectionKind, StoredActivation,
};
use maekon_core::ports::skill_pack_registry::{
    RecordActivationRequest, RegisterSkillPackRequest, SkillPackCatalogCommandPort,
    SkillPackCatalogQueryPort, SkillPackOutcome,
};

use crate::error::StorageError;
use crate::sqlite::SqliteStorage;

const CATALOG_COLUMNS: &str = "skill_id, install_id, extension_id, contribution_id,
     contribution_kind, version, publisher_id, body_sha256, required_capabilities,
     optional_capabilities, skill_references";

/// `with_conn_mut` returns `T::default()` when a GDPR erasure is in flight. The
/// command methods use `Option<SkillPackOutcome>` so that skip becomes an error
/// rather than a silent false success.
fn skipped_err() -> CoreError {
    StorageError::Internal("skill pack mutation skipped during erasure".to_string()).into()
}

fn parse_ts(value: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StorageError::Internal(format!("skill pack timestamp decode failed: {e}")))
}

fn encode_list(values: &[String]) -> Result<String, StorageError> {
    serde_json::to_string(values)
        .map_err(|e| StorageError::Internal(format!("skill pack list encode failed: {e}")))
}

fn decode_list(raw: &str) -> Result<Vec<String>, StorageError> {
    serde_json::from_str(raw)
        .map_err(|e| StorageError::Internal(format!("skill pack list decode failed: {e}")))
}

/// Row mapper returning a nested result so a decode failure surfaces as a
/// `StorageError` rather than a rusqlite type error.
#[allow(clippy::type_complexity)]
fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<SkillPackEntry, StorageError>> {
    let skill_id: String = r.get(0)?;
    let install_id: String = r.get(1)?;
    let extension_id: String = r.get(2)?;
    let contribution_id: String = r.get(3)?;
    let contribution_kind: String = r.get(4)?;
    let version: String = r.get(5)?;
    let publisher_id: String = r.get(6)?;
    let body_sha256: String = r.get(7)?;
    let required: String = r.get(8)?;
    let optional: String = r.get(9)?;
    let references: String = r.get(10)?;
    Ok((|| {
        Ok(SkillPackEntry {
            skill_id,
            install_id,
            extension_id,
            contribution_id,
            contribution_kind: ContributionKind::from_sql_str(&contribution_kind),
            version,
            publisher_id,
            body_sha256,
            required_capabilities: decode_list(&required)?,
            optional_capabilities: decode_list(&optional)?,
            references: decode_list(&references)?,
        })
    })())
}

#[allow(clippy::type_complexity)]
fn row_to_activation(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<StoredActivation, StorageError>> {
    let activation_id: String = r.get(0)?;
    let skill_id: String = r.get(1)?;
    let version: String = r.get(2)?;
    let body_sha256: String = r.get(3)?;
    let selection_kind: String = r.get(4)?;
    let binding_id: Option<String> = r.get(5)?;
    let activated_at: String = r.get(6)?;
    let expires_at: String = r.get(7)?;
    Ok((|| {
        let selection = match selection_kind.as_str() {
            "EXPLICIT_USER_SELECTION" => SkillSelectionKind::ExplicitUserSelection,
            "APPROVED_WORKFLOW_BINDING" => SkillSelectionKind::ApprovedWorkflowBinding {
                // A binding row without its id is corrupt, not a default.
                binding_id: binding_id.ok_or_else(|| {
                    StorageError::Internal(
                        "workflow-binding activation is missing its binding_id".to_string(),
                    )
                })?,
            },
            other => {
                return Err(StorageError::Internal(format!(
                    "unknown skill pack selection kind {other}"
                )))
            }
        };
        Ok(StoredActivation {
            activation_id,
            skill_id,
            version,
            body_sha256,
            selection,
            activated_at: parse_ts(&activated_at)?,
            expires_at: parse_ts(&expires_at)?,
        })
    })())
}

fn catalog_exists(tx: &Transaction<'_>, skill_id: &str) -> Result<bool, StorageError> {
    let n: i64 = tx.query_row(
        "SELECT COUNT(*) FROM skill_pack_catalog WHERE skill_id = ?1",
        [skill_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

#[async_trait]
impl SkillPackCatalogCommandPort for SqliteStorage {
    async fn register_skill_pack(
        &self,
        request: RegisterSkillPackRequest,
    ) -> Result<SkillPackOutcome, CoreError> {
        let result: Option<SkillPackOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let e = &request.entry;
                let required = encode_list(&e.required_capabilities)?;
                let optional = encode_list(&e.optional_capabilities)?;
                let references = encode_list(&e.references)?;
                tx.execute(
                    "INSERT INTO skill_pack_catalog
                     (skill_id, install_id, extension_id, contribution_id, contribution_kind,
                      version, publisher_id, body_sha256, required_capabilities,
                      optional_capabilities, skill_references, registered_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                     ON CONFLICT(skill_id) DO UPDATE SET
                        install_id=excluded.install_id,
                        extension_id=excluded.extension_id,
                        contribution_id=excluded.contribution_id,
                        contribution_kind=excluded.contribution_kind,
                        version=excluded.version,
                        publisher_id=excluded.publisher_id,
                        body_sha256=excluded.body_sha256,
                        required_capabilities=excluded.required_capabilities,
                        optional_capabilities=excluded.optional_capabilities,
                        skill_references=excluded.skill_references,
                        registered_at=excluded.registered_at",
                    params![
                        e.skill_id,
                        e.install_id,
                        e.extension_id,
                        e.contribution_id,
                        e.contribution_kind.as_sql_str(),
                        e.version,
                        e.publisher_id,
                        e.body_sha256,
                        required,
                        optional,
                        references,
                        request.now.to_rfc3339(),
                    ],
                )?;
                tx.commit()?;
                Ok(Some(SkillPackOutcome::Applied {
                    id: request.entry.skill_id.clone(),
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn record_activation(
        &self,
        request: RecordActivationRequest,
    ) -> Result<SkillPackOutcome, CoreError> {
        let result: Option<SkillPackOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                // An activation must name a skill that is actually in the
                // catalog; otherwise a stale UI selection could pin a body that
                // was never registered from a verified package.
                if !catalog_exists(&tx, &request.skill_id)? {
                    tx.commit()?;
                    return Ok(Some(SkillPackOutcome::NotFound));
                }
                let (kind, binding) = match &request.selection {
                    SkillSelectionKind::ExplicitUserSelection => {
                        ("EXPLICIT_USER_SELECTION", None::<String>)
                    }
                    SkillSelectionKind::ApprovedWorkflowBinding { binding_id } => {
                        ("APPROVED_WORKFLOW_BINDING", Some(binding_id.clone()))
                    }
                };
                let expires = StoredActivation::bounded_expiry(request.now, request.lifetime_secs);
                // Singleton table: replacing the row IS deselecting the old one.
                tx.execute(
                    "INSERT INTO skill_pack_activation
                     (singleton, activation_id, skill_id, version, body_sha256,
                      selection_kind, binding_id, activated_at, expires_at)
                     VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(singleton) DO UPDATE SET
                        activation_id=excluded.activation_id,
                        skill_id=excluded.skill_id,
                        version=excluded.version,
                        body_sha256=excluded.body_sha256,
                        selection_kind=excluded.selection_kind,
                        binding_id=excluded.binding_id,
                        activated_at=excluded.activated_at,
                        expires_at=excluded.expires_at",
                    params![
                        request.activation_id,
                        request.skill_id,
                        request.version,
                        request.body_sha256,
                        kind,
                        binding,
                        request.now.to_rfc3339(),
                        expires.to_rfc3339(),
                    ],
                )?;
                tx.commit()?;
                Ok(Some(SkillPackOutcome::Applied {
                    id: request.activation_id.clone(),
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn clear_activation(&self) -> Result<SkillPackOutcome, CoreError> {
        let result: Option<SkillPackOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let n = tx.execute("DELETE FROM skill_pack_activation", [])?;
                tx.commit()?;
                Ok(Some(if n == 0 {
                    SkillPackOutcome::AlreadyInState {
                        current: "no_activation".to_string(),
                    }
                } else {
                    SkillPackOutcome::Applied {
                        id: "activation".to_string(),
                    }
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn record_activation_audit(
        &self,
        audit: SkillActivationAudit,
    ) -> Result<SkillPackOutcome, CoreError> {
        let result: Option<SkillPackOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let decisions =
                    serde_json::to_string(&audit.capability_decisions).map_err(|e| {
                        StorageError::Internal(format!("capability decision encode failed: {e}"))
                    })?;
                tx.execute(
                    "INSERT OR IGNORE INTO skill_pack_activation_audit
                     (audit_id, skill_id, version, body_sha256, selection_kind, decision,
                      blocked_reason, capability_decisions_json, occurred_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        audit.audit_id,
                        audit.skill_id,
                        audit.version,
                        audit.body_sha256,
                        audit.selection_kind,
                        audit.decision,
                        audit.blocked_reason,
                        decisions,
                        audit.occurred_at.to_rfc3339(),
                    ],
                )?;
                tx.commit()?;
                Ok(Some(SkillPackOutcome::Applied {
                    id: audit.audit_id.clone(),
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn remove_for_install(&self, install_id: &str) -> Result<SkillPackOutcome, CoreError> {
        let install_id = install_id.to_string();
        let result: Option<SkillPackOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                // Child first: the activation points at a catalog row.
                //
                // #9786 measured what this delete is worth. Disabling it leaves
                // `remove_for_install_clears_the_catalog_and_its_activation`
                // passing — `skill_pack_activation.skill_id REFERENCES
                // skill_pack_catalog(skill_id) ON DELETE CASCADE` fires and
                // removes the row anyway. So the previous comment's claim that
                // this is belt-and-braces is now a measurement, not a guess.
                //
                // It stays because it does not depend on the PRAGMA being on.
                // #9735 found a dozen comments reasoning from the wrong value of
                // that setting; a delete that is correct either way is cheaper
                // than being sure which way it reads next year.
                tx.execute(
                    "DELETE FROM skill_pack_activation
                     WHERE skill_id IN (SELECT skill_id FROM skill_pack_catalog
                                        WHERE install_id = ?1)",
                    [&install_id],
                )?;
                let n = tx.execute(
                    "DELETE FROM skill_pack_catalog WHERE install_id = ?1",
                    [&install_id],
                )?;
                tx.commit()?;
                Ok(Some(if n == 0 {
                    SkillPackOutcome::NotFound
                } else {
                    SkillPackOutcome::Applied {
                        id: install_id.clone(),
                    }
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }
}

#[async_trait]
impl SkillPackCatalogQueryPort for SqliteStorage {
    async fn list_skill_packs(&self) -> Result<Vec<SkillPackEntry>, CoreError> {
        self.with_conn_read(move |conn| {
            let sql = format!("SELECT {CATALOG_COLUMNS} FROM skill_pack_catalog ORDER BY skill_id");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], row_to_entry)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row??);
            }
            Ok(out)
        })
        .await
        .map_err(Into::into)
    }

    async fn get_skill_pack(&self, skill_id: &str) -> Result<Option<SkillPackEntry>, CoreError> {
        let skill_id = skill_id.to_string();
        self.with_conn_read(move |conn| {
            let sql =
                format!("SELECT {CATALOG_COLUMNS} FROM skill_pack_catalog WHERE skill_id = ?1");
            let row = conn.query_row(&sql, [&skill_id], row_to_entry).optional()?;
            match row {
                Some(inner) => Ok(Some(inner?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn get_activation(&self) -> Result<Option<StoredActivation>, CoreError> {
        self.with_conn_read(move |conn| {
            let row = conn
                .query_row(
                    "SELECT activation_id, skill_id, version, body_sha256, selection_kind,
                            binding_id, activated_at, expires_at
                     FROM skill_pack_activation WHERE singleton = 1",
                    [],
                    row_to_activation,
                )
                .optional()?;
            match row {
                Some(inner) => Ok(Some(inner?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn reference_graph(&self) -> Result<BTreeMap<String, Vec<String>>, CoreError> {
        self.with_conn_read(move |conn| {
            let mut stmt =
                conn.prepare("SELECT skill_id, skill_references FROM skill_pack_catalog")?;
            let rows = stmt.query_map([], |r| {
                let id: String = r.get(0)?;
                let raw: String = r.get(1)?;
                Ok((id, raw))
            })?;
            let mut graph = BTreeMap::new();
            for row in rows {
                let (id, raw) = row?;
                graph.insert(id, decode_list(&raw)?);
            }
            Ok(graph)
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::extension::{
        Contribution, ContributionKind, ExecutionLocation, ExtensionManifest, RuntimeKind,
        SignatureState, SourceKind,
    };
    use maekon_core::ports::extension_registry::{
        ExtensionRegistryCommandPort, RegisterPackageRequest,
    };

    /// Real storage: `open_in_memory` runs the whole migration chain, so v52's
    /// `REFERENCES extension_installs(install_id)` points at the actual v50
    /// table rather than a stand-in.
    ///
    /// #9786 review CORRECTION: an earlier version of this comment said
    /// "nothing exercised the pair against the schema production runs". That was
    /// wrong — `tests/skill_pack_registry_reopen.rs` already drives
    /// `register_skill_pack` / `record_activation` / `list_skill_packs` /
    /// `reference_graph` / `get_activation` / `clear_activation` against a
    /// disk-backed `SqliteStorage::open`, which runs the identical chain. The
    /// claim was written after grepping this file for `#[cfg(test)]` and the
    /// tree for `SkillPackRegistry|skill_pack_catalog`; the reopen file uses
    /// neither literal, so a too-narrow search became a statement of fact.
    ///
    /// What was actually uncovered, and what these tests add: `remove_for_install`
    /// (no caller or test anywhere), the FK against the REAL v50 table (v52's own
    /// migration tests satisfy it with a stripped three-column stand-in, which an
    /// FK accepts because it only checks the referenced column), and the cascade
    /// in isolation.
    fn storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).expect("in-memory sqlite")
    }

    fn manifest(extension_id: &str) -> ExtensionManifest {
        ExtensionManifest {
            extension_id: extension_id.to_string(),
            version: "1.0.0".to_string(),
            publisher_id: "maekon".to_string(),
            package_digest: "sha256:abc".to_string(),
            source_kind: SourceKind::AppBundle,
            signature_state: SignatureState::AppBundleTrusted,
            signature_key_id: None,
            signed_manifest_digest: None,
            manifest_schema: 1,
            host_api_min: 1,
            host_api_max: 2,
            runtime_kind: RuntimeKind::FirstPartyBuiltin,
            execution_location: ExecutionLocation::Local,
            entry_point: "builtin:review".to_string(),
            contributions: vec![Contribution {
                contribution_id: "review.pack".to_string(),
                kind: ContributionKind::SkillPack,
                api_version: 1,
                requested_capabilities: vec![],
            }],
            permission_profile_id: None,
            external_egress: vec![],
            data_classification: None,
            retention_class: None,
            update_channel: None,
            rollback_window: None,
            minimum_allowed_version: None,
            uninstall_cleanup: vec![],
        }
    }

    /// Create the `extension_installs` parent through the production path.
    ///
    /// The extension id is derived from `install_id` because `register_package`
    /// conflicts on `extension_id`, not `install_id` (v50 `UNIQUE(extension_id)`).
    /// A fixed id would make a second `seed_install` on the same storage a silent
    /// no-op that still returns `Ok`, so the install would never exist and the
    /// test would be measuring something other than what it says (#9786 review).
    async fn seed_install(s: &SqliteStorage, install_id: &str) {
        s.register_package(RegisterPackageRequest {
            install_id: install_id.to_string(),
            manifest: manifest(&format!("com.maekon.review.{install_id}")),
            now: Utc::now(),
        })
        .await
        .expect("register the owning package");
    }

    fn entry(skill_id: &str, install_id: &str) -> SkillPackEntry {
        SkillPackEntry {
            skill_id: skill_id.to_string(),
            install_id: install_id.to_string(),
            extension_id: "com.maekon.review".to_string(),
            contribution_id: "review.pack".to_string(),
            contribution_kind: ContributionKind::SkillPack,
            version: "1.0.0".to_string(),
            publisher_id: "maekon".to_string(),
            body_sha256: "sha256:aa".to_string(),
            required_capabilities: vec!["context.read".to_string()],
            optional_capabilities: vec!["context.write".to_string()],
            references: vec![],
        }
    }

    async fn register(s: &SqliteStorage, e: SkillPackEntry) -> SkillPackOutcome {
        s.register_skill_pack(RegisterSkillPackRequest {
            entry: e,
            now: Utc::now(),
        })
        .await
        .expect("register_skill_pack")
    }

    /// Roundtrip through the real schema.
    ///
    /// #9786 review: this overlaps `tests/skill_pack_registry_reopen.rs`'s
    /// `catalog_entry_and_activation_survive_reopen`, which already asserts
    /// `body_sha256` / `contribution_kind` / `required_capabilities` /
    /// `references`. Kept for the two columns that file does not read back —
    /// `optional_capabilities` and `install_id` — and `install_id` is the one
    /// that matters here, since it is the FK column the tests below turn on.
    #[tokio::test]
    async fn register_then_read_back_preserves_every_field() {
        let s = storage();
        seed_install(&s, "inst_1").await;
        let out = register(&s, entry("sk.review", "inst_1")).await;
        assert!(matches!(out, SkillPackOutcome::Applied { .. }), "{out:?}");

        let got = s
            .get_skill_pack("sk.review")
            .await
            .expect("get_skill_pack")
            .expect("the entry just registered");
        // The list columns go through an encode/decode pair, which is the part
        // a roundtrip can actually get wrong.
        assert_eq!(got.required_capabilities, vec!["context.read".to_string()]);
        assert_eq!(got.optional_capabilities, vec!["context.write".to_string()]);
        assert_eq!(got.references, Vec::<String>::new());
        assert_eq!(got.body_sha256, "sha256:aa");
        assert_eq!(got.install_id, "inst_1");
    }

    /// The FK to `extension_installs` is live against the real v50 table.
    ///
    /// This is what the migration tests could not show: theirs satisfy the FK
    /// with a three-column stand-in, so an entry pointing at an install that was
    /// never created is exactly the case neither layer covered.
    #[tokio::test]
    async fn registering_against_an_unknown_install_is_refused() {
        let s = storage();
        let err = s
            .register_skill_pack(RegisterSkillPackRequest {
                entry: entry("sk.orphan", "inst_never_created"),
                now: Utc::now(),
            })
            .await
            .expect_err("an entry may not outlive the install that owns it");
        let msg = err.to_string();
        assert!(
            msg.contains("FOREIGN KEY constraint failed"),
            "expected the engine to refuse the orphan, got: {msg}"
        );
    }

    /// `remove_for_install` deletes children before parents, under real FK
    /// enforcement — the path #9735's review flagged as never verified.
    #[tokio::test]
    async fn remove_for_install_clears_the_catalog_and_its_activation() {
        let s = storage();
        seed_install(&s, "inst_1").await;
        register(&s, entry("sk.review", "inst_1")).await;
        s.record_activation(RecordActivationRequest {
            activation_id: "act_1".to_string(),
            skill_id: "sk.review".to_string(),
            version: "1.0.0".to_string(),
            body_sha256: "sha256:aa".to_string(),
            selection: SkillSelectionKind::ExplicitUserSelection,
            now: Utc::now(),
            lifetime_secs: 3600,
        })
        .await
        .expect("record_activation");
        assert!(s.get_activation().await.expect("get_activation").is_some());

        let out = s
            .remove_for_install("inst_1")
            .await
            .expect("remove_for_install");
        assert!(matches!(out, SkillPackOutcome::Applied { .. }), "{out:?}");

        assert!(
            s.get_skill_pack("sk.review")
                .await
                .expect("get_skill_pack")
                .is_none(),
            "the catalog entry must be gone"
        );
        assert!(
            s.get_activation().await.expect("get_activation").is_none(),
            "the activation pointed at a row that no longer exists, so it must \
             be gone too"
        );
    }

    /// The cascade alone removes the activation — measured, not assumed.
    ///
    /// #9786: `remove_for_install` deletes the activation explicitly first, and
    /// the adapter comment called that belt-and-braces. Deleting the catalog row
    /// directly, with no explicit child delete anywhere in the path, shows the
    /// engine doing the work: `skill_pack_activation.skill_id REFERENCES
    /// skill_pack_catalog(skill_id) ON DELETE CASCADE` under the enforcement
    /// #9735 established.
    ///
    /// Worth pinning separately because the outcome test above passes whether
    /// the explicit delete runs or not — it cannot tell which mechanism did it,
    /// and a test that cannot distinguish them cannot notice the cascade going
    /// away.
    #[tokio::test]
    async fn deleting_a_catalog_row_cascades_to_its_activation() {
        let s = storage();
        seed_install(&s, "inst_1").await;
        register(&s, entry("sk.review", "inst_1")).await;
        s.record_activation(RecordActivationRequest {
            activation_id: "act_1".to_string(),
            skill_id: "sk.review".to_string(),
            version: "1.0.0".to_string(),
            body_sha256: "sha256:aa".to_string(),
            selection: SkillSelectionKind::ExplicitUserSelection,
            now: Utc::now(),
            lifetime_secs: 3600,
        })
        .await
        .expect("record_activation");

        // Delete the parent directly, bypassing `remove_for_install` entirely.
        {
            let conn = s.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "DELETE FROM skill_pack_catalog WHERE skill_id = 'sk.review'",
                    [],
                )
                .expect("delete the catalog row");
        }

        assert!(
            s.get_activation().await.expect("get_activation").is_none(),
            "the declared ON DELETE CASCADE must remove the orphaned activation"
        );
    }

    /// Removing an install that owns nothing reports `NotFound` rather than
    /// claiming it deleted something.
    #[tokio::test]
    async fn remove_for_install_reports_not_found_when_nothing_is_owned() {
        let s = storage();
        seed_install(&s, "inst_1").await;
        let out = s
            .remove_for_install("inst_1")
            .await
            .expect("remove_for_install");
        assert!(matches!(out, SkillPackOutcome::NotFound), "{out:?}");
    }
}
