//! OS keychain secret store adapter.
//!
//! Hybrid approach: `keyring` crate for OS keychain storage,
//! JSON file as enumeration cache for `delete_namespace`/list.
// OOS-TBD: ADR-013 file split — registry (KeychainRegistry + locks) vs
// keyring ops vs async adapter are natural module seams once this grows again.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::StorageError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::ports::secret_store::SecretStore;
use serde::{Deserialize, Serialize};
use tracing::warn;

/// Keys that OAuth flows are known to store.
/// `delete_namespace` always tries these in addition to registry contents.
///
/// This list is the *registry-loss* safety net: when the enumeration cache no
/// longer names a namespace (last-writer-wins across the several live
/// `KeychainOps` instances), it is all `delete_namespace` has to go on. Every
/// key any flow persists must therefore appear here — a missing one survives
/// logout in the OS keychain indefinitely. `identifier` / `organization_id`
/// come from the #9459 ONESHIM login session (`oneshim/auth/default`) and are
/// PII, so their omission was a privacy leak, not just stale data.
const KNOWN_OAUTH_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "scopes",
    "expires_at",
    "id_token",
    "identifier",
    "organization_id",
];

/// JSON enumeration cache — tracks which namespace/key combinations exist
/// in the OS keychain. NOT source of truth; keychain is authoritative.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct KeychainRegistry {
    pub version: u32,
    pub namespaces: HashMap<String, BTreeSet<String>>,
}

impl KeychainRegistry {
    fn new() -> Self {
        Self {
            version: 1,
            namespaces: HashMap::new(),
        }
    }

    /// Load from disk. Returns empty registry on missing/corrupt file.
    pub fn load_or_default(path: &std::path::Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str::<Self>(&contents) {
                Ok(reg) => reg,
                Err(e) => {
                    warn!(
                        "Corrupt keychain registry at {}: {e}. Starting empty.",
                        path.display()
                    );
                    Self::new()
                }
            },
            Err(_) => Self::new(),
        }
    }

    /// Persist the registry to `path` using the tmp-file + 0o600/DACL + atomic-rename
    /// hardening pattern (mirrors `FileSecretRegistry::save` /
    /// `integration_state_store::save`).
    ///
    /// This enumeration cache lists which namespace/key combinations exist in
    /// the OS keychain (e.g. OAuth provider names). The values themselves live
    /// in the OS keychain, but the namespace/key inventory is still privacy-
    /// sensitive, so the persisted file is created owner-only: mode 0o600 on
    /// Unix (atomic `create_new` + `mode`, no world-readable window) and, on
    /// Windows, an owner-only DACL applied to the EMPTY tmp before the inventory
    /// bytes are written (closing the creation-time read window, #6942). The
    /// Windows DACL step stays NON-FATAL (this is an enumeration cache, not the
    /// secret values): on failure it warns and falls back to the inherited ACL.
    pub fn save(&self, path: &std::path::Path) -> Result<(), StorageError> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| StorageError::SecretStore(format!("registry serialization: {e}")))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        // Remove any orphaned tmp from a previously aborted save so the atomic
        // create below starts clean (no stale contents / inherited permissions).
        let _ = std::fs::remove_file(&tmp);

        // Unix: create the tmp file with mode 0o600 ATOMICALLY (O_CREAT|O_EXCL +
        // mode in a single open) so the registry is never world-readable —
        // mirrors FileSecretRegistry::save. On any write error the tmp is
        // cleaned up.
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| {
                    StorageError::SecretStore(format!("keychain registry tmp create: {e}"))
                })?;
            f.write_all(json.as_bytes()).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                StorageError::SecretStore(format!("keychain registry tmp write: {e}"))
            })?;
        }

        // Windows: create the tmp EMPTY, apply an owner-only DACL while it holds
        // no inventory bytes, then write the payload — so on the success path the
        // registry is never readable via the inherited parent-directory ACL once
        // it contains the namespace/key inventory (#6942 closes the creation-time
        // window the previous "write then tighten" order left open; reuses the
        // shared helper in `encryption.rs`). DACL failure stays NON-FATAL: this is
        // an enumeration cache (the secret values live in the OS keychain), so on
        // failure we warn and fall back to writing under the inherited ACL,
        // preserving the pre-existing best-effort contract.
        #[cfg(windows)]
        {
            use std::io::Write as _;
            // Empty tmp first — no inventory bytes exist during the brief
            // inherited-ACL window.
            if let Err(e) = std::fs::File::create(&tmp) {
                let _ = std::fs::remove_file(&tmp);
                return Err(StorageError::SecretStore(format!(
                    "keychain registry tmp create: {e}"
                )));
            }
            // Tighten to owner-only BEFORE writing the inventory (non-fatal).
            if let Err(e) = crate::encryption::set_owner_only_dacl(&tmp) {
                warn!("keychain registry: failed to set owner-only DACL: {e}");
            }
            // Write the inventory into the (owner-only on the success path) tmp.
            match std::fs::OpenOptions::new().write(true).open(&tmp) {
                Ok(mut f) => {
                    if let Err(e) = f.write_all(json.as_bytes()) {
                        let _ = std::fs::remove_file(&tmp);
                        return Err(StorageError::SecretStore(format!(
                            "keychain registry tmp write: {e}"
                        )));
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(StorageError::SecretStore(format!(
                        "keychain registry tmp open for write: {e}"
                    )));
                }
            }
        }

        // Exotic targets without unix/windows permission models: plain write.
        #[cfg(not(any(unix, windows)))]
        {
            std::fs::write(&tmp, json.as_bytes()).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                StorageError::SecretStore(format!("keychain registry tmp write: {e}"))
            })?;
        }

        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn add_key(&mut self, namespace: &str, key: &str) {
        self.namespaces
            .entry(namespace.to_owned())
            .or_default()
            .insert(key.to_owned());
    }

    pub fn remove_key(&mut self, namespace: &str, key: &str) {
        if let Some(keys) = self.namespaces.get_mut(namespace) {
            keys.remove(key);
            if keys.is_empty() {
                self.namespaces.remove(namespace);
            }
        }
    }

    pub fn keys_for(&self, namespace: &str) -> BTreeSet<String> {
        self.namespaces.get(namespace).cloned().unwrap_or_default()
    }

    pub fn all_namespaces(&self) -> Vec<String> {
        self.namespaces.keys().cloned().collect()
    }
}

