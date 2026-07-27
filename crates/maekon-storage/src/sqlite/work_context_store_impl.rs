//! Work-context store adapter (ADR-030 §9, #8587/#8589).
//!
//! The adapter merely applies an already-decided merge outcome inside a
//! transaction; the merge rules themselves are owned by
//! `maekon_core::models::work_context::merge_decide`. The cleanup order is
//! enforced by the application (`foreign_keys` PRAGMA off) — revoke/erase
//! delete child rows explicitly first.
//!
//! Cursor advancement is a compare-and-swap (ADR-030 revision I4). The
//! `cursor_revision` token prevents two overlapping ingests from overwriting
//! or rewinding each other's cursor.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::work_context::{
    effective_now, merge_decide, Lifecycle, MergeExisting, MergeIncoming, MergeOutcome,
    RevisionModel, WorkContextEnvelope,
};
use maekon_core::models::work_context_projection::{ProjectionContent, TimelineEvidenceItem};
use maekon_core::ports::work_context::{
    CommitOutcome, CommitPageRequest, CursorState, WorkContextStorePort,
};
use rusqlite::{params, OptionalExtension, Transaction};

use super::work_context_writers::{
    clear_content_for_object, enforce_raw_budget, query_projection, query_timeline,
    write_accepted_content, WriteCtx, RAW_PLANE_DISK_BUDGET_BYTES,
};
use super::SqliteStorage;
use crate::error::StorageError;

fn ts(value: &DateTime<Utc>) -> String {
    value.to_rfc3339()
}

fn parse_ts(value: &str) -> Result<DateTime<Utc>, StorageError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| StorageError::Internal(format!("work-context timestamp decode failed: {e}")))
}

fn parse_ts_opt(value: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
    value.map(|v| parse_ts(&v)).transpose()
}

