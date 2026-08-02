//! `VaultMirrorStatePort` impl on `SqliteStorage` (ADR-033 §1.4, V53).
//!
//! Async port; all blocking SQLite work is offloaded via `with_conn` /
//! `with_conn_read` (ADR-026 funnel). Keys are vault-relative file names
//! (never absolute paths). Writes are skipped during a consent-erase
//! (`with_conn` deletion-flag discipline; `vault_mirror_state` ∈ ALL_TABLES).
//!
//! The §6.4 last-cycle summary (#9522) rides the same table under the reserved
//! `::last_cycle` key, JSON-encoded in the value column — see `LAST_CYCLE_KEY`
//! for why it is a reserved row rather than its own table.

use std::collections::HashMap;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::memory_vault::VaultLastCycleSummary;
use maekon_core::ports::vault_mirror_state::VaultMirrorStatePort;
use rusqlite::params;
use tracing::warn;

use crate::error::StorageError;

use super::SqliteStorage;

/// Reserved key holding the JSON-encoded §6.4 last-cycle summary (#9522).
///
/// A reserved `::` key rather than its own table: the summary is exactly the
/// per-mirror, per-device, erase-with-the-files state this table already exists
/// for (the sibling `::active_root` row set the precedent), and reusing it makes
/// `ALL_TABLES` coverage structural — a new table would have to be remembered
/// into the §4 sweep, and a forgotten one would leave conflict names (and the
/// evidence a cycle ran at all) behind an Art.17 erase.
const LAST_CYCLE_KEY: &str = "::last_cycle";