/// #9493: process-global per-registry-file locks. Several independent
/// `KeychainOps` instances live in one process (provider runtime, automation
/// builder, server runtime context, session wiring, auth wiring); each used to
/// hold a boot-time in-memory registry copy and save it whole-file, so the
/// last writer erased every other instance's registrations. Every registry
/// mutation now runs reload → apply-one-op → save under this per-path lock —
/// the shared file is the merge point, and same-process instances can no
/// longer interleave between reload and save. Bounded: one entry per distinct
/// registry path (production has exactly one). Paths are used as configured
/// (not canonicalized) — all in-process instances receive the same
/// config-derived path, and the file may not exist yet on first use.
static REGISTRY_FILE_LOCKS: std::sync::OnceLock<
    parking_lot::Mutex<HashMap<PathBuf, Arc<parking_lot::Mutex<()>>>>,
> = std::sync::OnceLock::new();

fn registry_file_lock(path: &Path) -> Arc<parking_lot::Mutex<()>> {
    REGISTRY_FILE_LOCKS
        .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
        .lock()
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(parking_lot::Mutex::new(())))
        .clone()
}

/// #9523: cross-process advisory lock for the registry file. The in-process
/// `REGISTRY_FILE_LOCKS` map cannot reach a second process — `maekon-app auth
/// revoke` / `maekon-app secret ...` run as their own short-lived process
/// while the GUI app may be live on the same registry — so every registry
/// mutation additionally holds an exclusive SQLite transaction on a sibling
/// `<registry stem>.lock.db`. SQLite's file locking is the one battle-tested
/// cross-platform advisory lock already in the dependency tree (rusqlite is a
/// direct dependency; no new supply-chain surface). The lock db never holds
/// data — only its OS-level file lock matters.
///
/// Best-effort by policy: if the lock db cannot be opened or the busy timeout
/// expires, we warn and proceed WITHOUT the cross-process guard. The registry
/// is an enumeration cache whose loss is self-healing (`KNOWN_OAUTH_KEYS`
/// sweep), and wedging a revoke behind an unremovable stale lock would be
/// worse than a transient cross-process enumeration race.
struct RegistryXprocLock {
    conn: rusqlite::Connection,
}