/// Reads the current merge state of a single source object inside the transaction.
///
/// If a tombstone exists, its lifecycle is the current state (§6). Otherwise the
/// active envelope is used. Only the minimal fields needed for the merge decision
/// are fetched.
fn load_existing(
    tx: &Transaction,
    source_object_key: &str,
    access_epoch_id: i64,
) -> Result<Option<MergeExisting>, StorageError> {
    // Tombstone first — a terminal state dominates the active revision.
    let tomb: Option<(String, String, Option<i64>)> = tx
        .query_row(
            "SELECT lifecycle, revision_fingerprint, source_order
               FROM work_context_tombstones
              WHERE source_object_key = ?1 AND access_epoch_id = ?2
              ORDER BY deleted_at DESC LIMIT 1",
            params![source_object_key, access_epoch_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(StorageError::from)?;
    if let Some((lifecycle, fp, order)) = tomb {
        return Ok(Some(MergeExisting {
            revision_fingerprint: fp,
            // A tombstone is not a content-comparison target, so the model is
            // meaningless, but we set it to monotonic for the comparable path
            // (undelete decision).
            revision_model: RevisionModel::Monotonic,
            source_order: order,
            content_hash: String::new(),
            lifecycle: Lifecycle::from_str_lossy(&lifecycle).unwrap_or(Lifecycle::Deleted),
        }));
    }

    let row: Option<(String, String, Option<i64>, String, String)> = tx
        .query_row(
            "SELECT revision_fingerprint, revision_model, source_order, content_hash, lifecycle
               FROM work_context_envelopes
              WHERE source_object_key = ?1 AND access_epoch_id = ?2 AND lifecycle = 'active'
              ORDER BY ingested_at DESC LIMIT 1",
            params![source_object_key, access_epoch_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(StorageError::from)?;

    Ok(
        row.map(|(fp, model, order, hash, lifecycle)| MergeExisting {
            revision_fingerprint: fp,
            revision_model: RevisionModel::from_str_lossy(&model).unwrap_or(RevisionModel::Opaque),
            source_order: order,
            content_hash: hash,
            lifecycle: Lifecycle::from_str_lossy(&lifecycle).unwrap_or(Lifecycle::Active),
        }),
    )
}

/// Inserts an envelope. The local uniqueness key makes replay idempotent.
///
/// An already-existing (source_object_key, epoch, fingerprint) is silently
/// ignored by `INSERT OR IGNORE` — a replay does not create a second
/// envelope (§9).
fn insert_envelope(tx: &Transaction, env: &WorkContextEnvelope) -> Result<(), StorageError> {
    let relations = serde_json::to_string(&env.relations)
        .map_err(|e| StorageError::Internal(format!("relations encode failed: {e}")))?;
    let access_snap = env
        .access_snapshot
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| StorageError::Internal(format!("access snapshot encode failed: {e}")))?;
    let consent_snap = env
        .consent_snapshot
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|e| StorageError::Internal(format!("consent snapshot encode failed: {e}")))?;

    tx.execute(
        "INSERT OR IGNORE INTO work_context_envelopes (
            envelope_id, schema_version, source_object_key, access_epoch_id,
            revision_fingerprint, extension_id, install_id, account_subject_ref,
            remote_type, remote_id, revision_model, remote_revision, etag, source_order,
            content_hash, kind, classification, retention_class, occurred_at,
            source_updated_at, observed_at, ingested_at, relations_json,
            access_snapshot_json, consent_snapshot_json, ingest_run_id, prior_envelope_id,
            source_cursor_digest, projection_ref, raw_blob_ref, lifecycle, expires_at,
            has_confirmed_ref
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
            ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, 0
        )",
        params![
            env.envelope_id,
            env.schema_version,
            env.source_object_key,
            env.access_epoch_id,
            env.revision_fingerprint,
            env.identity.extension_id,
            env.identity.install_id,
            env.identity.account_subject_ref,
            env.identity.remote_type,
            env.identity.remote_id,
            env.revision_model.as_str(),
            env.remote_revision,
            env.etag,
            env.source_order,
            env.content_hash,
            env.kind.as_str(),
            env.classification.as_str(),
            env.retention_class,
            env.occurred_at.as_ref().map(ts),
            env.source_updated_at.as_ref().map(ts),
            ts(&env.observed_at),
            ts(&env.ingested_at),
            relations,
            access_snap,
            consent_snap,
            env.ingest_run_id,
            env.prior_envelope_id,
            env.source_cursor_digest,
            env.projection_ref,
            env.raw_blob_ref,
            env.lifecycle.as_str(),
            // Revision B1: the expiry is always present and clamped to the hard
            // maximum (365 days).
            ts(&(env.ingested_at
                + chrono::Duration::seconds(
                    super::super::migration::v51_work_context::clamp_envelope_ttl_secs(None),
                ))),
        ],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

/// Marks the prior active envelope as terminal (on supersede/delete).
fn deactivate_prior(
    tx: &Transaction,
    source_object_key: &str,
    access_epoch_id: i64,
    lifecycle: Lifecycle,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE work_context_envelopes
            SET lifecycle = ?3
          WHERE source_object_key = ?1 AND access_epoch_id = ?2 AND lifecycle = 'active'",
        params![source_object_key, access_epoch_id, lifecycle.as_str()],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

/// Writes a suppression tombstone (§6). It carries no content.
fn write_tombstone(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    lifecycle: Lifecycle,
) -> Result<(), StorageError> {
    let expires = env.ingested_at
        + chrono::Duration::seconds(
            super::super::migration::v51_work_context::clamp_tombstone_ttl_secs(None),
        );
    tx.execute(
        "INSERT OR IGNORE INTO work_context_tombstones
            (tombstone_id, source_object_key, access_epoch_id, revision_fingerprint,
             lifecycle, source_order, deleted_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            format!("{}_tomb", env.envelope_id),
            env.source_object_key,
            env.access_epoch_id,
            env.revision_fingerprint,
            lifecycle.as_str(),
            env.source_order,
            ts(&env.ingested_at),
            ts(&expires),
        ],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

/// Writes a content-free lifecycle tombstone — keyed **only by the HMAC
/// `source_object_key`** (it does not carry the raw remote_id). This is the
/// suppression tombstone shape shared by mark_lifecycle, revoke, and retention
/// expiry (§7 content-free plane, §12).
///
/// `source_order` is NULL and `revision_fingerprint` is `lifecycle_<state>` —
/// it suppresses not a specific revision but only the fact that "this epoch of
/// this object is terminal". The bulk paths (revoke/expire) generate the same
/// format via SQL string concatenation, so INSERT OR IGNORE folds duplicates
/// across paths.
fn write_lifecycle_tombstone(
    tx: &Transaction,
    source_object_key: &str,
    access_epoch_id: i64,
    lifecycle: Lifecycle,
    now: &DateTime<Utc>,
) -> Result<(), StorageError> {
    let expires = *now
        + chrono::Duration::seconds(
            super::super::migration::v51_work_context::clamp_tombstone_ttl_secs(None),
        );
    tx.execute(
        "INSERT OR IGNORE INTO work_context_tombstones
            (tombstone_id, source_object_key, access_epoch_id, revision_fingerprint,
             lifecycle, source_order, deleted_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7)",
        params![
            format!(
                "{source_object_key}_{}_{}",
                access_epoch_id,
                lifecycle.as_str()
            ),
            source_object_key,
            access_epoch_id,
            format!("lifecycle_{}", lifecycle.as_str()),
            lifecycle.as_str(),
            ts(now),
            ts(&expires),
        ],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

/// Quarantines an incomparable revision under a deterministic conflict ID
/// (§6 rule 7, revision M1).
fn quarantine_conflict(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    conflict_id: &str,
) -> Result<(), StorageError> {
    let expires = env.ingested_at
        + chrono::Duration::seconds(
            super::super::migration::v51_work_context::clamp_envelope_ttl_secs(None),
        );
    // Revision M1: one row per (source_object_key, epoch). Only the fingerprint
    // set is updated.
    tx.execute(
        "INSERT INTO work_context_conflicts
            (conflict_id, source_object_key, access_epoch_id, fingerprints_json, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(source_object_key, access_epoch_id)
         DO UPDATE SET conflict_id = excluded.conflict_id,
                       fingerprints_json = excluded.fingerprints_json",
        params![
            conflict_id,
            env.source_object_key,
            env.access_epoch_id,
            serde_json::to_string(&[&env.revision_fingerprint]).unwrap_or_else(|_| "[]".into()),
            ts(&env.ingested_at),
            ts(&expires),
        ],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

impl SqliteStorage {
    /// Path that decrypts a retained raw blob to open the original content
    /// (ADR-030 §12 raw-open, #8589).
    ///
    /// Re-authentication and live consent/access re-verification are the
    /// **caller's responsibility** (§8/§12) — this method only decrypts.
    /// Returns `None` if crypto-shredded (row/salt destroyed) or the master key
    /// is absent. If there is no active envelope (terminal/expired) it does not
    /// decrypt — what is stored does not authorize a live read. The return value
    /// is wrapped in `Zeroizing` so it is erased on drop.
    ///
    /// It never flows into a normal JSON DTO — this is exclusively the §12
    /// "reauthenticated encrypted attachment" path.
    pub fn work_context_open_raw(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, StorageError> {
        let Some(key) = self.work_context_raw_key.as_ref() else {
            return Ok(None);
        };
        let read = self.conn.read_lock();
        let row: Option<(Vec<u8>, Vec<u8>, Vec<u8>, String, String)> = read
            .conn()
            .query_row(
                "SELECT r.key_salt, r.nonce, r.ciphertext, r.install_id, e.account_subject_ref
                   FROM work_context_raw_blobs AS r
                   JOIN work_context_envelopes AS e
                     ON e.source_object_key = r.source_object_key
                    AND e.access_epoch_id = r.access_epoch_id
                    AND e.lifecycle = 'active'
                  WHERE r.source_object_key = ?1 AND r.access_epoch_id = ?2
                  LIMIT 1",
                params![source_object_key, access_epoch_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()
            .map_err(StorageError::from)?;
        let Some((key_salt, nonce, ciphertext, install_id, account_subject_ref)) = row else {
            return Ok(None);
        };
        let bundle = crate::encryption::raw_plane::RawPlaneCiphertext {
            key_salt,
            nonce,
            ciphertext,
        };
        // A decryption failure (when the master key changed due to rekey/rollback)
        // is not a hard error but fails closed as "unavailable" — the same None
        // result as terminal/crypto-shred. It does not occur under normal wiring.
        match key.decrypt_raw_plane(&bundle, &install_id, &account_subject_ref) {
            Ok(plaintext) => Ok(Some(plaintext)),
            Err(e) => {
                tracing::warn!(
                    "work-context raw blob for {source_object_key} did not decrypt \
                     (likely a key rotation/rollback); treating as unavailable: {e}"
                );
                Ok(None)
            }
        }
    }

    /// Reconciles the ledger after rekey/rollback: removes raw blobs that the
    /// current master key can no longer decrypt (ADR-030 §7 ephemeral plane, #8589).
    ///
    /// When the underlying DB encryption key (`EncryptionKey`) rotates, the raw
    /// plane subkey derived from its bytes (revision I1) changes, so existing raw
    /// blobs become undecryptable. Since raw is inherently short-lived
    /// (24 hours–7 days) and consent-based, it is safe to **discard without
    /// migrating**. Envelopes/cursors/tombstones/projections are ordinary columns
    /// inside SQLCipher page encryption, so they survive rekey transparently and
    /// ledger/cursor/provenance consistency is preserved — this method only
    /// touches the raw plane.
    ///
    /// The return value is the number of discarded raw blobs. If the master key
    /// is absent (encrypted→plaintext rollback), all raw blobs are discarded
    /// (permanently undecryptable).
    pub async fn reconcile_raw_plane(&self) -> Result<u64, CoreError> {
        let raw_key = self.work_context_raw_key.clone();
        let dropped = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction().map_err(StorageError::from)?;
                let mut to_delete: Vec<String> = Vec::new();
                {
                    let mut stmt = tx
                        .prepare(
                            "SELECT r.raw_blob_ref, r.key_salt, r.nonce, r.ciphertext,
                                    r.install_id, e.account_subject_ref
                               FROM work_context_raw_blobs AS r
                               LEFT JOIN work_context_envelopes AS e
                                 ON e.source_object_key = r.source_object_key
                                AND e.access_epoch_id = r.access_epoch_id
                              GROUP BY r.raw_blob_ref",
                        )
                        .map_err(StorageError::from)?;
                    let rows = stmt
                        .query_map([], |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Vec<u8>>(1)?,
                                row.get::<_, Vec<u8>>(2)?,
                                row.get::<_, Vec<u8>>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                            ))
                        })
                        .map_err(StorageError::from)?;
                    for row in rows {
                        let (rref, key_salt, nonce, ciphertext, install_id, account) =
                            row.map_err(StorageError::from)?;
                        let decodable = match (raw_key.as_ref(), account) {
                            (Some(key), Some(account)) => {
                                let bundle = crate::encryption::raw_plane::RawPlaneCiphertext {
                                    key_salt,
                                    nonce,
                                    ciphertext,
                                };
                                key.decrypt_raw_plane(&bundle, &install_id, &account)
                                    .is_ok()
                            }
                            // Orphan raw is discarded when the key is absent
                            // (plaintext rollback) or the envelope is gone.
                            _ => false,
                        };
                        if !decodable {
                            to_delete.push(rref);
                        }
                    }
                }
                let mut dropped = 0u64;
                for rref in to_delete {
                    dropped += tx
                        .execute(
                            "DELETE FROM work_context_raw_blobs WHERE raw_blob_ref = ?1",
                            params![rref],
                        )
                        .map_err(StorageError::from)? as u64;
                }
                tx.commit().map_err(StorageError::from)?;
                Ok(dropped)
            })
            .await?;
        Ok(dropped)
    }

    /// #8589 diagnostic support: the current row count of the raw plane. Used to
    /// verify crypto-shred (0 after revoke/erase) and disk-budget eviction (not
    /// sensitive — count only).
    #[doc(hidden)]
    pub fn work_context_raw_blob_count(&self) -> Result<i64, StorageError> {
        let read = self.conn.read_lock();
        read.conn()
            .query_row("SELECT COUNT(*) FROM work_context_raw_blobs", [], |r| {
                r.get(0)
            })
            .map_err(StorageError::from)
    }
}

#[async_trait]
impl WorkContextStorePort for SqliteStorage {
    async fn begin_access_epoch(
        &self,
        install_id: &str,
        account_subject_ref: &str,
        now: DateTime<Utc>,
    ) -> Result<i64, CoreError> {
        let install = install_id.to_string();
        let account = account_subject_ref.to_string();
        let epoch = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction().map_err(StorageError::from)?;
                // Revision I2: the store issues the epoch. 1 initially, then +1
                // on each re-authorization.
                tx.execute(
                    "INSERT INTO work_context_access_epochs
                        (install_id, account_subject_ref, current_epoch, updated_at)
                     VALUES (?1, ?2, 1, ?3)
                     ON CONFLICT(install_id, account_subject_ref)
                     DO UPDATE SET current_epoch = current_epoch + 1, updated_at = excluded.updated_at",
                    params![install, account, ts(&now)],
                )
                .map_err(StorageError::from)?;
                let epoch: i64 = tx
                    .query_row(
                        "SELECT current_epoch FROM work_context_access_epochs
                          WHERE install_id = ?1 AND account_subject_ref = ?2",
                        params![install, account],
                        |r| r.get(0),
                    )
                    .map_err(StorageError::from)?;
                tx.commit().map_err(StorageError::from)?;
                Ok(epoch)
            })
            .await?;
        Ok(epoch)
    }

    async fn get_cursor(
        &self,
        install_id: &str,
        account_subject_ref: &str,
    ) -> Result<Option<CursorState>, CoreError> {
        let install = install_id.to_string();
        let account = account_subject_ref.to_string();
        let out = self
            .with_conn_read(move |conn| {
                let row: Option<(Option<String>, i64, Option<String>)> = conn
                    .query_row(
                        "SELECT cursor, access_epoch_id, last_ingested_at
                           FROM work_context_cursors
                          WHERE install_id = ?1 AND account_subject_ref = ?2",
                        params![install, account],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(StorageError::from)?;
                match row {
                    Some((cursor, epoch, last)) => Ok(Some(CursorState {
                        install_id: install,
                        account_subject_ref: account,
                        cursor,
                        access_epoch_id: epoch,
                        last_ingested_at: parse_ts_opt(last)?,
                    })),
                    None => Ok(None),
                }
            })
            .await?;
        Ok(out)
    }

    async fn commit_page(&self, request: CommitPageRequest) -> Result<CommitOutcome, CoreError> {
        // Move the master key used to derive the raw-plane AEAD subkey into the
        // closure (revision I1). None for a non-encrypted handle — even with
        // consent, raw is never left in plaintext.
        let raw_key = self.work_context_raw_key.clone();
        let outcome: Option<CommitOutcome> = self
            .with_conn_mut(move |conn| {
                let write_ctx = WriteCtx {
                    raw_key: raw_key.as_ref(),
                    contents: &request.contents,
                    now: request.now,
                };
                let tx = conn.transaction().map_err(StorageError::from)?;

                // 1. Epoch validation — discard if the page's epoch differs from
                //    the account's current epoch.
                let current_epoch: Option<i64> = tx
                    .query_row(
                        "SELECT current_epoch FROM work_context_access_epochs
                          WHERE install_id = ?1 AND account_subject_ref = ?2",
                        params![request.install_id, request.account_subject_ref],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(StorageError::from)?;
                if let Some(cur) = current_epoch {
                    if cur != request.access_epoch_id {
                        return Ok(Some(CommitOutcome::EpochMismatch { current_epoch: cur }));
                    }
                }

                // 2. Cursor CAS — commit nothing if the stored value differs from
                //    the value read at the start.
                let stored_cursor: Option<(Option<String>, i64)> = tx
                    .query_row(
                        "SELECT cursor, cursor_revision FROM work_context_cursors
                          WHERE install_id = ?1 AND account_subject_ref = ?2",
                        params![request.install_id, request.account_subject_ref],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()
                    .map_err(StorageError::from)?;
                let (current_cursor, cursor_rev) = match &stored_cursor {
                    Some((c, rev)) => (c.clone(), *rev),
                    None => (None, 0),
                };
                if current_cursor != request.cursor.expected_cursor {
                    return Ok(Some(CommitOutcome::CursorConflict));
                }

                // 3. Apply the per-record merge decision.
                let mut results = Vec::with_capacity(request.envelopes.len());
                for env in &request.envelopes {
                    // I2: every record on a page must carry that page's epoch.
                    // Records with a mismatched epoch are ignored — this prevents
                    // writing to the wrong (source_object_key, epoch) partition
                    // and bypassing suppression/quarantine. A normal connector
                    // always stamps the page epoch in record_to_envelope.
                    if env.access_epoch_id != request.access_epoch_id {
                        continue;
                    }
                    let existing = load_existing(&tx, &env.source_object_key, env.access_epoch_id)?;
                    let incoming = MergeIncoming {
                        revision_fingerprint: env.revision_fingerprint.clone(),
                        revision_model: env.revision_model,
                        source_order: env.source_order,
                        content_hash: env.content_hash.clone(),
                        lifecycle: env.lifecycle,
                        // Whether a connector declares undelete support is the
                        // descriptor's concern, so here we trust only the arriving
                        // envelope's lifecycle. Conservatively false.
                        provider_supports_undelete: false,
                        access_denied: matches!(env.lifecycle, Lifecycle::AccessRevoked)
                            || env
                                .access_snapshot
                                .as_ref()
                                .map(|s| s.is_denied())
                                .unwrap_or(false),
                    };
                    let decision = merge_decide(existing.as_ref(), &incoming);
                    apply_decision(&tx, env, &decision, &write_ctx)?;
                    results.push(decision);
                }

                // 4. Advance the cursor — CAS passed, so bump the revision and write.
                tx.execute(
                    "INSERT INTO work_context_cursors
                        (install_id, account_subject_ref, cursor, access_epoch_id,
                         cursor_revision, last_ingested_at, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
                     ON CONFLICT(install_id, account_subject_ref)
                     DO UPDATE SET cursor = excluded.cursor,
                                   access_epoch_id = excluded.access_epoch_id,
                                   cursor_revision = ?5,
                                   last_ingested_at = excluded.last_ingested_at,
                                   updated_at = excluded.updated_at",
                    params![
                        request.install_id,
                        request.account_subject_ref,
                        request.cursor.next_cursor,
                        request.access_epoch_id,
                        cursor_rev + 1,
                        ts(&request.now),
                    ],
                )
                .map_err(StorageError::from)?;

                tx.commit().map_err(StorageError::from)?;
                Ok(Some(CommitOutcome::Committed { results }))
            })
            .await?;
        // If skipped mid-erase it is None — nothing was committed, so safely
        // return CursorConflict.
        Ok(outcome.unwrap_or(CommitOutcome::CursorConflict))
    }

    async fn get_envelope(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
    ) -> Result<Option<WorkContextEnvelope>, CoreError> {
        let key = source_object_key.to_string();
        let out = self
            .with_conn_read(move |conn| {
                conn.query_row(
                    "SELECT * FROM work_context_envelopes
                      WHERE source_object_key = ?1 AND access_epoch_id = ?2
                      ORDER BY ingested_at DESC LIMIT 1",
                    params![key, access_epoch_id],
                    row_to_envelope,
                )
                .optional()
                .map_err(StorageError::from)
            })
            .await?;
        Ok(out)
    }

    async fn list_projectable(&self, limit: u32) -> Result<Vec<WorkContextEnvelope>, CoreError> {
        let out = self
            .with_conn_read(move |conn| {
                // Envelopes with an unknown kind, a terminal lifecycle, or expired
                // (pre-sweep) are excluded (§2, §6), and objects under quarantine
                // (a conflict row exists) are excluded via anti-join — during
                // conflict quarantine no winner is exposed (§6 rule 7, Invariant 5).
                // Expiry is compared normalized via datetime() (avoiding the
                // 'T'/offset fragility of RFC3339 string lexical comparison). This
                // filter closes the window that expire_planes has not yet flipped.
                let mut stmt = conn
                    .prepare(
                        "SELECT * FROM work_context_envelopes AS e
                          WHERE e.lifecycle = 'active' AND e.kind != 'unknown'
                            AND datetime(e.expires_at) > datetime('now')
                            AND NOT EXISTS (
                                SELECT 1 FROM work_context_conflicts AS c
                                 WHERE c.source_object_key = e.source_object_key
                                   AND c.access_epoch_id = e.access_epoch_id)
                          ORDER BY e.ingested_at DESC LIMIT ?1",
                    )
                    .map_err(StorageError::from)?;
                let rows = stmt
                    .query_map(params![limit], row_to_envelope)
                    .map_err(StorageError::from)?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r.map_err(StorageError::from)?);
                }
                Ok(out)
            })
            .await?;
        Ok(out)
    }

    async fn list_work_context_timeline(
        &self,
        limit: u32,
    ) -> Result<Vec<TimelineEvidenceItem>, CoreError> {
        let out = self
            .with_conn_read(move |conn| query_timeline(conn, limit))
            .await?;
        Ok(out)
    }

    async fn read_projection(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
    ) -> Result<Option<ProjectionContent>, CoreError> {
        let key = source_object_key.to_string();
        let out = self
            .with_conn_read(move |conn| query_projection(conn, &key, access_epoch_id))
            .await?;
        Ok(out)
    }

    async fn mark_lifecycle(
        &self,
        source_object_key: &str,
        access_epoch_id: i64,
        lifecycle: Lifecycle,
        now: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let key = source_object_key.to_string();
        self.with_conn_mut(move |conn| {
            let tx = conn.transaction().map_err(StorageError::from)?;
            deactivate_prior(&tx, &key, access_epoch_id, lifecycle)?;
            if lifecycle.is_terminal() {
                // #8589 (§8/§12): on a terminal transition, clear the content
                // plane immediately.
                clear_content_for_object(&tx, &key, access_epoch_id)?;
                write_lifecycle_tombstone(&tx, &key, access_epoch_id, lifecycle, &now)?;
            }
            tx.commit().map_err(StorageError::from)?;
            Ok(())
        })
        .await?;
        Ok(())
    }

    async fn revoke_account(
        &self,
        install_id: &str,
        account_subject_ref: &str,
        now: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let install = install_id.to_string();
        let account = account_subject_ref.to_string();
        self.with_conn_mut(move |conn| {
            let tx = conn.transaction().map_err(StorageError::from)?;

            // 1. Write the suppression tombstone **first** (§12). Without it, an
            //    at-least-once replay page would resurrect the erased content
            //    (BLOCKING 2, rejected alternative F). It is content-free and keyed
            //    only by the HMAC source_object_key. The active envelopes must be
            //    SELECTed before they are deactivated so the target set is captured.
            let tomb_expires = now
                + chrono::Duration::seconds(
                    super::super::migration::v51_work_context::clamp_tombstone_ttl_secs(None),
                );
            tx.execute(
                "INSERT OR IGNORE INTO work_context_tombstones
                    (tombstone_id, source_object_key, access_epoch_id, revision_fingerprint,
                     lifecycle, source_order, deleted_at, expires_at)
                 SELECT DISTINCT
                     source_object_key || '_' || access_epoch_id || '_access_revoked',
                     source_object_key, access_epoch_id,
                     'lifecycle_access_revoked', 'access_revoked', NULL, ?3, ?4
                   FROM work_context_envelopes
                  WHERE install_id = ?1 AND account_subject_ref = ?2 AND lifecycle = 'active'",
                params![install, account, ts(&now), ts(&tomb_expires)],
            )
            .map_err(StorageError::from)?;

            // 2. Clear the content plane (raw/projection) (§8/§12). Children first —
            //    foreign_keys is off, so delete in explicit order.
            tx.execute(
                "DELETE FROM work_context_raw_blobs
                  WHERE install_id = ?1 AND source_object_key IN (
                      SELECT source_object_key FROM work_context_envelopes
                       WHERE install_id = ?1 AND account_subject_ref = ?2)",
                params![install, account],
            )
            .map_err(StorageError::from)?;
            tx.execute(
                "DELETE FROM work_context_projections
                  WHERE source_object_key IN (
                      SELECT source_object_key FROM work_context_envelopes
                       WHERE install_id = ?1 AND account_subject_ref = ?2)",
                params![install, account],
            )
            .map_err(StorageError::from)?;

            // 3. Terminate the envelope as access_revoked and **erase the remote
            //    identifiers not needed for suppression** (§12: "account/remote
            //    identifiers not needed for bounded suppression"). Clear the raw
            //    remote_id, content_hash, relations, timestamps, and snapshots —
            //    suppression needs only the tombstone (source_object_key). NOT NULL
            //    columns (remote_id/content_hash) become the empty string, nullable
            //    ones become NULL. source_object_key/access_epoch_id/
            //    revision_fingerprint are kept because they are needed for
            //    suppression matching. load_existing checks the tombstone first, so
            //    the emptied envelope fields do not affect the merge.
            tx.execute(
                "UPDATE work_context_envelopes
                    SET lifecycle = 'access_revoked',
                        remote_id = '', remote_revision = NULL, etag = NULL,
                        content_hash = '', relations_json = '[]',
                        occurred_at = NULL, source_updated_at = NULL,
                        access_snapshot_json = NULL, consent_snapshot_json = NULL,
                        projection_ref = NULL, raw_blob_ref = NULL
                  WHERE install_id = ?1 AND account_subject_ref = ?2 AND lifecycle = 'active'",
                params![install, account],
            )
            .map_err(StorageError::from)?;

            // 4. Increment the access epoch (Frozen Invariant 4: "access revoke
            //    requires a new access epoch"). A stale in-flight page arriving
            //    with the pre-revoke epoch is discarded as EpochMismatch at
            //    commit_page's epoch gate. If there is no account row (revoke
            //    before ingest), this is a no-op.
            tx.execute(
                "UPDATE work_context_access_epochs
                    SET current_epoch = current_epoch + 1, updated_at = ?3
                  WHERE install_id = ?1 AND account_subject_ref = ?2",
                params![install, account, ts(&now)],
            )
            .map_err(StorageError::from)?;

            // 5. Delete the cursor to make the next sync fail-closed.
            tx.execute(
                "DELETE FROM work_context_cursors
                  WHERE install_id = ?1 AND account_subject_ref = ?2",
                params![install, account],
            )
            .map_err(StorageError::from)?;
            tx.commit().map_err(StorageError::from)?;
            Ok(())
        })
        .await?;
        Ok(())
    }

    async fn expire_planes(&self, now: DateTime<Utc>) -> Result<u64, CoreError> {
        // Expiry is decided with effective_now = max(now, latest local anchor)
        // (revision I5). Even if the clock moves backward, data is not expired
        // any later.
        let deleted = self
            .with_conn_mut(move |conn| {
                let tx = conn.transaction().map_err(StorageError::from)?;

                // M-1: if the anti-rollback anchor lives only on the cursor, the
                // anchor disappears after revoke/erase empties the cursor. Also
                // consult envelope.ingested_at and tombstone.deleted_at to keep
                // the latest local time.
                let anchor: Option<String> = tx
                    .query_row(
                        "SELECT MAX(v) FROM (
                             SELECT MAX(last_ingested_at) AS v FROM work_context_cursors
                             UNION ALL SELECT MAX(ingested_at) FROM work_context_envelopes
                             UNION ALL SELECT MAX(deleted_at) FROM work_context_tombstones
                         )",
                        [],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(StorageError::from)?
                    .flatten();
                let effective = effective_now(now, anchor.map(|v| parse_ts(&v)).transpose()?);
                let eff = ts(&effective);

                // B1: an active envelope with neither a projection nor a confirmed
                // reference terminated nothing even past its cap, so it lingered
                // indefinitely. Flip active envelopes past the cap to
                // retention_expired and leave only a content-free tombstone — this
                // must run **before** the terminal DELETE so the flipped rows are
                // then deleted. The tombstone has its own independent cap
                // (90–365 days).
                let tomb_expires = effective
                    + chrono::Duration::seconds(
                        super::super::migration::v51_work_context::clamp_tombstone_ttl_secs(None),
                    );
                tx.execute(
                    "INSERT OR IGNORE INTO work_context_tombstones
                        (tombstone_id, source_object_key, access_epoch_id, revision_fingerprint,
                         lifecycle, source_order, deleted_at, expires_at)
                     SELECT DISTINCT
                         source_object_key || '_' || access_epoch_id || '_retention_expired',
                         source_object_key, access_epoch_id,
                         'lifecycle_retention_expired', 'retention_expired', NULL, ?1, ?2
                       FROM work_context_envelopes
                      WHERE lifecycle = 'active' AND expires_at < ?1",
                    params![eff, ts(&tomb_expires)],
                )
                .map_err(StorageError::from)?;
                tx.execute(
                    "UPDATE work_context_envelopes SET lifecycle = 'retention_expired'
                      WHERE lifecycle = 'active' AND expires_at < ?1",
                    params![eff],
                )
                .map_err(StorageError::from)?;

                let mut total = 0u64;
                for table in [
                    "work_context_raw_blobs",
                    "work_context_projections",
                    "work_context_conflicts",
                    "work_context_tombstones",
                ] {
                    let n = tx
                        .execute(
                            &format!("DELETE FROM {table} WHERE expires_at < ?1"),
                            params![eff],
                        )
                        .map_err(StorageError::from)?;
                    total += n as u64;
                }
                // Envelopes are deleted only when expired after being marked
                // terminal (including the flip above).
                let n = tx
                    .execute(
                        "DELETE FROM work_context_envelopes
                          WHERE expires_at < ?1 AND lifecycle != 'active'",
                        params![eff],
                    )
                    .map_err(StorageError::from)?;
                total += n as u64;

                // #8589 (§7 compaction): after TTL-expiry deletion, evict the
                // remaining raw below the disk budget. Delete oldest-first to
                // preserve the newest raw.
                total += enforce_raw_budget(&tx, RAW_PLANE_DISK_BUDGET_BYTES)?;

                tx.commit().map_err(StorageError::from)?;
                Ok(total)
            })
            .await?;
        Ok(deleted)
    }
}

/// Updates the store state according to the merge decision.
fn apply_decision(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    decision: &MergeOutcome,
    ctx: &WriteCtx,
) -> Result<(), StorageError> {
    match decision {
        // Replay — the same fingerprint is already stored, so INSERT OR IGNORE
        // ignores it. #8589: the content (projection/raw) was already written on
        // the first Accepted too, so it is left untouched — a replay page does
        // not erase or rewrite content.
        MergeOutcome::Duplicate => {
            insert_envelope(tx, env)?;
        }
        // A stale revision neither supersedes nor resurrects — it is not stored
        // (§6 rule 4). Inserting it would enter as active and pollute the latest
        // (violating delivery-order independence).
        MergeOutcome::Stale => {}
        // A higher revision supersedes the existing active projection.
        MergeOutcome::Accepted => {
            // NOTE: marking the prior (superseded) active envelope as `Deleted`
            // here is strictly a "supersede", not a "delete". Since there is no
            // dedicated `superseded` lifecycle (it would require extending the
            // enum and CHECK constraint), `Deleted` is reused as a harmless
            // inactive marker that is always followed by the re-inserted new
            // active. load_existing reads tombstone-first then active, so the
            // always-following new active is the current state and this `Deleted`
            // history row does not participate in the merge (reviewer-confirmed:
            // merge inert).
            deactivate_prior(
                tx,
                &env.source_object_key,
                env.access_epoch_id,
                Lifecycle::Deleted,
            )?;
            let mut active = env.clone();
            active.lifecycle = Lifecycle::Active;
            // #8589 (§9 step 4): since FK is active, insert the parent (envelope)
            // **first**, then write the children (projection/raw), and finally
            // stamp the provenance backlink.
            insert_envelope(tx, &active)?;
            let (projection_ref, raw_blob_ref) = write_accepted_content(tx, &active, ctx)?;
            if projection_ref.is_some() || raw_blob_ref.is_some() {
                super::work_context_writers::backlink_content_refs(
                    tx,
                    &active.envelope_id,
                    projection_ref.as_deref(),
                    raw_blob_ref.as_deref(),
                )?;
            }
        }
        // Access revocation erases content and leaves a tombstone.
        MergeOutcome::AccessRevoked => {
            deactivate_prior(
                tx,
                &env.source_object_key,
                env.access_epoch_id,
                Lifecycle::AccessRevoked,
            )?;
            // #8589 (§8/§12): per-page access denial also clears the content plane
            // immediately. Deleting the raw row destroys the key_salt, so that is
            // itself a crypto-shred.
            clear_content_for_object(tx, &env.source_object_key, env.access_epoch_id)?;
            let mut t = env.clone();
            t.lifecycle = Lifecycle::AccessRevoked;
            write_tombstone(tx, &t, Lifecycle::AccessRevoked)?;
        }
        // A delete/retention tombstone suppresses an equal or lower active revision.
        MergeOutcome::Suppressed => {
            let lc = if env.lifecycle.is_terminal() {
                env.lifecycle
            } else {
                Lifecycle::Deleted
            };
            deactivate_prior(tx, &env.source_object_key, env.access_epoch_id, lc)?;
            // #8589: delete/retention-expiry suppression erases content
            // availability (§6/§7).
            clear_content_for_object(tx, &env.source_object_key, env.access_epoch_id)?;
            write_tombstone(tx, env, lc)?;
        }
        // Same revision, different content — record as a conflict but do not
        // expose a winner.
        MergeOutcome::RevisionConflict => {
            quarantine_prior_and_incoming(tx, env, "revision_conflict")?;
        }
        // Incomparable — quarantine under a deterministic conflict ID; project
        // no winner.
        MergeOutcome::Incomparable { conflict_id } => {
            quarantine_prior_and_incoming(tx, env, conflict_id)?;
        }
    }
    Ok(())
}

/// Quarantines a conflict/incomparable case: deactivates the existing active
/// envelope (BLOCKING 1) and leaves a deterministic conflict row (§6 rule 7,
/// Frozen Invariant 5).
///
/// If the prior active envelope is not deactivated, its revision remains
/// projectable and **delivery order decides the winner** (v1→v2 and v2→v1
/// expose different content). ADR-030 §6 rule 7 requires that during quarantine
/// **no winner** is exposed to search, suggestions, tasks, or the graph. The
/// conflict anti-join in `list_projectable` is the primary guarantee, and this
/// deactivation is belt-and-suspenders. The arriving conflict record itself is
/// not inserted as an envelope — its metadata is preserved only in the conflict
/// row.
fn quarantine_prior_and_incoming(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    conflict_id: &str,
) -> Result<(), StorageError> {
    deactivate_prior(
        tx,
        &env.source_object_key,
        env.access_epoch_id,
        Lifecycle::Deleted,
    )?;
    // #8589 (§6 rule 7): since no winner may be exposed during quarantine, the
    // content of the prior active revision is erased too — if a projection
    // lingers beside a deactivated envelope, the read paths
    // (timeline/read_projection) filter it out but the mere lingering is risky.
    clear_content_for_object(tx, &env.source_object_key, env.access_epoch_id)?;
    quarantine_conflict(tx, env, conflict_id)?;
    Ok(())
}

/// SQLite row → `WorkContextEnvelope`.
fn row_to_envelope(row: &rusqlite::Row) -> rusqlite::Result<WorkContextEnvelope> {
    use maekon_core::models::work_context::{
        AccessSnapshot, ConsentSnapshot, DataClassification, RelationRef, SourceObjectIdentity,
        WorkContextKind,
    };

    let relations_json: String = row.get("relations_json")?;
    let relations: Vec<RelationRef> = serde_json::from_str(&relations_json).unwrap_or_default();
    let access_json: Option<String> = row.get("access_snapshot_json")?;
    let access_snapshot: Option<AccessSnapshot> =
        access_json.and_then(|s| serde_json::from_str(&s).ok());
    let consent_json: Option<String> = row.get("consent_snapshot_json")?;
    let consent_snapshot: Option<ConsentSnapshot> =
        consent_json.and_then(|s| serde_json::from_str(&s).ok());

    let parse = |s: String| {
        DateTime::parse_from_rfc3339(&s)
            .map(|d| d.with_timezone(&Utc))
            .ok()
    };
    let occurred_at: Option<String> = row.get("occurred_at")?;
    let source_updated_at: Option<String> = row.get("source_updated_at")?;
    let observed_at: String = row.get("observed_at")?;
    let ingested_at: String = row.get("ingested_at")?;

    let revision_model: String = row.get("revision_model")?;
    let kind: String = row.get("kind")?;
    let classification: String = row.get("classification")?;
    let lifecycle: String = row.get("lifecycle")?;

    Ok(WorkContextEnvelope {
        envelope_id: row.get("envelope_id")?,
        schema_version: row.get("schema_version")?,
        access_epoch_id: row.get("access_epoch_id")?,
        identity: SourceObjectIdentity {
            extension_id: row.get("extension_id")?,
            install_id: row.get("install_id")?,
            account_subject_ref: row.get("account_subject_ref")?,
            remote_type: row.get("remote_type")?,
            remote_id: row.get("remote_id")?,
        },
        source_object_key: row.get("source_object_key")?,
        revision_model: RevisionModel::from_str_lossy(&revision_model)
            .unwrap_or(RevisionModel::Opaque),
        remote_revision: row.get("remote_revision")?,
        etag: row.get("etag")?,
        source_order: row.get("source_order")?,
        content_hash: row.get("content_hash")?,
        revision_fingerprint: row.get("revision_fingerprint")?,
        kind: WorkContextKind::from_str_lossy(&kind),
        classification: DataClassification::from_str_lossy(&classification),
        retention_class: row.get("retention_class")?,
        occurred_at: occurred_at.and_then(parse),
        source_updated_at: source_updated_at.and_then(parse),
        observed_at: parse(observed_at).unwrap_or_else(Utc::now),
        ingested_at: parse(ingested_at).unwrap_or_else(Utc::now),
        relations,
        access_snapshot,
        consent_snapshot,
        ingest_run_id: row.get("ingest_run_id")?,
        prior_envelope_id: row.get("prior_envelope_id")?,
        source_cursor_digest: row.get("source_cursor_digest")?,
        projection_ref: row.get("projection_ref")?,
        raw_blob_ref: row.get("raw_blob_ref")?,
        lifecycle: Lifecycle::from_str_lossy(&lifecycle).unwrap_or(Lifecycle::Active),
    })
}