#[async_trait]
impl VaultMirrorStatePort for SqliteStorage {
    async fn vault_hashes(&self) -> Result<HashMap<String, String>, CoreError> {
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare("SELECT file_name, content_hash FROM vault_mirror_state")
                .map_err(|e| StorageError::Internal(format!("prepare vault_hashes: {e}")))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                .map_err(|e| StorageError::Internal(format!("query vault_hashes: {e}")))?;
            let mut out = HashMap::new();
            for r in rows {
                let (name, hash) =
                    r.map_err(|e| StorageError::Internal(format!("read vault hash row: {e}")))?;
                out.insert(name, hash);
            }
            Ok(out)
        })
        .await
        .map_err(Into::into)
    }

    async fn upsert_vault_hash(
        &self,
        file_name: &str,
        content_hash: &str,
        updated_at: i64,
    ) -> Result<(), CoreError> {
        let file_name = file_name.to_string();
        let content_hash = content_hash.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO vault_mirror_state (file_name, content_hash, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(file_name) DO UPDATE SET
                    content_hash = excluded.content_hash,
                    updated_at = excluded.updated_at",
                params![file_name, content_hash, updated_at],
            )
            .map_err(|e| StorageError::Internal(format!("upsert_vault_hash: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn delete_vault_hash(&self, file_name: &str) -> Result<(), CoreError> {
        let file_name = file_name.to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "DELETE FROM vault_mirror_state WHERE file_name = ?1",
                params![file_name],
            )
            .map_err(|e| StorageError::Internal(format!("delete_vault_hash: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn last_cycle_summary(&self) -> Result<Option<VaultLastCycleSummary>, CoreError> {
        let encoded: Option<String> = self
            .with_conn_read(move |conn| {
                conn.query_row(
                    "SELECT content_hash FROM vault_mirror_state WHERE file_name = ?1",
                    params![LAST_CYCLE_KEY],
                    |r| r.get::<_, String>(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(StorageError::Internal(format!(
                        "read last_cycle_summary: {other}"
                    ))),
                })
            })
            .await?;
        let Some(encoded) = encoded else {
            return Ok(None);
        };
        // A row this row-kind cannot parse is a downgrade/corruption artifact of
        // a purely informational field. Reporting "no cycle recorded" is the
        // honest degradation; propagating would take the whole settings screen
        // (and the §3 path controls with it) down over a status line. The next
        // cycle overwrites it.
        match serde_json::from_str(&encoded) {
            Ok(summary) => Ok(Some(summary)),
            Err(e) => {
                warn!("vault mirror: unreadable last-cycle summary row, ignoring: {e}");
                Ok(None)
            }
        }
    }

    async fn put_last_cycle_summary(
        &self,
        summary: &VaultLastCycleSummary,
    ) -> Result<(), CoreError> {
        let encoded = serde_json::to_string(summary)
            .map_err(|e| StorageError::Internal(format!("encode last_cycle_summary: {e}")))?;
        let updated_at = summary.finished_at;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO vault_mirror_state (file_name, content_hash, updated_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(file_name) DO UPDATE SET
                    content_hash = excluded.content_hash,
                    updated_at = excluded.updated_at",
                params![LAST_CYCLE_KEY, encoded, updated_at],
            )
            .map_err(|e| StorageError::Internal(format!("put_last_cycle_summary: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    #[tokio::test]
    async fn upsert_get_delete_roundtrip() {
        let s = SqliteStorage::open_in_memory(30).expect("storage");
        s.upsert_vault_hash("claims.md", "h1", 1).await.unwrap();
        s.upsert_vault_hash("daily/2026-07-29.md", "h2", 2)
            .await
            .unwrap();
        s.upsert_vault_hash("claims.md", "h3", 3).await.unwrap();

        let hashes = s.vault_hashes().await.unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes.get("claims.md").map(String::as_str), Some("h3"));
        assert_eq!(
            hashes.get("daily/2026-07-29.md").map(String::as_str),
            Some("h2")
        );

        s.delete_vault_hash("claims.md").await.unwrap();
        let hashes = s.vault_hashes().await.unwrap();
        assert_eq!(hashes.len(), 1);
        assert!(!hashes.contains_key("claims.md"));
    }

    fn summary(finished_at: i64, paths: &[&str]) -> VaultLastCycleSummary {
        VaultLastCycleSummary {
            finished_at,
            day_files_written: 2,
            files_expired: 1,
            conflicts: paths.len(),
            conflict_paths: paths.iter().map(|p| (*p).to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn last_cycle_summary_roundtrips_and_keeps_only_the_newest() {
        // #9522: the whole point is that a SCHEDULED cycle's §6.4 conflicts
        // survive the invocation, so the next settings-screen read sees them.
        let s = SqliteStorage::open_in_memory(30).expect("storage");
        assert_eq!(
            s.last_cycle_summary().await.unwrap(),
            None,
            "no cycle has run yet"
        );

        s.put_last_cycle_summary(&summary(100, &["daily/2026-07-29.md"]))
            .await
            .unwrap();
        let read = s.last_cycle_summary().await.unwrap().expect("summary");
        assert_eq!(read, summary(100, &["daily/2026-07-29.md"]));

        // Replace, not append: the row is "last cycle", singular.
        s.put_last_cycle_summary(&summary(200, &[])).await.unwrap();
        let read = s.last_cycle_summary().await.unwrap().expect("summary");
        assert_eq!(read.finished_at, 200);
        assert!(read.conflict_paths.is_empty());
    }

    #[tokio::test]
    async fn the_reserved_summary_row_is_not_mistaken_for_a_file_hash() {
        // `vault_hashes` feeds §1.4 staleness decisions keyed by vault-relative
        // name; the reserved row must never collide with one, and the summary
        // must not be readable as (or clobbered by) a hash row.
        let s = SqliteStorage::open_in_memory(30).expect("storage");
        s.put_last_cycle_summary(&summary(300, &["claims.md"]))
            .await
            .unwrap();
        s.upsert_vault_hash("claims.md", "h1", 1).await.unwrap();

        let hashes = s.vault_hashes().await.unwrap();
        assert_eq!(hashes.get("claims.md").map(String::as_str), Some("h1"));
        assert!(
            !hashes
                .keys()
                .any(|k| !k.starts_with("::") && k != "claims.md"),
            "no file-shaped key other than the real one: {hashes:?}"
        );
        assert_eq!(
            s.last_cycle_summary().await.unwrap().map(|s| s.finished_at),
            Some(300)
        );
    }

    #[tokio::test]
    async fn an_unparseable_summary_row_reads_as_no_summary() {
        // Informational field: a corrupt/downgraded row must degrade to "no
        // cycle recorded", not take the settings screen down with it.
        let s = SqliteStorage::open_in_memory(30).expect("storage");
        s.upsert_vault_hash(LAST_CYCLE_KEY, "not-json", 1)
            .await
            .unwrap();
        assert_eq!(s.last_cycle_summary().await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_summary_does_not_survive_gdpr_delete_all_data() {
        // ADR-033 §4 conformance at the PORT level, asserted through the accessors the
        // settings surface actually calls (the table-level guard is
        // `sqlite::tests::vault_mirror_state_is_erased_by_gdpr_delete_all_data`).
        // #9522 put conflict FILE NAMES from the user's own folder into this
        // row, so a reserved row the ALL_TABLES sweep missed would keep
        // reporting a cycle — and those names — after Art.17 erased everything
        // else. Reusing the swept table is what makes that impossible.
        let s = SqliteStorage::open_in_memory(30).expect("storage");
        s.upsert_vault_hash("claims.md", "h1", 1).await.unwrap();
        s.put_last_cycle_summary(&summary(400, &["daily/2026-07-29.md"]))
            .await
            .unwrap();

        s.delete_all_data().expect("delete_all_data");

        assert_eq!(
            s.last_cycle_summary().await.unwrap(),
            None,
            "the §6.4 summary must not outlive the data it describes"
        );
        assert!(s.vault_hashes().await.unwrap().is_empty());
    }
}