impl RegistryXprocLock {
    fn acquire(registry_path: &Path, busy_timeout: std::time::Duration) -> Option<Self> {
        let lock_path = registry_path.with_extension("lock.db");
        // Review nit (#9529): pre-create the lock file owner-only so it
        // matches the directory's permission posture. It only ever holds an
        // empty SQLite header (no inventory, no secrets), so failure here is
        // non-fatal — SQLite reuses the file whatever its mode.
        if !lock_path.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&lock_path);
            }
            #[cfg(windows)]
            {
                if std::fs::File::create(&lock_path).is_ok() {
                    if let Err(e) = crate::encryption::set_owner_only_dacl(&lock_path) {
                        warn!("keychain registry lock db: failed to set owner-only DACL: {e}");
                    }
                }
            }
        }
        let conn = match rusqlite::Connection::open(&lock_path) {
            Ok(conn) => conn,
            Err(e) => {
                warn!("keychain registry xproc lock db open failed: {e}");
                return None;
            }
        };
        if let Err(e) = conn.busy_timeout(busy_timeout) {
            warn!("keychain registry xproc lock busy_timeout failed: {e}");
            return None;
        }
        // BEGIN IMMEDIATE takes the RESERVED lock — exclusive against every
        // other writer connection, in this process or any other.
        match conn.execute_batch("BEGIN IMMEDIATE") {
            Ok(()) => Some(Self { conn }),
            Err(e) => {
                warn!("keychain registry xproc lock acquire failed (proceeding unguarded): {e}");
                None
            }
        }
    }
}

impl Drop for RegistryXprocLock {
    fn drop(&mut self) {
        if let Err(e) = self.conn.execute_batch("COMMIT") {
            warn!("keychain registry xproc lock release failed: {e}");
        }
    }
}

/// One registry mutation, applied against a fresh reload of the file.
enum RegistryOp<'a> {
    Add {
        namespace: &'a str,
        key: &'a str,
    },
    Remove {
        namespace: &'a str,
        key: &'a str,
    },
    RemoveMany {
        namespace: &'a str,
        keys: &'a BTreeSet<String>,
    },
}

/// Sync core — all keyring and registry operations.
/// Used directly by CLI (no tokio needed), wrapped by KeychainSecretStore for async port.
pub struct KeychainOps {
    service_name: String,
    registry: parking_lot::Mutex<KeychainRegistry>,
    registry_path: PathBuf,
}

/// Status of a namespace in the keychain.
#[derive(Debug)]
pub struct NamespaceStatus {
    pub connected: bool,
    pub keys_found: Vec<String>,
    pub expires_at: Option<String>,
}

impl KeychainOps {
    /// Create ops with the given registry file path.
    /// The caller (setup.rs) resolves the config directory.
    pub fn new(registry_path: PathBuf) -> Result<Self, StorageError> {
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let registry = KeychainRegistry::load_or_default(&registry_path);
        Ok(Self {
            // #9588: service name = flavored app identity; pre-scoping items
            // stay reachable via `retrieve_sync`'s legacy read-through. See
            // `keychain_service_name` docs for scope and non-goals.
            service_name: maekon_core::config_manager::keychain_service_name(),
            registry: parking_lot::Mutex::new(registry),
            registry_path,
        })
    }

