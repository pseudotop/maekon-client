//! SQLite adapter for the Extension registry ports (ADR-029, #8586).
//!
//! The adapter executes already-decided transitions inside a transaction and
//! records a metadata-only audit row. It never invents trust: the fail-closed
//! §10 gates come from `ExtensionManifest::evaluate_availability`, and the
//! installation state machine comes from `InstallationState::can_transition_to`.
//!
//! Uninstall deletes child rows explicitly in the same transaction. #9735: this
//! used to be justified by "the `foreign_keys` PRAGMA is off" — it is ON, so the
//! declared cascades fire too. The explicit child-first ordering stays because
//! it is correct either way and does not depend on every table having a
//! declared FK.

// OOS-TBD: ADR-013 file split — command port, query port, and row helpers can
// move into an `extension_registry_impl/` directory module if this grows.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::extension::{
    AccountAuthentication, Availability, CapabilityGrant, Enablement, ExtensionInstall,
    ExtensionManifest, ExtensionProvenance, ExtensionReadiness, Health, InstallationState,
    SignatureState, SourceKind, UnavailableReason, UpdateState,
};
use maekon_core::ports::extension_registry::{
    ExtensionRegistryCommandPort, ExtensionRegistryQueryPort, InstallRequest, RecordHealthRequest,
    RegisterPackageRequest, RegistryOutcome, RollbackRequest, SetEnablementRequest,
    SetGrantRequest, UninstallRequest, UpdateRequest,
};
use rusqlite::{params, OptionalExtension, Transaction};

use super::SqliteStorage;
use crate::error::StorageError;

fn parse_ts(value: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StorageError::Internal(format!("extension timestamp decode failed: {e}")))
}

