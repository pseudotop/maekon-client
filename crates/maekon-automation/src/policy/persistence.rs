//! Durable persistence for automation execution policies (#7915).
//!
//! Mirrors the `ConsentManager` local-JSON pattern (ADR-026): a small,
//! owner-only JSON file in the app data dir, written atomically (temp file +
//! rename) and loaded FAIL-CLOSED. Any corrupt / unreadable / unknown-schema
//! file yields an EMPTY policy set (exactly a fresh install), so a parse fault
//! can never grant broader permissions than the previous in-memory-only
//! default. Before this module, `PolicyClient::new()` loaded nothing and every
//! mutation was in-memory only, so user-granted execution policies evaporated
//! on restart — forcing every Codex command to re-escalate as `NoMatch`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::models::ExecutionPolicy;

/// On-disk schema version of the persisted policy store. Bump only for a
/// breaking envelope change; an unknown version is treated as fail-closed empty
/// (never partial-parsed) so future scope-B work can migrate deliberately.
pub(super) const POLICY_STORE_SCHEMA_VERSION: u32 = 1;

/// Versioned envelope written to disk. `schema_version` lets a future migration
/// detect the format; `policies` is the authoritative set. Deliberately minimal
/// (no expiry / signing metadata) — scope A only makes the EXISTING policy set
/// durable, it does not extend what kinds of policies exist.
#[derive(Debug, Serialize, Deserialize)]
struct PersistedPolicyStore {
    schema_version: u32,
    policies: Vec<ExecutionPolicy>,
}

/// Load the persisted execution policies from `path`, FAIL-CLOSED to an empty
/// set on any failure:
/// - missing / unreadable file → empty (legitimate first run; silent)
/// - corrupt / schema-drifted JSON → empty + warn (visible in telemetry)
/// - unknown `schema_version` → empty + warn (never partial-parse)
///
/// A parse failure must NEVER grant broader permissions than a fresh install,
/// so the caller starts with exactly the prior behavior (no policies) on error.
pub(super) fn load_policies(path: &Path) -> Vec<ExecutionPolicy> {
    let Ok(data) = std::fs::read_to_string(path) else {
        // Absent/unreadable file is a legitimate first run — stay silent, like
        // ConsentManager::load_from_file's missing-file branch.
        return Vec::new();
    };
    let store: PersistedPolicyStore = match serde_json::from_str(&data) {
        Ok(store) => store,
        Err(e) => {
            tracing::warn!(
                err.code = "internal.serialization",
                path = %path.display(),
                error = %e,
                "automation policy store present but failed to parse — starting with EMPTY policies (fail-closed)"
            );
            return Vec::new();
        }
    };
    if store.schema_version != POLICY_STORE_SCHEMA_VERSION {
        tracing::warn!(
            err.code = "internal.serialization",
            path = %path.display(),
            found = store.schema_version,
            expected = POLICY_STORE_SCHEMA_VERSION,
            "automation policy store schema_version is unknown — starting with EMPTY policies (fail-closed, no partial parse)"
        );
        return Vec::new();
    }
    // ExecutionPolicy carries no TTL/expiry field, so there is nothing to expire
    // on load (scope discipline — expiry is deliberately NOT added).
    store.policies
}