    /// #9493 core: reload the registry file, apply ONE mutation, save, and
    /// refresh this instance's cache — all under the process-global per-path
    /// lock, so a concurrent instance's mutation can never interleave between
    /// our reload and our save. Lock order (always this way): file lock →
    /// `self.registry`. Save failure is best-effort like before (the OS
    /// keychain stays authoritative; `KNOWN_OAUTH_KEYS` remains the sweep
    /// safety net), but the cache is refreshed regardless so this instance is
    /// never staler than what it just read.
    fn registry_apply(&self, op: RegistryOp<'_>) {
        let file_lock = registry_file_lock(&self.registry_path);
        let _file_guard = file_lock.lock();
        // #9523: cross-process guard on top of the in-process one. Held (via
        // Drop) through save so a concurrent CLI process cannot interleave
        // between our reload and our save. Best-effort — None proceeds
        // unguarded (see RegistryXprocLock docs). Blocking up to the busy
        // timeout is fine here: this fn always runs on a blocking thread
        // (spawn_blocking) or in the pre-runtime sync CLI.
        let _xproc_guard =
            RegistryXprocLock::acquire(&self.registry_path, std::time::Duration::from_secs(5));
        let mut disk = KeychainRegistry::load_or_default(&self.registry_path);
        match op {
            RegistryOp::Add { namespace, key } => disk.add_key(namespace, key),
            RegistryOp::Remove { namespace, key } => disk.remove_key(namespace, key),
            RegistryOp::RemoveMany { namespace, keys } => {
                for key in keys {
                    disk.remove_key(namespace, key);
                }
            }
        }
        if let Err(e) = disk.save(&self.registry_path) {
            warn!("Failed to persist keychain registry: {e}");
        }
        *self.registry.lock() = disk;
    }

    /// Keychain account for a namespace/key pair. The service half of every
    /// entry is `self.service_name`; all keyring touches route through the
    /// bounded primitives in `keychain_guard` (#9588 — a pending macOS ACL
    /// prompt degrades into an explicit error instead of hanging boot).
    fn user(namespace: &str, key: &str) -> String {
        format!("{namespace}.{key}")
    }

