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
//!   `REFERENCES ... ON DELETE CASCADE` clauses in v52 are inert because this
//!   workspace runs with `foreign_keys` OFF (ADR-028 Amendment B3), so relying
//!   on them would leave orphaned activations pointing at uninstalled packages.
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
                // Child first: the activation points at a catalog row, and the
                // FK cascade is inert with foreign_keys OFF.
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
