//! Work-context projection/raw writer, timeline queries, retention budget (ADR-030 §7/§8/§11, #8589).
//!
//! #8587 recorded only envelope/tombstone. This module fills §7's other two
//! content planes:
//!
//! - **projection**: sanitized title/summary. Since the DB is already encrypted
//!   at rest (SQLCipher), this is stored as bounded plaintext with no extra app-level encryption.
//! - **raw blob**: the original payload. Written AEAD-encrypted **only with
//!   explicit consent** (revision I1 HKDF subkey). If consent or the master key
//!   is absent, it is not written — memory-only by default.
//!
//! Both planes are written only for objects judged `Accepted`, and are removed
//! immediately by `clear_content_for_object` on close/revocation/supersede. Since
//! the `foreign_keys` PRAGMA is OFF, the children (raw/projection) are deleted explicitly first.

use chrono::{DateTime, Utc};
use maekon_core::models::work_context::{
    DataClassification, Lifecycle, WorkContextEnvelope, WorkContextKind,
};
use maekon_core::models::work_context_projection::{
    ProjectionContent, SourceFamily, TimelineEvidenceItem,
};
use maekon_core::ports::work_context::{CommitContent, RawPayloadInput};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use crate::migration::v51_work_context::{clamp_raw_ttl_secs, PROJECTION_PLANE_DEFAULT_TTL_SECS};

/// On-disk budget for the raw plane (bytes). The overflow is evicted oldest-first (§7 compaction).
///
/// Raw is already bounded by a 24-hour (hard max 7-day) TTL, but a large influx
/// in a short window can grow the disk unboundedly on TTL alone. This budget pins that ceiling.
pub(super) const RAW_PLANE_DISK_BUDGET_BYTES: i64 = 32 * 1024 * 1024;

fn ts(value: &DateTime<Utc>) -> String {
    value.to_rfc3339()
}

/// Context needed to write projection/raw during a commit.
pub(super) struct WriteCtx<'a> {
    /// Master key for deriving the raw-plane AEAD subkey. If `None` (non-encrypting
    /// handle), raw is not written even with consent — a fail-closed that prevents plaintext persistence.
    pub raw_key: Option<&'a EncryptionKey>,
    /// Per-envelope content. Matched by `source_object_key`.
    pub contents: &'a [CommitContent],
    pub now: DateTime<Utc>,
}

impl<'a> WriteCtx<'a> {
    fn content_for(&self, source_object_key: &str) -> Option<&'a CommitContent> {
        self.contents
            .iter()
            .find(|c| c.source_object_key == source_object_key)
    }
}