/// Metadata-only audit row: id/version/source/signature state and the outcome.
/// Never a secret, body, or payload (§11).
#[allow(clippy::too_many_arguments)] // metadata-only audit row: natural column arity (§11)
fn write_audit(
    tx: &Transaction<'_>,
    install_id: &str,
    extension_id: &str,
    version: &str,
    source_kind: &str,
    signature_state: &str,
    event_kind: &str,
    outcome: &str,
    now: DateTime<Utc>,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO extension_registry_audit
         (audit_id, install_id, extension_id, version, source_kind, signature_state,
          event_kind, outcome, occurred_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            format!("{install_id}:{event_kind}:{}", now.timestamp_micros()),
            install_id,
            extension_id,
            version,
            source_kind,
            signature_state,
            event_kind,
            outcome,
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Row snapshot used for compare-and-swap plus audit context.
struct InstallRow {
    extension_id: String,
    version: String,
    source_kind: String,
    signature_state: String,
    installation: String,
    enablement: String,
    update_state: String,
    previous_version: Option<String>,
    revision: i64,
}

fn load_row(tx: &Transaction<'_>, install_id: &str) -> Result<Option<InstallRow>, StorageError> {
    Ok(tx
        .query_row(
            "SELECT extension_id, version, source_kind, signature_state, installation,
                    enablement, update_state, previous_version, revision
             FROM extension_installs WHERE install_id = ?1",
            [install_id],
            |r| {
                Ok(InstallRow {
                    extension_id: r.get(0)?,
                    version: r.get(1)?,
                    source_kind: r.get(2)?,
                    signature_state: r.get(3)?,
                    installation: r.get(4)?,
                    enablement: r.get(5)?,
                    update_state: r.get(6)?,
                    previous_version: r.get(7)?,
                    revision: r.get(8)?,
                })
            },
        )
        .optional()?)
}

fn load_manifest_json(
    tx: &Transaction<'_>,
    install_id: &str,
) -> Result<Option<ExtensionManifest>, StorageError> {
    let json: Option<String> = tx
        .query_row(
            "SELECT manifest_json FROM extension_manifests WHERE install_id = ?1",
            [install_id],
            |r| r.get(0),
        )
        .optional()?;
    match json {
        Some(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|e| StorageError::Internal(format!("manifest decode failed: {e}"))),
        None => Ok(None),
    }
}

fn skipped_err() -> CoreError {
    StorageError::Internal("extension registry mutation skipped during erasure".to_string()).into()
}

#[async_trait]
impl ExtensionRegistryCommandPort for SqliteStorage {
    async fn register_package(
        &self,
        request: RegisterPackageRequest,
    ) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let m = &request.manifest;
                // Registration records provenance; it never installs.
                let provenance = match m.source_kind {
                    SourceKind::AppBundle => ExtensionProvenance::Bundled,
                    _ => ExtensionProvenance::CuratedLocal,
                };
                let manifest_json = serde_json::to_string(m)
                    .map_err(|e| StorageError::Internal(format!("manifest encode failed: {e}")))?;
                let inserted = tx.execute(
                    "INSERT INTO extension_installs
                     (install_id, extension_id, version, provenance, source_kind, signature_state,
                      installation, enablement, authentication, capability_grant, update_state,
                      health_json, previous_version, revision, created_at, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,'NOT_INSTALLED','DISABLED','NOT_REQUIRED',
                             'NOT_REQUESTED','CURRENT',?7,NULL,1,?8,?8)
                     ON CONFLICT(extension_id) DO NOTHING",
                    params![
                        request.install_id,
                        m.extension_id,
                        m.version,
                        provenance.as_sql_str(),
                        m.source_kind.as_sql_str(),
                        m.signature_state.as_sql_str(),
                        serde_json::to_string(&Health::Unknown).unwrap_or_else(|_| "{}".into()),
                        request.now.to_rfc3339(),
                    ],
                )?;
                if inserted == 0 {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::AlreadyInState {
                        current: "registered".to_string(),
                    }));
                }
                tx.execute(
                    "INSERT INTO extension_manifests
                     (install_id, extension_id, version, publisher_id, package_digest,
                      manifest_schema, host_api_min, host_api_max, manifest_json, recorded_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        request.install_id,
                        m.extension_id,
                        m.version,
                        m.publisher_id,
                        m.package_digest,
                        m.manifest_schema,
                        m.host_api_min,
                        m.host_api_max,
                        manifest_json,
                        request.now.to_rfc3339(),
                    ],
                )?;
                write_audit(
                    &tx,
                    &request.install_id,
                    &m.extension_id,
                    &m.version,
                    &m.source_kind.as_sql_str(),
                    m.signature_state.as_sql_str(),
                    "register",
                    "registered",
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: request.install_id.clone(),
                    revision: 1,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn install(&self, request: InstallRequest) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                if row.installation == InstallationState::Installed.as_sql_str() {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::AlreadyInState {
                        current: row.installation,
                    }));
                }
                if row.revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                }
                // Fail-closed §10 gates before any state change.
                let Some(manifest) = load_manifest_json(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::Unavailable {
                        reason: UnavailableReason::ManifestSchemaUnsupported,
                    }));
                };
                if let Availability::Unavailable { reason } = manifest.evaluate_availability() {
                    write_audit(
                        &tx,
                        id,
                        &row.extension_id,
                        &row.version,
                        &row.source_kind,
                        &row.signature_state,
                        "install",
                        reason.as_str(),
                        request.now,
                    )?;
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::Unavailable { reason }));
                }
                let from = InstallationState::from_sql_str(&row.installation).ok_or_else(|| {
                    StorageError::Internal(format!("unknown installation {}", row.installation))
                })?;
                if !from.can_transition_to(InstallationState::Installed)
                    && !from.can_transition_to(InstallationState::Installing)
                {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::IllegalTransition {
                        from: row.installation.clone(),
                        to: InstallationState::Installed.as_sql_str().to_string(),
                    }));
                }
                let new_rev = row.revision + 1;
                tx.execute(
                    "UPDATE extension_installs
                     SET installation='INSTALLED', revision=?2, updated_at=?3
                     WHERE install_id=?1",
                    params![id, new_rev, request.now.to_rfc3339()],
                )?;
                write_audit(
                    &tx,
                    id,
                    &row.extension_id,
                    &row.version,
                    &row.source_kind,
                    &row.signature_state,
                    "install",
                    "installed",
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn set_enablement(
        &self,
        request: SetEnablementRequest,
    ) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let target = if request.enabled {
                    Enablement::Enabled
                } else {
                    Enablement::Disabled
                };
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                if row.enablement == target.as_sql_str() {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::AlreadyInState {
                        current: row.enablement,
                    }));
                }
                if row.revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                }
                // Only an installed package can be enabled; disable is always allowed.
                if request.enabled && row.installation != InstallationState::Installed.as_sql_str()
                {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::IllegalTransition {
                        from: row.installation.clone(),
                        to: "ENABLED".to_string(),
                    }));
                }
                let new_rev = row.revision + 1;
                tx.execute(
                    "UPDATE extension_installs SET enablement=?2, revision=?3, updated_at=?4
                     WHERE install_id=?1",
                    params![id, target.as_sql_str(), new_rev, request.now.to_rfc3339()],
                )?;
                write_audit(
                    &tx,
                    id,
                    &row.extension_id,
                    &row.version,
                    &row.source_kind,
                    &row.signature_state,
                    "set_enablement",
                    target.as_sql_str(),
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn update(&self, request: UpdateRequest) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                if row.revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                }
                if row.installation != InstallationState::Installed.as_sql_str() {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::IllegalTransition {
                        from: row.installation.clone(),
                        to: "UPDATE".to_string(),
                    }));
                }
                let new_rev = row.revision + 1;
                // Activate atomically, keeping the previous known-good version so a
                // later failure can roll back rather than strand the install (§8).
                tx.execute(
                    "UPDATE extension_installs
                     SET version=?2, previous_version=?3, update_state='CURRENT',
                         revision=?4, updated_at=?5
                     WHERE install_id=?1",
                    params![
                        id,
                        request.target_version,
                        row.version,
                        new_rev,
                        request.now.to_rfc3339()
                    ],
                )?;
                write_audit(
                    &tx,
                    id,
                    &row.extension_id,
                    &request.target_version,
                    &row.source_kind,
                    &row.signature_state,
                    "update",
                    "activated",
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn rollback(&self, request: RollbackRequest) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                if row.revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                }
                // No known-good version: stay explicitly unavailable rather than
                // silently pretending the install is fine (§8).
                let Some(previous) = row.previous_version.clone() else {
                    write_audit(
                        &tx,
                        id,
                        &row.extension_id,
                        &row.version,
                        &row.source_kind,
                        &row.signature_state,
                        "rollback",
                        "no_previous_version",
                        request.now,
                    )?;
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::Unavailable {
                        reason: UnavailableReason::SourceUnhealthy,
                    }));
                };
                let new_rev = row.revision + 1;
                tx.execute(
                    "UPDATE extension_installs
                     SET version=?2, previous_version=NULL, update_state='CURRENT',
                         revision=?3, updated_at=?4
                     WHERE install_id=?1",
                    params![id, previous, new_rev, request.now.to_rfc3339()],
                )?;
                write_audit(
                    &tx,
                    id,
                    &row.extension_id,
                    &previous,
                    &row.source_kind,
                    &row.signature_state,
                    "rollback",
                    "restored",
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn uninstall(&self, request: UninstallRequest) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                if row.revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                }
                // An in-flight activation must be aborted first (Amendment I3a).
                let update = UpdateState::from_sql_str(&row.update_state);
                if update.is_some_and(|u| u.is_activation_in_flight()) {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::IllegalTransition {
                        from: row.update_state.clone(),
                        to: "UNINSTALLED".to_string(),
                    }));
                }
                // Every declared cleanup step must be confirmed; a missing step is
                // reported, never silently treated as done (§8).
                let declared: Vec<String> = load_manifest_json(&tx, id)?
                    .map(|m| m.uninstall_cleanup)
                    .unwrap_or_default();
                let missing: Vec<String> = declared
                    .iter()
                    .filter(|step| !request.completed_cleanup.contains(step))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    write_audit(
                        &tx,
                        id,
                        &row.extension_id,
                        &row.version,
                        &row.source_kind,
                        &row.signature_state,
                        "uninstall",
                        UnavailableReason::CleanupIncomplete.as_str(),
                        request.now,
                    )?;
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::CleanupIncomplete { missing }));
                }
                let new_rev = row.revision + 1;
                // Application-ordered child-first delete (FK cascade is inert), then
                // mark the install uninstalled. The audit trail is preserved.
                tx.execute("DELETE FROM extension_manifests WHERE install_id=?1", [id])?;
                tx.execute(
                    "UPDATE extension_installs
                     SET installation='UNINSTALLED', enablement='DISABLED',
                         authentication='NOT_REQUIRED', capability_grant='NOT_REQUESTED',
                         revision=?2, updated_at=?3
                     WHERE install_id=?1",
                    params![id, new_rev, request.now.to_rfc3339()],
                )?;
                write_audit(
                    &tx,
                    id,
                    &row.extension_id,
                    &row.version,
                    &row.source_kind,
                    &row.signature_state,
                    "uninstall",
                    "uninstalled",
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn record_health(
        &self,
        request: RecordHealthRequest,
    ) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                let health_json = serde_json::to_string(&request.health)
                    .map_err(|e| StorageError::Internal(format!("health encode failed: {e}")))?;
                // Health is observational: it does not consume a revision.
                tx.execute(
                    "UPDATE extension_installs SET health_json=?2, updated_at=?3
                     WHERE install_id=?1",
                    params![id, health_json, request.now.to_rfc3339()],
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: row.revision,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }

    async fn set_grant(&self, request: SetGrantRequest) -> Result<RegistryOutcome, CoreError> {
        let result: Option<RegistryOutcome> = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction()?;
                let id = &request.install_id;
                let Some(row) = load_row(&tx, id)? else {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                };
                if row.revision != request.expected_revision {
                    tx.commit()?;
                    return Ok(Some(RegistryOutcome::RevisionConflict));
                }
                let new_rev = row.revision + 1;
                tx.execute(
                    "UPDATE extension_installs SET capability_grant=?2, revision=?3, updated_at=?4
                     WHERE install_id=?1",
                    params![
                        id,
                        request.grant.as_sql_str(),
                        new_rev,
                        request.now.to_rfc3339()
                    ],
                )?;
                write_audit(
                    &tx,
                    id,
                    &row.extension_id,
                    &row.version,
                    &row.source_kind,
                    &row.signature_state,
                    "set_grant",
                    request.grant.as_sql_str(),
                    request.now,
                )?;
                tx.commit()?;
                Ok(Some(RegistryOutcome::Applied {
                    install_id: id.clone(),
                    revision: new_rev,
                }))
            })
            .await
            .map_err(Into::<CoreError>::into)?;
        result.ok_or_else(skipped_err)
    }
}