/// Atomically persist `policies` to `path` as a versioned envelope.
///
/// Uses the house write pattern shared with consent/config: serialize to a
/// `.tmp` sibling with owner-only permissions (`write_owner_only` — the Unix
/// `0o600` / Windows owner-only-DACL SSOT), then `rename` into place, so a crash
/// mid-write can never leave a truncated file that `load_policies` would reject.
pub(super) fn save_policies(path: &Path, policies: &[ExecutionPolicy]) -> std::io::Result<()> {
    let store = PersistedPolicyStore {
        schema_version: POLICY_STORE_SCHEMA_VERSION,
        policies: policies.to_vec(),
    };
    let json = serde_json::to_string_pretty(&store).map_err(std::io::Error::other)?;
    let tmp_path = path.with_extension("tmp");
    if let Some(parent) = tmp_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Owner-only tmp before the rename: policies may carry sensitive allow-lists
    // (paths, args), so the published file is never world/group-readable.
    maekon_core::secure_file::write_owner_only(&tmp_path, json.as_bytes())?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{load_policies, save_policies, POLICY_STORE_SCHEMA_VERSION};
    use crate::policy::{AuditLevel, ExecutionPolicy, PolicyClient};

    fn sample_policy(policy_id: &str, process: &str) -> ExecutionPolicy {
        ExecutionPolicy {
            policy_id: policy_id.to_string(),
            process_name: process.to_string(),
            process_hash: None,
            allowed_args: vec!["status".to_string()],
            requires_sudo: false,
            max_execution_time_ms: 5000,
            audit_level: AuditLevel::Basic,
            sandbox_profile: None,
            allowed_paths: vec![],
            allow_network: Some(true),
            require_signed_token: false,
            confirmation: Default::default(),
        }
    }

    #[test]
    fn save_then_load_round_trips_the_policy_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");
        let policies = vec![sample_policy("pol-1", "git"), sample_policy("pol-2", "ls")];
        save_policies(&path, &policies).unwrap();

        let loaded = load_policies(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].policy_id, "pol-1");
        assert_eq!(loaded[0].allowed_args, vec!["status".to_string()]);
        assert_eq!(loaded[1].process_name, "ls");
        assert_eq!(loaded[1].allow_network, Some(true));
    }

    #[test]
    fn save_is_atomic_no_tmp_leftover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");
        save_policies(&path, &[sample_policy("pol-1", "git")]).unwrap();
        assert!(
            path.exists(),
            "the store file must be written by save_policies"
        );
        assert!(
            !path.with_extension("tmp").exists(),
            ".tmp must be renamed into place (atomic write), not left behind"
        );
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        assert!(
            load_policies(&path).is_empty(),
            "a first run (no file) must start with zero policies"
        );
    }

    #[test]
    fn corrupt_file_fails_closed_to_empty() {
        // A parse failure must NEVER grant broader permission than a fresh
        // install — the corrupt file yields zero policies.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");
        std::fs::write(&path, b"{ this is not valid json ]").unwrap();
        assert!(
            load_policies(&path).is_empty(),
            "corrupt store must fail closed to empty"
        );
    }

    #[test]
    fn unknown_schema_version_fails_closed_to_empty() {
        // A future/unknown envelope version is fail-closed empty (never
        // partial-parsed), even though the `policies` array is well-formed.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");
        let forward = serde_json::json!({
            "schema_version": POLICY_STORE_SCHEMA_VERSION + 999,
            "policies": [{
                "policy_id": "pol-future",
                "process_name": "git",
                "process_hash": null,
                "allowed_args": [],
                "requires_sudo": false,
                "max_execution_time_ms": 5000
            }]
        });
        std::fs::write(&path, serde_json::to_vec(&forward).unwrap()).unwrap();
        assert!(
            load_policies(&path).is_empty(),
            "unknown schema_version must fail closed to empty (no partial parse)"
        );
    }

    // ── DoD (#7915): PolicyClient durability across a simulated restart ──

    #[tokio::test]
    async fn policy_client_persists_across_reconstruction() {
        // Construct → add a policy → drop → reconstruct from the SAME path →
        // the policy is still there (policies survive restart), so Codex approval
        // no longer re-escalates a previously-granted process as NoMatch.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");

        let before = PolicyClient::with_persistence(path.clone());
        before.add_policy(sample_policy("pol-git", "git")).await;
        drop(before);

        let after = PolicyClient::with_persistence(path);
        let policies = after.list_policies().await;
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].policy_id, "pol-git");
        // The allowed-process index is rebuilt on load, so the process is matched
        // (not re-escalated) after restart.
        assert!(after.is_process_allowed("git").await);
        assert!(after.get_policy_for_process("git").await.is_some());
    }

    #[tokio::test]
    async fn policy_client_remove_is_durable_across_reconstruction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");

        let before = PolicyClient::with_persistence(path.clone());
        before.add_policy(sample_policy("pol-1", "git")).await;
        before.add_policy(sample_policy("pol-2", "ls")).await;
        assert!(before.remove_policy("pol-1").await.unwrap());
        drop(before);

        let after = PolicyClient::with_persistence(path);
        let policies = after.list_policies().await;
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].policy_id, "pol-2");
        assert!(
            !after.is_process_allowed("git").await,
            "a removed policy must not resurrect on restart"
        );
    }

    #[tokio::test]
    async fn policy_client_update_policies_is_durable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");

        let before = PolicyClient::with_persistence(path.clone());
        before
            .update_policies(vec![
                sample_policy("pol-a", "cat"),
                sample_policy("pol-b", "grep"),
            ])
            .await;
        drop(before);

        let after = PolicyClient::with_persistence(path);
        assert_eq!(after.list_policies().await.len(), 2);
        assert!(after.is_process_allowed("cat").await);
        assert!(after.is_process_allowed("grep").await);
    }

    #[tokio::test]
    async fn in_memory_client_writes_no_file() {
        // `PolicyClient::new()` stays in-memory only (storage_path None), so the
        // many test/bench call sites keep their current behavior and no stray
        // file is created next to the cwd.
        let client = PolicyClient::new();
        client.add_policy(sample_policy("pol-1", "git")).await;
        assert_eq!(client.list_policies().await.len(), 1);
        assert!(
            !std::path::Path::new("automation_policies.json").exists(),
            "a non-persistent client must not write any policy file"
        );
    }

    #[tokio::test]
    async fn corrupt_store_yields_empty_client_not_stale_grant() {
        // Simulated restart onto a corrupt file: the reconstructed client must
        // start EMPTY (fail-closed), matching a fresh install — never inherit a
        // stale/garbled grant.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");
        std::fs::write(&path, b"<<< corrupt >>>").unwrap();

        let client = PolicyClient::with_persistence(path);
        assert!(client.list_policies().await.is_empty());
        assert!(!client.is_process_allowed("git").await);
    }

    // ── Part A (#7932): persist-first revocation is fail-visible ──

    #[tokio::test]
    async fn remove_policy_success_removes_from_memory_and_disk() {
        // The happy path drops the policy from BOTH the in-memory cache and the
        // durable file: a fresh client reconstructed from the same path (a
        // simulated restart) sees the revocation, so it never resurrects.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");

        let client = PolicyClient::with_persistence(path.clone());
        client.add_policy(sample_policy("pol-1", "git")).await;
        client.add_policy(sample_policy("pol-2", "ls")).await;

        assert!(client.remove_policy("pol-1").await.unwrap());
        // In-memory: pol-1 and its process are gone.
        let list = client.list_policies().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].policy_id, "pol-2");
        assert!(!client.is_process_allowed("git").await);

        // On-disk: reconstruct from the same file — the revocation is durable.
        let after_restart = PolicyClient::with_persistence(path);
        let reloaded = after_restart.list_policies().await;
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].policy_id, "pol-2");
        assert!(
            !after_restart.is_process_allowed("git").await,
            "a successfully removed policy must not resurrect on restart"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn remove_policy_persist_failure_leaves_memory_and_disk_unchanged() {
        // Inject a durable-write failure by making the store's parent directory
        // read-only, so the atomic temp-write in `save_policies` fails. The
        // revocation must then FAIL VISIBLY: an Err is returned, the in-memory
        // policy is still present, and the on-disk set is byte-for-byte unchanged
        // — never a silent memory-only removal that resurrects on restart.
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("automation_policies.json");

        let client = PolicyClient::with_persistence(path.clone());
        client.add_policy(sample_policy("pol-1", "git")).await;
        client.add_policy(sample_policy("pol-2", "ls")).await;
        let disk_before =
            std::fs::read(&path).expect("store file must exist before the failed remove");

        // Make the parent dir read-only so creating the `.tmp` sibling fails.
        let mut ro = std::fs::metadata(dir.path()).unwrap().permissions();
        ro.set_mode(0o500);
        std::fs::set_permissions(dir.path(), ro).unwrap();

        let result = client.remove_policy("pol-1").await;

        // Restore write permission so tempdir cleanup succeeds regardless of outcome.
        let mut rw = std::fs::metadata(dir.path()).unwrap().permissions();
        rw.set_mode(0o700);
        std::fs::set_permissions(dir.path(), rw).unwrap();

        // Fail-visible: an Err surfaces to the caller, not a silent success.
        let err =
            result.expect_err("a durable-write failure must surface as Err, not a silent removal");
        assert!(
            matches!(err, crate::error::AutomationError::Internal(_)),
            "persist failure must map to a typed Internal error, got {err:?}"
        );

        // Memory unchanged: pol-1 is still present and still an allowed process.
        let list = client.list_policies().await;
        assert_eq!(
            list.len(),
            2,
            "the in-memory policy set must be unchanged after a failed revocation"
        );
        assert!(client.is_process_allowed("git").await);

        // Disk unchanged: the on-disk set still grants pol-1 (no partial write).
        let disk_after = std::fs::read(&path).expect("store file must still exist");
        assert_eq!(
            disk_before, disk_after,
            "the on-disk policy set must not change when the durable revocation write fails"
        );

        // And a simulated restart still sees pol-1 (consistent with disk_before).
        let after_restart = PolicyClient::with_persistence(path);
        assert!(
            after_restart.is_process_allowed("git").await,
            "a rejected revocation must leave the durable grant intact across restart"
        );
    }
}