/// Backlinks the projection/raw references onto the just-inserted envelope (provenance).
///
/// **Must be called after the envelope is inserted** — projection/raw reference
/// envelope_id (in an FK-enabled environment), so follow the order parent
/// (envelope) → child (projection/raw) → backlink UPDATE.
pub(super) fn backlink_content_refs(
    tx: &Transaction,
    envelope_id: &str,
    projection_ref: Option<&str>,
    raw_blob_ref: Option<&str>,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE work_context_envelopes
            SET projection_ref = ?2, raw_blob_ref = ?3
          WHERE envelope_id = ?1",
        params![envelope_id, projection_ref, raw_blob_ref],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

/// Writes the projection/raw for an accepted envelope (§9 step 4).
///
/// To make supersede/re-acceptance idempotent, it **clears the existing content
/// first** and then writes the new content. With no content (the orchestrator
/// path) it only clears and returns — a stale projection does not linger under a new revision.
///
/// **The envelope must already be inserted before this is called** — because the
/// projection references envelope_id (FK-enabled environment, parent-first). The return value is used for backlinking.
pub(super) fn write_accepted_content(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    ctx: &WriteCtx,
) -> Result<(Option<String>, Option<String>), StorageError> {
    // Supersede/re-acceptance idempotency: remove this (object, epoch)'s existing content first.
    clear_content_for_object(tx, &env.source_object_key, env.access_epoch_id)?;

    let Some(content) = ctx.content_for(&env.source_object_key) else {
        return Ok((None, None));
    };

    let projection_ref = match &content.projection {
        Some(p) => write_projection(tx, env, p, &ctx.now)?,
        None => None,
    };
    let raw_blob_ref = match &content.raw_payload {
        Some(raw) => write_raw_blob(tx, env, raw, ctx.raw_key, &ctx.now)?,
        None => None,
    };
    Ok((projection_ref, raw_blob_ref))
}

/// Writes the sanitized title/summary to the projection plane (§7).
///
/// Because projection_id is deterministic per (object, epoch), `INSERT OR REPLACE`
/// handles supersede naturally. Bounded truncation (`bounded`) prevents copying the original.
fn write_projection(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    projection: &ProjectionContent,
    now: &DateTime<Utc>,
) -> Result<Option<String>, StorageError> {
    let bounded = projection.clone().bounded();
    if bounded.is_empty() {
        return Ok(None);
    }
    let projection_id = format!("wctxproj_{}_{}", env.source_object_key, env.access_epoch_id);
    let expires = *now + chrono::Duration::seconds(PROJECTION_PLANE_DEFAULT_TTL_SECS);
    tx.execute(
        "INSERT OR REPLACE INTO work_context_projections
            (projection_id, envelope_id, source_object_key, access_epoch_id,
             sanitized_title, sanitized_summary, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            projection_id,
            env.envelope_id,
            env.source_object_key,
            env.access_epoch_id,
            bounded.sanitized_title,
            bounded.sanitized_summary,
            ts(now),
            ts(&expires),
        ],
    )
    .map_err(StorageError::from)?;
    Ok(Some(projection_id))
}

/// Writes the original payload to the raw plane, AEAD-encrypted (§7, revision I1).
///
/// **Nothing is written without consent** (memory-only by default). Even with
/// consent, if the master key is absent (non-encrypting handle) it fails closed
/// and skips, so as not to leave plaintext. The TTL is narrowed to the 7-day hard max by `clamp_raw_ttl_secs`.
fn write_raw_blob(
    tx: &Transaction,
    env: &WorkContextEnvelope,
    raw: &RawPayloadInput,
    raw_key: Option<&EncryptionKey>,
    now: &DateTime<Utc>,
) -> Result<Option<String>, StorageError> {
    if !raw.consent_present {
        // Absence of explicit consent = do not leave the original in the ledger (§7 default).
        return Ok(None);
    }
    let Some(key) = raw_key else {
        // Consent is present but there is no master key to encrypt with — refuse
        // plaintext persistence and keep it memory-only (fail-closed). Does not happen in production.
        tracing::warn!(
            "work-context raw payload consented but no master key available; \
             keeping it memory-only (no plaintext persisted)"
        );
        return Ok(None);
    };

    let bundle = key.encrypt_raw_plane(
        &raw.plaintext,
        &env.identity.install_id,
        &env.identity.account_subject_ref,
    )?;
    let raw_blob_ref = format!("wctxraw_{}_{}", env.source_object_key, env.access_epoch_id);
    let expires = *now + chrono::Duration::seconds(clamp_raw_ttl_secs(raw.requested_ttl_secs));
    tx.execute(
        "INSERT OR REPLACE INTO work_context_raw_blobs
            (raw_blob_ref, source_object_key, access_epoch_id, install_id,
             key_salt, nonce, ciphertext, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            raw_blob_ref,
            env.source_object_key,
            env.access_epoch_id,
            env.identity.install_id,
            bundle.key_salt,
            bundle.nonce,
            bundle.ciphertext,
            ts(now),
            ts(&expires),
        ],
    )
    .map_err(StorageError::from)?;
    Ok(Some(raw_blob_ref))
}

/// Immediately removes one (object, epoch)'s raw + projection content (§8/§12).
///
/// Called on close (deletion/revocation/retention-expiry) or supersede. Deleting
/// the raw row destroys the `key_salt`, so it is a crypto-shred in itself. It
/// deletes only the children (raw/projection) and does not touch
/// envelope/tombstone — suppression is the tombstone's job.
pub(super) fn clear_content_for_object(
    tx: &Transaction,
    source_object_key: &str,
    access_epoch_id: i64,
) -> Result<(), StorageError> {
    tx.execute(
        "DELETE FROM work_context_raw_blobs
          WHERE source_object_key = ?1 AND access_epoch_id = ?2",
        params![source_object_key, access_epoch_id],
    )
    .map_err(StorageError::from)?;
    tx.execute(
        "DELETE FROM work_context_projections
          WHERE source_object_key = ?1 AND access_epoch_id = ?2",
        params![source_object_key, access_epoch_id],
    )
    .map_err(StorageError::from)?;
    Ok(())
}

/// Evicts the raw plane down to the disk budget (§7 compaction).
///
/// When total ciphertext bytes exceed the budget, it deletes **oldest-first**
/// (created_at ASC) — preserving the newest raw preferentially. Returns the
/// number of deleted rows. Call it after TTL-expiry deletion so the budget applies only among what remains.
pub(super) fn enforce_raw_budget(tx: &Transaction, budget_bytes: i64) -> Result<u64, StorageError> {
    let total: i64 = tx
        .query_row(
            "SELECT COALESCE(SUM(length(ciphertext)), 0) FROM work_context_raw_blobs",
            [],
            |r| r.get(0),
        )
        .map_err(StorageError::from)?;
    if total <= budget_bytes {
        return Ok(0);
    }

    // Accumulate from newest and collect everything past the point that exceeds the budget (the older ones) as eviction targets.
    let mut stmt = tx
        .prepare(
            "SELECT raw_blob_ref, length(ciphertext) FROM work_context_raw_blobs
              ORDER BY created_at DESC, raw_blob_ref DESC",
        )
        .map_err(StorageError::from)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map_err(StorageError::from)?;

    let mut kept: i64 = 0;
    let mut to_delete: Vec<String> = Vec::new();
    for row in rows {
        let (rref, len) = row.map_err(StorageError::from)?;
        if kept + len <= budget_bytes {
            kept += len;
        } else {
            to_delete.push(rref);
        }
    }
    drop(stmt);

    let mut deleted = 0u64;
    for rref in to_delete {
        deleted += tx
            .execute(
                "DELETE FROM work_context_raw_blobs WHERE raw_blob_ref = ?1",
                params![rref],
            )
            .map_err(StorageError::from)? as u64;
    }
    Ok(deleted)
}

// ---------------------------------------------------------------------------
// Read path (§8: a stored snapshot cannot authorize a live read — the caller
// re-evaluates live consent/access before display/suggestion. Here we structurally
// exclude closed/expired/quarantined items so nothing leaks even in the not-yet-cleared residual window.)
// ---------------------------------------------------------------------------

fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .ok()
}

