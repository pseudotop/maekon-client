//! Shared serialization for vault mirror cycles and Art.17 erasure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;

type RootLock = Arc<tokio::sync::Mutex<()>>;
type RootLockRegistry = Mutex<HashMap<PathBuf, RootLock>>;

/// Writer instances are interchangeable, so instance-level state cannot
/// serialize them. Cycles sharing a canonical root must share one lock to
/// protect the temporary file and its hash row. Erasure uses the same lock
/// so generated writes cannot interleave with Art.17 deletion.
static VAULT_ROOT_LOCKS: OnceLock<RootLockRegistry> = OnceLock::new();

pub(super) fn root_lock(canonical_root: &Path) -> Result<RootLock, CoreError> {
    root_lock_in(
        VAULT_ROOT_LOCKS.get_or_init(|| Mutex::new(HashMap::new())),
        canonical_root,
    )
}

fn root_lock_in(registry: &RootLockRegistry, canonical_root: &Path) -> Result<RootLock, CoreError> {
    // #12031: never replace a poisoned registry with independent root locks.
    // Propagate a coarse error; formatting the poisoned guard could disclose
    // user-owned vault paths to logs or an erase outcome.
    let mut guard = registry.lock().map_err(|_| CoreError::Internal {
        code: InternalCode::Generic,
        message: "Vault root lock registry poisoned".to_string(),
    })?;
    Ok(Arc::clone(
        guard
            .entry(canonical_root.to_path_buf())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_root_shares_exclusion_across_lookups() {
        let registry = Mutex::new(HashMap::new());
        let first = root_lock_in(&registry, Path::new("root-a")).unwrap();
        let second = root_lock_in(&registry, Path::new("root-a")).unwrap();
        let held = first.try_lock().unwrap();
        assert_eq!(
            second.try_lock().unwrap_err().to_string(),
            "operation would block"
        );
        drop(held);
        drop(second.try_lock().expect("released root must be available"));
    }

    #[test]
    fn different_roots_do_not_block_each_other() {
        let registry = Mutex::new(HashMap::new());
        let first = root_lock_in(&registry, Path::new("root-a")).unwrap();
        let second = root_lock_in(&registry, Path::new("root-b")).unwrap();
        let _held = first.try_lock().unwrap();
        drop(
            second
                .try_lock()
                .expect("another root must remain available"),
        );
    }

    #[test]
    fn process_registry_shares_the_same_root_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let first = root_lock(&root).unwrap();
        let second = root_lock(&root).unwrap();
        let _held = first.try_lock().unwrap();
        assert_eq!(
            second.try_lock().unwrap_err().to_string(),
            "operation would block"
        );
    }

    #[test]
    fn poisoned_registry_returns_coarse_error_without_recovery() {
        // Poison only an isolated registry; parallel vault tests keep their
        // process-global registry intact.
        let registry = Arc::new(Mutex::new(HashMap::new()));
        let original = root_lock_in(&registry, Path::new("private-root")).unwrap();
        let worker_registry = Arc::clone(&registry);
        let panic = std::thread::spawn(move || {
            let _held = worker_registry.lock().unwrap();
            panic!("injected vault registry poison");
        })
        .join()
        .unwrap_err();
        assert_eq!(
            panic.downcast_ref::<&str>(),
            Some(&"injected vault registry poison")
        );
        assert!(registry.is_poisoned());

        for path in ["private-root", "new-private-root"] {
            let error = root_lock_in(&registry, Path::new(path)).unwrap_err();
            assert!(matches!(
                error,
                CoreError::Internal { code: InternalCode::Generic, ref message }
                    if message == "Vault root lock registry poisoned"
            ));
            assert!(registry.is_poisoned());
        }
        // Test-only inspection proves no replacement lock or extra entry was
        // minted after failure. Production never recovers this poisoned guard.
        let guard = registry.lock().unwrap_err().into_inner();
        assert_eq!(guard.len(), 1);
        assert!(Arc::ptr_eq(
            guard.get(Path::new("private-root")).unwrap(),
            &original
        ));
    }
}