    pub fn store_sync(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError> {
        // 1. Write to keychain (bounded, #9588)
        crate::keychain_guard::set_bounded(&self.service_name, &Self::user(namespace, key), value)?;
        // 2. Registry: reload-apply-save under the per-path lock (#9493) —
        //    best-effort like before (the keychain already holds the
        //    authoritative value; KNOWN_OAUTH_KEYS covers a lost save).
        self.registry_apply(RegistryOp::Add { namespace, key });
        Ok(())
    }

    pub fn retrieve_sync(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<String>, StorageError> {
        // #9588 review I4 + re-review N1: master-key-namespace-only legacy
        // read-through keeps pre-scoping flavored profiles' databases
        // decryptable (policy + namespace gate in `keychain_guard`).
        let (value, migrated) = crate::keychain_guard::get_bounded_for_namespace(
            &self.service_name,
            namespace,
            &Self::user(namespace, key),
        )?;
        if migrated {
            self.registry_apply(RegistryOp::Add { namespace, key });
        }
        Ok(value)
    }

    pub fn delete_sync(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        crate::keychain_guard::delete_bounded(&self.service_name, &Self::user(namespace, key))?;
        self.registry_apply(RegistryOp::Remove { namespace, key });
        Ok(())
    }

    pub fn delete_namespace_sync(&self, namespace: &str) -> Result<(), StorageError> {
        // Build key set = fresh-disk registry keys ∪ this instance's cached
        // keys ∪ KNOWN_OAUTH_KEYS. The disk reload (#9493) sees registrations
        // made by OTHER instances; the cache union is safe-biased (an extra
        // key only costs a tolerated NoEntry delete, a missing key survives
        // logout in the OS keychain).
        let mut all_keys: BTreeSet<String> = {
            let file_lock = registry_file_lock(&self.registry_path);
            let _file_guard = file_lock.lock();
            KeychainRegistry::load_or_default(&self.registry_path).keys_for(namespace)
        };
        all_keys.append(&mut self.registry.lock().keys_for(namespace));
        for k in KNOWN_OAUTH_KEYS {
            all_keys.insert((*k).to_owned());
        }

        let mut errors = Vec::new();
        let mut failed_keys = BTreeSet::new();
        for key in &all_keys {
            // Bounded delete (#9588): NoEntry is tolerated inside; a timeout
            // (pending ACL prompt) records the key as failed so the registry
            // keeps naming it for a later retry.
            if let Err(e) = crate::keychain_guard::delete_bounded(
                &self.service_name,
                &Self::user(namespace, key),
            ) {
                failed_keys.insert(key.clone());
                errors.push(format!("{key}: {e}"));
            }
        }

        let deleted_keys: BTreeSet<String> = all_keys.difference(&failed_keys).cloned().collect();

        self.registry_apply(RegistryOp::RemoveMany {
            namespace,
            keys: &deleted_keys,
        });

        if errors.is_empty() {
            Ok(())
        } else {
            Err(StorageError::SecretStore(format!(
                "partial delete_namespace failure: {}",
                errors.join("; ")
            )))
        }
    }

    pub fn all_namespaces(&self) -> Vec<String> {
        // #9493: enumerate from a fresh reload, not the boot-time cache —
        // another instance may have registered namespaces since construction.
        let file_lock = registry_file_lock(&self.registry_path);
        let _file_guard = file_lock.lock();
        let disk = KeychainRegistry::load_or_default(&self.registry_path);
        let namespaces = disk.all_namespaces();
        *self.registry.lock() = disk;
        namespaces
    }

    /// Probe a namespace: check which known keys exist in the keychain.
    pub fn probe_namespace(&self, namespace: &str) -> NamespaceStatus {
        let mut keys_found = Vec::new();
        let mut expires_at = None;

        for key in KNOWN_OAUTH_KEYS {
            if let Ok(Some(val)) = self.retrieve_sync(namespace, key) {
                keys_found.push((*key).to_owned());
                if *key == "expires_at" {
                    expires_at = Some(val);
                }
            }
        }

        let connected = access_token_is_connected(&keys_found, expires_at.as_deref(), Utc::now());

        NamespaceStatus {
            connected,
            keys_found,
            expires_at,
        }
    }
}

fn access_token_is_connected(
    keys_found: &[String],
    expires_at: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    if !keys_found.iter().any(|key| key == "access_token") {
        return false;
    }

    let Some(expires_at) = expires_at else {
        return false;
    };

    let Ok(expires_at) = DateTime::parse_from_rfc3339(expires_at) else {
        return false;
    };

    now < expires_at.with_timezone(&Utc) - chrono::Duration::seconds(60)
}

/// Async adapter — wraps KeychainOps via spawn_blocking to implement SecretStore.
pub struct KeychainSecretStore {
    ops: Arc<KeychainOps>,
}

impl KeychainSecretStore {
    pub fn new(ops: Arc<KeychainOps>) -> Self {
        Self { ops }
    }
}

#[async_trait]
impl SecretStore for KeychainSecretStore {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        let ops = self.ops.clone();
        let ns = namespace.to_owned();
        let k = key.to_owned();
        let v = value.to_owned();
        tokio::task::spawn_blocking(move || ops.store_sync(&ns, &k, &v))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        let ops = self.ops.clone();
        let ns = namespace.to_owned();
        let k = key.to_owned();
        tokio::task::spawn_blocking(move || ops.retrieve_sync(&ns, &k))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        let ops = self.ops.clone();
        let ns = namespace.to_owned();
        let k = key.to_owned();
        tokio::task::spawn_blocking(move || ops.delete_sync(&ns, &k))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        let ops = self.ops.clone();
        let ns = namespace.to_owned();
        tokio::task::spawn_blocking(move || ops.delete_namespace_sync(&ns))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: maekon_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// #9459: `delete_namespace` falls back to `KNOWN_OAUTH_KEYS` whenever the
    /// registry has lost a namespace, so every key the ONESHIM login session
    /// persists must be listed there. A key missing from the fallback survives
    /// logout in the OS keychain forever — for `identifier` / `organization_id`
    /// that is retained PII, not merely a stale token.
    ///
    /// Asserted against the port constants rather than literals so renaming a
    /// persisted key without extending the fallback fails here.
    #[test]
    fn known_oauth_keys_cover_every_persisted_oneshim_session_key() {
        use maekon_core::ports::secret_store::{
            ONESHIM_ACCESS_TOKEN_SECRET_KEY, ONESHIM_EXPIRES_AT_SECRET_KEY,
            ONESHIM_IDENTIFIER_SECRET_KEY, ONESHIM_ORGANIZATION_ID_SECRET_KEY,
            ONESHIM_REFRESH_TOKEN_SECRET_KEY,
        };
        for key in [
            ONESHIM_ACCESS_TOKEN_SECRET_KEY,
            ONESHIM_REFRESH_TOKEN_SECRET_KEY,
            ONESHIM_EXPIRES_AT_SECRET_KEY,
            ONESHIM_IDENTIFIER_SECRET_KEY,
            ONESHIM_ORGANIZATION_ID_SECRET_KEY,
        ] {
            assert!(
                KNOWN_OAUTH_KEYS.contains(&key),
                "KNOWN_OAUTH_KEYS must list '{key}': it is the only key set \
                 delete_namespace can fall back on once the registry has lost \
                 the namespace"
            );
        }
    }

    #[test]
    fn registry_add_key_idempotent() {
        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "access_token");
        reg.add_key("openai", "access_token");
        assert_eq!(reg.keys_for("openai").len(), 1);
    }

    #[test]
    fn registry_remove_key_cleans_empty_namespace() {
        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "token");
        reg.remove_key("openai", "token");
        assert!(reg.all_namespaces().is_empty());
    }