/// Queries external work-context timeline items (§11).
///
/// Returns only sanitized projections joined to active, non-expired, non-quarantined
/// envelopes, under the `work_context` label. Expiry is judged by normalized
/// `datetime()` comparison (avoiding string lexical fragility).
pub(super) fn query_timeline(
    conn: &Connection,
    limit: u32,
) -> Result<Vec<TimelineEvidenceItem>, StorageError> {
    let mut stmt = conn
        .prepare(
            "SELECT e.envelope_id, e.kind, e.classification, e.lifecycle,
                    e.occurred_at, e.observed_at,
                    p.sanitized_title, p.sanitized_summary
               FROM work_context_envelopes AS e
               LEFT JOIN work_context_projections AS p
                 ON p.source_object_key = e.source_object_key
                AND p.access_epoch_id = e.access_epoch_id
              WHERE e.lifecycle = 'active' AND e.kind != 'unknown'
                AND datetime(e.expires_at) > datetime('now')
                AND NOT EXISTS (
                    SELECT 1 FROM work_context_conflicts AS c
                     WHERE c.source_object_key = e.source_object_key
                       AND c.access_epoch_id = e.access_epoch_id)
              ORDER BY COALESCE(e.occurred_at, e.observed_at) DESC
              LIMIT ?1",
        )
        .map_err(StorageError::from)?;
    let rows = stmt
        .query_map(params![limit], |r| {
            let kind: String = r.get(1)?;
            let classification: String = r.get(2)?;
            let lifecycle: String = r.get(3)?;
            let occurred_at: Option<String> = r.get(4)?;
            let observed_at: String = r.get(5)?;
            let sanitized_title: Option<String> = r.get(6)?;
            let sanitized_summary: Option<String> = r.get(7)?;
            Ok(TimelineRow {
                envelope_id: r.get(0)?,
                kind,
                classification,
                lifecycle,
                occurred_at,
                observed_at,
                sanitized_title,
                sanitized_summary,
            })
        })
        .map_err(StorageError::from)?;

    let mut out = Vec::new();
    for row in rows {
        let row = row.map_err(StorageError::from)?;
        let observed = parse_ts(&row.observed_at).unwrap_or_else(Utc::now);
        let display_time = row
            .occurred_at
            .as_deref()
            .and_then(parse_ts)
            .unwrap_or(observed);
        out.push(TimelineEvidenceItem {
            source_family: SourceFamily::WorkContext,
            evidence_id: row.envelope_id,
            display_time,
            kind: Some(WorkContextKind::from_str_lossy(&row.kind)),
            classification: Some(DataClassification::from_str_lossy(&row.classification)),
            lifecycle: Lifecycle::from_str_lossy(&row.lifecycle),
            sanitized_title: row.sanitized_title,
            sanitized_summary: row.sanitized_summary,
        });
    }
    Ok(out)
}