// ---- Query port ----

#[allow(clippy::type_complexity)]
fn row_to_install(
    r: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<ExtensionInstall, StorageError>> {
    let install_id: String = r.get(0)?;
    let extension_id: String = r.get(1)?;
    let version: String = r.get(2)?;
    let provenance: String = r.get(3)?;
    let source_kind: String = r.get(4)?;
    let signature_state: String = r.get(5)?;
    let installation: String = r.get(6)?;
    let enablement: String = r.get(7)?;
    let authentication: String = r.get(8)?;
    let grant: String = r.get(9)?;
    let update_state: String = r.get(10)?;
    let health_json: String = r.get(11)?;
    let previous_version: Option<String> = r.get(12)?;
    let revision: i64 = r.get(13)?;
    let created_at: String = r.get(14)?;
    let updated_at: String = r.get(15)?;
    Ok((|| {
        let unknown = |field: &str, v: &str| {
            StorageError::Internal(format!("unknown extension {field} value {v}"))
        };
        Ok(ExtensionInstall {
            install_id,
            extension_id,
            version,
            provenance: ExtensionProvenance::from_sql_str(&provenance)
                .ok_or_else(|| unknown("provenance", &provenance))?,
            source_kind: SourceKind::from_sql_str(&source_kind),
            signature_state: SignatureState::from_sql_str(&signature_state)
                .ok_or_else(|| unknown("signature_state", &signature_state))?,
            installation: InstallationState::from_sql_str(&installation)
                .ok_or_else(|| unknown("installation", &installation))?,
            enablement: Enablement::from_sql_str(&enablement)
                .ok_or_else(|| unknown("enablement", &enablement))?,
            authentication: AccountAuthentication::from_sql_str(&authentication)
                .ok_or_else(|| unknown("authentication", &authentication))?,
            grant: CapabilityGrant::from_sql_str(&grant)
                .ok_or_else(|| unknown("capability_grant", &grant))?,
            update: UpdateState::from_sql_str(&update_state)
                .ok_or_else(|| unknown("update_state", &update_state))?,
            health: serde_json::from_str(&health_json)
                .map_err(|e| StorageError::Internal(format!("health decode failed: {e}")))?,
            previous_version,
            revision,
            created_at: parse_ts(&created_at)?,
            updated_at: parse_ts(&updated_at)?,
        })
    })())
}

const INSTALL_COLUMNS: &str = "install_id, extension_id, version, provenance, source_kind,
     signature_state, installation, enablement, authentication, capability_grant,
     update_state, health_json, previous_version, revision, created_at, updated_at";

#[async_trait]
impl ExtensionRegistryQueryPort for SqliteStorage {
    async fn list_installs(&self) -> Result<Vec<ExtensionInstall>, CoreError> {
        self.with_conn_read(move |conn| {
            let sql =
                format!("SELECT {INSTALL_COLUMNS} FROM extension_installs ORDER BY extension_id");
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map([], row_to_install)?
                .collect::<Result<Vec<_>, _>>()?;
            rows.into_iter().collect::<Result<Vec<_>, StorageError>>()
        })
        .await
        .map_err(Into::into)
    }

    async fn get_install(&self, install_id: &str) -> Result<Option<ExtensionInstall>, CoreError> {
        let install_id = install_id.to_string();
        self.with_conn_read(move |conn| {
            let sql =
                format!("SELECT {INSTALL_COLUMNS} FROM extension_installs WHERE install_id = ?1");
            let row = conn
                .query_row(&sql, [&install_id], row_to_install)
                .optional()?;
            match row {
                Some(inner) => Ok(Some(inner?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn get_manifest(&self, install_id: &str) -> Result<Option<ExtensionManifest>, CoreError> {
        let install_id = install_id.to_string();
        self.with_conn_read(move |conn| {
            let json: Option<String> = conn
                .query_row(
                    "SELECT manifest_json FROM extension_manifests WHERE install_id = ?1",
                    [&install_id],
                    |r| r.get(0),
                )
                .optional()?;
            match json {
                Some(raw) => serde_json::from_str(&raw)
                    .map(Some)
                    .map_err(|e| StorageError::Internal(format!("manifest decode failed: {e}"))),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn list_readiness(&self) -> Result<Vec<ExtensionReadiness>, CoreError> {
        let installs = self.list_installs().await?;
        let mut out = Vec::with_capacity(installs.len());
        for install in installs {
            // Availability is re-derived from the stored manifest every time, so a
            // host upgrade that narrows compatibility is reflected immediately.
            let availability = match self.get_manifest(&install.install_id).await? {
                Some(manifest) => manifest.evaluate_availability(),
                None => Availability::Unavailable {
                    reason: UnavailableReason::ManifestSchemaUnsupported,
                },
            };
            out.push(ExtensionReadiness {
                extension_id: install.extension_id,
                install_id: install.install_id,
                provenance: install.provenance,
                availability,
                installation: install.installation,
                enablement: install.enablement,
                authentication: install.authentication,
                grant: install.grant,
                update: install.update,
                health: install.health,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::extension::{
        Contribution, ContributionKind, ExecutionLocation, HealthReason, RuntimeKind,
    };

    fn storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).unwrap()
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
            entry_point: "builtin:calendar".to_string(),
            contributions: vec![Contribution {
                contribution_id: "calendar.read".to_string(),
                kind: ContributionKind::ContextSource,
                api_version: 1,
                requested_capabilities: vec!["context.read".to_string()],
            }],
            permission_profile_id: None,
            external_egress: vec![],
            data_classification: None,
            retention_class: None,
            update_channel: None,
            rollback_window: None,
            minimum_allowed_version: None,
            uninstall_cleanup: vec!["credentials".to_string(), "cursors".to_string()],
        }
    }

    async fn register(s: &SqliteStorage, id: &str, m: ExtensionManifest) {
        let out = s
            .register_package(RegisterPackageRequest {
                install_id: id.to_string(),
                manifest: m,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(
            matches!(out, RegistryOutcome::Applied { .. }),
            "register: {out:?}"
        );
    }

    #[tokio::test]
    async fn register_records_provenance_without_installing() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        // Bundled provenance, but explicitly NOT installed yet (Amendment B1).
        assert_eq!(row.provenance, ExtensionProvenance::Bundled);
        assert_eq!(row.installation, InstallationState::NotInstalled);
        assert_eq!(row.enablement, Enablement::Disabled);
    }

    #[tokio::test]
    async fn install_then_enable_and_readiness_reports_axes() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        let out = s
            .install(InstallRequest {
                install_id: "inst_1".into(),
                expected_revision: 1,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(
            matches!(out, RegistryOutcome::Applied { revision: 2, .. }),
            "{out:?}"
        );

        let out = s
            .set_enablement(SetEnablementRequest {
                install_id: "inst_1".into(),
                enabled: true,
                expected_revision: 2,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(
            matches!(out, RegistryOutcome::Applied { revision: 3, .. }),
            "{out:?}"
        );

        let readiness = s.list_readiness().await.unwrap();
        assert_eq!(readiness.len(), 1);
        let r = &readiness[0];
        assert_eq!(r.availability, Availability::Available);
        assert_eq!(r.installation, InstallationState::Installed);
        assert_eq!(r.enablement, Enablement::Enabled);
        assert_eq!(r.summary_label(), Some("ready"));
    }

    #[tokio::test]
    async fn incompatible_manifest_fails_closed_on_install() {
        let s = storage();
        let mut m = manifest("com.maekon.relay");
        m.execution_location = ExecutionLocation::Relay;
        register(&s, "inst_1", m).await;
        let out = s
            .install(InstallRequest {
                install_id: "inst_1".into(),
                expected_revision: 1,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert_eq!(
            out,
            RegistryOutcome::Unavailable {
                reason: UnavailableReason::ExecutionLocationUnsupported
            }
        );
        // State did not advance.
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.installation, InstallationState::NotInstalled);
    }

    #[tokio::test]
    async fn enable_requires_installed_and_revision_matches() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        // Enabling a not-installed package is an illegal transition.
        let out = s
            .set_enablement(SetEnablementRequest {
                install_id: "inst_1".into(),
                enabled: true,
                expected_revision: 1,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(
            matches!(out, RegistryOutcome::IllegalTransition { .. }),
            "{out:?}"
        );

        s.install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();
        // Stale revision is rejected.
        let out = s
            .set_enablement(SetEnablementRequest {
                install_id: "inst_1".into(),
                enabled: true,
                expected_revision: 1,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert_eq!(out, RegistryOutcome::RevisionConflict);
    }

    #[tokio::test]
    async fn update_keeps_previous_version_and_rollback_restores_it() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        s.install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();

        s.update(UpdateRequest {
            install_id: "inst_1".into(),
            target_version: "2.0.0".into(),
            expected_revision: 2,
            now: Utc::now(),
        })
        .await
        .unwrap();
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.version, "2.0.0");
        assert_eq!(row.previous_version.as_deref(), Some("1.0.0"));

        s.rollback(RollbackRequest {
            install_id: "inst_1".into(),
            expected_revision: 3,
            now: Utc::now(),
        })
        .await
        .unwrap();
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.version, "1.0.0");
        assert!(row.previous_version.is_none());
    }

    #[tokio::test]
    async fn rollback_without_previous_version_stays_explicitly_unavailable() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        s.install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();
        let out = s
            .rollback(RollbackRequest {
                install_id: "inst_1".into(),
                expected_revision: 2,
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(
            matches!(out, RegistryOutcome::Unavailable { .. }),
            "{out:?}"
        );
    }

    #[tokio::test]
    async fn uninstall_reports_missing_cleanup_and_never_silently_completes() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        s.install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();

        // Only one of the two declared cleanup steps confirmed.
        let out = s
            .uninstall(UninstallRequest {
                install_id: "inst_1".into(),
                expected_revision: 2,
                completed_cleanup: vec!["credentials".into()],
                now: Utc::now(),
            })
            .await
            .unwrap();
        match out {
            RegistryOutcome::CleanupIncomplete { missing } => {
                assert_eq!(missing, vec!["cursors".to_string()]);
            }
            other => panic!("expected CleanupIncomplete, got {other:?}"),
        }
        // Still installed — uninstall did not partially apply.
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.installation, InstallationState::Installed);

        // With every step confirmed it completes and clears the manifest.
        let out = s
            .uninstall(UninstallRequest {
                install_id: "inst_1".into(),
                expected_revision: 2,
                completed_cleanup: vec!["credentials".into(), "cursors".into()],
                now: Utc::now(),
            })
            .await
            .unwrap();
        assert!(matches!(out, RegistryOutcome::Applied { .. }), "{out:?}");
        let row = s.get_install("inst_1").await.unwrap().unwrap();
        assert_eq!(row.installation, InstallationState::Uninstalled);
        assert!(s.get_manifest("inst_1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn health_is_observational_and_surfaces_in_readiness() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        s.install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();
        s.set_enablement(SetEnablementRequest {
            install_id: "inst_1".into(),
            enabled: true,
            expected_revision: 2,
            now: Utc::now(),
        })
        .await
        .unwrap();

        s.record_health(RecordHealthRequest {
            install_id: "inst_1".into(),
            health: Health::Degraded {
                reason: HealthReason::RateLimited,
            },
            now: Utc::now(),
        })
        .await
        .unwrap();

        let r = &s.list_readiness().await.unwrap()[0];
        assert_eq!(
            r.health,
            Health::Degraded {
                reason: HealthReason::RateLimited
            }
        );
        // A degraded extension must never read as "ready".
        assert_eq!(r.summary_label(), Some("degraded"));
        // Health did not consume a revision.
        assert_eq!(s.get_install("inst_1").await.unwrap().unwrap().revision, 3);
    }

    #[tokio::test]
    async fn registry_audit_records_metadata_only() {
        let s = storage();
        register(&s, "inst_1", manifest("com.maekon.a")).await;
        s.install(InstallRequest {
            install_id: "inst_1".into(),
            expected_revision: 1,
            now: Utc::now(),
        })
        .await
        .unwrap();
        let rows: Vec<(String, String, String)> = s
            .with_conn_read(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT event_kind, outcome, signature_state FROM extension_registry_audit
                     ORDER BY occurred_at",
                )?;
                let out = stmt
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(out)
            })
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "register");
        assert_eq!(rows[1].0, "install");
        assert_eq!(rows[1].2, "APP_BUNDLE_TRUSTED");
    }
}