    #[test]
    fn registry_remove_key_keeps_other_keys_in_namespace() {
        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "access_token");
        reg.add_key("openai", "refresh_token");
        reg.remove_key("openai", "access_token");
        assert_eq!(reg.keys_for("openai").len(), 1);
        assert!(reg.keys_for("openai").contains("refresh_token"));
    }

    #[test]
    fn registry_save_and_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");

        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "access_token");
        reg.add_key("openai", "refresh_token");
        reg.save(&path).unwrap();

        let loaded = KeychainRegistry::load_or_default(&path);
        assert_eq!(loaded.keys_for("openai").len(), 2);
    }

    /// keychain-perms: on Unix the persisted enumeration registry file must be
    /// owner-only (0o600) so the namespace/key inventory is not world-readable.
    /// Mirrors `file_secret_store::file_secret_store_is_owner_only_on_unix`.
    #[cfg(unix)]
    #[test]
    fn registry_save_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");

        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "access_token");
        reg.save(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        // Only the owner rw bits may be set; group/other must be clear.
        assert_eq!(
            mode & 0o777,
            0o600,
            "keychain registry file must be 0o600, got {:o}",
            mode & 0o777
        );
    }

    /// keychain-perms: an orphaned `.tmp` from a previously aborted save must
    /// not block the next save (the atomic `create_new` would otherwise fail
    /// with `AlreadyExists`). The pre-rename `remove_file` clears it.
    #[test]
    fn registry_save_overwrites_orphaned_tmp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");
        let tmp = path.with_extension("tmp");
        // Simulate a stale tmp left by an aborted save.
        std::fs::write(&tmp, "STALE ORPHANED CONTENTS").unwrap();

        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "access_token");
        reg.save(&path).unwrap();

        // Save succeeded, produced the real file, and consumed the orphaned tmp.
        let loaded = KeychainRegistry::load_or_default(&path);
        assert_eq!(loaded.keys_for("openai").len(), 1);
        assert!(
            !tmp.exists(),
            "orphaned tmp must be gone after a successful save"
        );
    }

    #[test]
    fn registry_save_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("registry.json");

        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "access_token");
        reg.save(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn registry_load_corrupt_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("registry.json");
        std::fs::write(&path, "NOT JSON").unwrap();

        let reg = KeychainRegistry::load_or_default(&path);
        assert!(reg.all_namespaces().is_empty());
    }

    #[test]
    fn registry_load_missing_returns_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");

        let reg = KeychainRegistry::load_or_default(&path);
        assert!(reg.all_namespaces().is_empty());
    }

    #[test]
    fn registry_btreeset_sorted_deterministic() {
        let mut reg = KeychainRegistry::new();
        reg.add_key("openai", "z_key");
        reg.add_key("openai", "a_key");
        let keys: Vec<String> = reg.keys_for("openai").into_iter().collect();
        assert_eq!(keys, vec!["a_key", "z_key"]);
    }

    #[tokio::test]
    async fn access_token_requires_valid_future_expiry() {
        let now = Utc::now();
        let keys = vec!["access_token".to_string()];

        let valid = access_token_is_connected(
            &keys,
            Some(&(now + chrono::Duration::minutes(5)).to_rfc3339()),
            now,
        );
        assert!(valid);
    }

    #[test]
    fn access_token_without_expiry_is_not_connected() {
        let keys = vec!["access_token".to_string()];
        assert!(!access_token_is_connected(&keys, None, Utc::now()));
    }

    #[test]
    fn access_token_with_expired_expiry_is_not_connected() {
        let now = Utc::now();
        let keys = vec!["access_token".to_string()];

        let valid = access_token_is_connected(
            &keys,
            Some(&(now - chrono::Duration::minutes(1)).to_rfc3339()),
            now,
        );
        assert!(!valid);
    }

    /// Integration tests — require OS keychain. Run with:
    /// `cargo test -p maekon-storage -- --ignored keychain`
    mod integration {
        use super::*;

        fn make_ops() -> (KeychainOps, TempDir) {
            let dir = TempDir::new().unwrap();
            let path = dir.path().join("registry.json");
            let ops = KeychainOps::new(path).unwrap();
            (ops, dir)
        }

        // F2 (P2): These three tests require the macOS Keychain daemon (securityd).
        //
        // Why `#[ignore]` is kept:
        //   - On Linux and Windows the OS keychain backend differs (libsecret /
        //     Windows Credential Manager); these tests exercise macOS-specific paths.
        //   - Running `cargo test` on a developer machine without a dedicated
        //     unlocked keychain should not trigger OS keychain dialogs or fail.
        //   `#[ignore]` preserves that local-default behaviour.
        //
        // How to run locally (macOS only):
        //   cargo test -p maekon-storage -- --ignored keychain
        //
        // CI (macos-latest leg of `test-platform`): the job creates a dedicated
        // CI keychain, sets it as default, unlocks it, and disables auto-lock
        // before running these tests explicitly via `-- --ignored keychain`.
        // See the "Prepare CI keychain (macOS)" and "Run keychain integration
        // tests (macOS)" steps in .github/workflows/ci.yml.
        #[test]
        #[ignore]
        fn keychain_store_and_retrieve() {
            let (ops, _dir) = make_ops();
            let ns = "test_maekon_integration";
            ops.store_sync(ns, "test_key", "test_value").unwrap();
            let val = ops.retrieve_sync(ns, "test_key").unwrap();
            assert_eq!(val, Some("test_value".to_owned()));
            // Cleanup
            ops.delete_sync(ns, "test_key").unwrap();
        }

        #[test]
        #[ignore]
        fn keychain_delete_returns_none() {
            let (ops, _dir) = make_ops();
            let ns = "test_maekon_integration";
            ops.store_sync(ns, "del_key", "val").unwrap();
            ops.delete_sync(ns, "del_key").unwrap();
            let val = ops.retrieve_sync(ns, "del_key").unwrap();
            assert_eq!(val, None);
        }

        #[test]
        #[ignore]
        fn keychain_delete_namespace_clears_all() {
            let (ops, _dir) = make_ops();
            let ns = "test_maekon_ns_delete";
            ops.store_sync(ns, "access_token", "a").unwrap();
            ops.store_sync(ns, "refresh_token", "b").unwrap();
            ops.delete_namespace_sync(ns).unwrap();
            assert_eq!(ops.retrieve_sync(ns, "access_token").unwrap(), None);
            assert_eq!(ops.retrieve_sync(ns, "refresh_token").unwrap(), None);
        }
    }

    /// #9493 regression: multiple `KeychainOps` instances over the SAME
    /// registry file (the real process has 5+) must not erase each other's
    /// registrations. These exercise `registry_apply` directly — the registry
    /// mutation seam — so no OS keychain access is needed (un-ignored).
    mod registry_last_writer_wins {
        use super::*;

        fn two_instances() -> (KeychainOps, KeychainOps, TempDir) {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("registry.json");
            let a = KeychainOps::new(path.clone()).expect("instance a");
            // B is constructed while the file is still empty — the boot-time
            // stale copy that used to clobber A's later registrations.
            let b = KeychainOps::new(path).expect("instance b");
            (a, b, dir)
        }

        #[test]
        fn interleaved_saves_preserve_each_others_registrations() {
            let (a, b, dir) = two_instances();
            a.registry_apply(RegistryOp::Add {
                namespace: "oneshim",
                key: "access_token",
            });
            // Pre-fix: B's save wrote its empty boot copy + its own key,
            // erasing the oneshim namespace A just registered.
            b.registry_apply(RegistryOp::Add {
                namespace: "openai",
                key: "refresh_token",
            });

            let disk = KeychainRegistry::load_or_default(&dir.path().join("registry.json"));
            assert!(
                disk.keys_for("oneshim").contains("access_token"),
                "instance B's save must not erase instance A's registration"
            );
            assert!(disk.keys_for("openai").contains("refresh_token"));
            // B's cache refreshed from the merged file — it can now enumerate
            // A's namespace too (delete_namespace inventory depends on this).
            assert!(b
                .registry
                .lock()
                .keys_for("oneshim")
                .contains("access_token"));
        }

        #[test]
        fn remove_only_removes_its_target_and_does_not_resurrect() {
            let (a, b, dir) = two_instances();
            a.registry_apply(RegistryOp::Add {
                namespace: "oneshim",
                key: "access_token",
            });
            b.registry_apply(RegistryOp::Add {
                namespace: "openai",
                key: "refresh_token",
            });
            // A removes its own key based on a reload — B's entry survives,
            // and A's removal is not resurrected by B's stale cache later.
            a.registry_apply(RegistryOp::Remove {
                namespace: "oneshim",
                key: "access_token",
            });

            let path = dir.path().join("registry.json");
            let disk = KeychainRegistry::load_or_default(&path);
            assert!(
                disk.keys_for("oneshim").is_empty(),
                "removed key must stay removed"
            );
            assert!(
                disk.keys_for("openai").contains("refresh_token"),
                "the other instance's registration must survive the removal save"
            );

            let removed: BTreeSet<String> = ["refresh_token".to_owned()].into();
            b.registry_apply(RegistryOp::RemoveMany {
                namespace: "openai",
                keys: &removed,
            });
            let disk = KeychainRegistry::load_or_default(&path);
            assert!(
                disk.all_namespaces().is_empty(),
                "RemoveMany must clear its namespace without resurrecting oneshim"
            );
        }

        /// #9523: the cross-process lock contract. SQLite locks are held per
        /// CONNECTION, so two connections in one test exercise exactly what
        /// two processes would — a second `BEGIN IMMEDIATE` must not succeed
        /// while the first transaction is open, and must succeed after it
        /// drops.
        #[test]
        fn xproc_lock_excludes_a_second_holder_until_released() {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("registry.json");

            let first = RegistryXprocLock::acquire(&path, std::time::Duration::from_secs(5))
                .expect("first holder must acquire");
            // Short timeout: the second holder must give up while the first
            // transaction is still open (best-effort None, never a deadlock).
            let contended =
                RegistryXprocLock::acquire(&path, std::time::Duration::from_millis(150));
            assert!(
                contended.is_none(),
                "second holder must not acquire while the first transaction is open"
            );

            drop(first);
            let after = RegistryXprocLock::acquire(&path, std::time::Duration::from_secs(5));
            assert!(
                after.is_some(),
                "lock must be re-acquirable after the first holder drops"
            );
        }

        /// #9523: the uncontended happy path — registry_apply works with the
        /// xproc guard wired in, and the lock db appears beside the registry.
        #[test]
        fn registry_apply_succeeds_with_xproc_lock_in_path() {
            let dir = TempDir::new().expect("temp dir");
            let path = dir.path().join("registry.json");
            let ops = KeychainOps::new(path.clone()).expect("instance");
            ops.registry_apply(RegistryOp::Add {
                namespace: "oneshim",
                key: "access_token",
            });
            let disk = KeychainRegistry::load_or_default(&path);
            assert!(disk.keys_for("oneshim").contains("access_token"));
            assert!(
                path.with_extension("lock.db").exists(),
                "xproc lock db must be created beside the registry"
            );
        }
    }
}