struct TimelineRow {
    envelope_id: String,
    kind: String,
    classification: String,
    lifecycle: String,
    occurred_at: Option<String>,
    observed_at: String,
    sanitized_title: Option<String>,
    sanitized_summary: Option<String>,
}

/// Dereferences a source object's live sanitized projection (§8/§11).
///
/// Returns **only the projection of an active, non-expired, non-quarantined
/// envelope**. A closed (deleted/revoked/expired) or quarantined object is `None`
/// even before physical deletion — enforcing §8 (a stored snapshot cannot
/// authorize a read) as a join condition.
pub(super) fn query_projection(
    conn: &Connection,
    source_object_key: &str,
    access_epoch_id: i64,
) -> Result<Option<ProjectionContent>, StorageError> {
    conn.query_row(
        "SELECT p.sanitized_title, p.sanitized_summary
           FROM work_context_projections AS p
           JOIN work_context_envelopes AS e
             ON e.source_object_key = p.source_object_key
            AND e.access_epoch_id = p.access_epoch_id
          WHERE p.source_object_key = ?1 AND p.access_epoch_id = ?2
            AND e.lifecycle = 'active'
            AND datetime(e.expires_at) > datetime('now')
            AND NOT EXISTS (
                SELECT 1 FROM work_context_conflicts AS c
                 WHERE c.source_object_key = e.source_object_key
                   AND c.access_epoch_id = e.access_epoch_id)
          LIMIT 1",
        params![source_object_key, access_epoch_id],
        |r| {
            Ok(ProjectionContent {
                sanitized_title: r.get(0)?,
                sanitized_summary: r.get(1)?,
            })
        },
    )
    .optional()
    .map_err(StorageError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Minimal raw-plane schema (only the columns needed for the budget test).
    fn raw_table(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE work_context_raw_blobs (
                 raw_blob_ref TEXT PRIMARY KEY, source_object_key TEXT, access_epoch_id INTEGER,
                 install_id TEXT, key_salt BLOB, nonce BLOB, ciphertext BLOB,
                 created_at TEXT, expires_at TEXT);",
        )
        .unwrap();
    }

    fn insert_raw(conn: &Connection, rref: &str, created_at: &str, cipher_len: usize) {
        conn.execute(
            "INSERT INTO work_context_raw_blobs
                (raw_blob_ref, source_object_key, access_epoch_id, install_id,
                 key_salt, nonce, ciphertext, created_at, expires_at)
             VALUES (?1, 'sok', 1, 'inst', x'00', x'00', ?2, ?3, '2099-01-01T00:00:00Z')",
            params![rref, vec![0u8; cipher_len], created_at],
        )
        .unwrap();
    }

    #[test]
    fn budget_evicts_oldest_raw_first_and_keeps_newest() {
        let mut conn = Connection::open_in_memory().unwrap();
        raw_table(&conn);
        // 5 rows × 100 bytes = 500 bytes, created_at ascending (r1 oldest … r5 newest).
        for (i, r) in ["r1", "r2", "r3", "r4", "r5"].iter().enumerate() {
            insert_raw(&conn, r, &format!("2026-07-2{i}T00:00:00Z"), 100);
        }
        let tx = conn.transaction().unwrap();
        // Budget 250 bytes → keep from newest r5(100)+r4(100)=200, evict r3/r2/r1.
        let deleted = enforce_raw_budget(&tx, 250).unwrap();
        tx.commit().unwrap();
        assert_eq!(deleted, 3, "가장 오래된 3행이 축출되어야 한다");

        let remaining: Vec<String> = conn
            .prepare("SELECT raw_blob_ref FROM work_context_raw_blobs ORDER BY raw_blob_ref")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(remaining, vec!["r4".to_string(), "r5".to_string()]);
    }

    #[test]
    fn budget_is_a_noop_under_the_limit() {
        let mut conn = Connection::open_in_memory().unwrap();
        raw_table(&conn);
        insert_raw(&conn, "r1", "2026-07-20T00:00:00Z", 100);
        let tx = conn.transaction().unwrap();
        let deleted = enforce_raw_budget(&tx, RAW_PLANE_DISK_BUDGET_BYTES).unwrap();
        tx.commit().unwrap();
        assert_eq!(deleted, 0);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM work_context_raw_blobs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 1);
    }
}
