//! #9588: bounded keyring primitives for [`crate::keychain::KeychainOps`].
//!
//! Every OS keychain touch (set/get/delete) runs through
//! [`maekon_core::bounded_op::run_keychain_bounded`] so a pending macOS ACL consent
//! prompt — e.g. a binary at a new path touching an item created by another
//! build — degrades into an explicit `StorageError` after a bounded wait
//! instead of hanging boot indefinitely. The `keyring::Entry` is constructed
//! INSIDE the worker thread so no keyring state needs to cross threads.
//!
//! Error message shapes are kept identical to the pre-#9588 inline calls
//! (`"keyring entry creation: …"` / `"keychain: …"`) so existing callers and
//! tests observe the same failures. Timeouts and wedge short-circuits map to
//! the dedicated [`StorageError::SecretStoreTimeout`] variant — "could not
//! reach the store" must stay distinguishable from "the store answered"
//! (review #9593 B1: master-key resolution fails closed on timeout).

use maekon_core::bounded_op::{run_keychain_bounded, BoundedOpError, DEFAULT_KEYCHAIN_OP_TIMEOUT};
use tracing::warn;

use crate::error::StorageError;

/// Pre-#9588 fixed service name. Flavored profiles used to share it; kept as
/// the read-through fallback so their already-sealed items stay reachable
/// after the service-name scoping (review #9593 I4 — without this, every
/// existing flavored profile fails closed at boot because its master key is
/// only ever under the legacy service).
pub(crate) const LEGACY_KEYCHAIN_SERVICE: &str = "maekon";

fn map_bounded_err(e: BoundedOpError) -> StorageError {
    match e {
        BoundedOpError::TimedOut { .. } | BoundedOpError::Wedged { .. } => {
            StorageError::SecretStoreTimeout(e.to_string())
        }
        BoundedOpError::SpawnFailed { .. } => StorageError::SecretStore(e.to_string()),
    }
}

fn entry_for(service: &str, user: &str) -> Result<keyring::Entry, StorageError> {
    keyring::Entry::new(service, user)
        .map_err(|e| StorageError::SecretStore(format!("keyring entry creation: {e}")))
}

/// Pure result mapping for get: `NoEntry` is a normal state → `Ok(None)`
/// (the pre-#9588 `retrieve_sync` contract).
fn map_get(res: Result<String, keyring::Error>) -> Result<Option<String>, StorageError> {
    match res {
        Ok(val) => Ok(Some(val)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(StorageError::SecretStore(format!("keychain: {e}"))),
    }
}

/// Pure result mapping for delete: `NoEntry` is tolerated (idempotent delete,
/// the pre-#9588 `delete_sync` contract).
fn map_delete(res: Result<(), keyring::Error>) -> Result<(), StorageError> {
    match res {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(StorageError::SecretStore(format!("keychain: {e}"))),
    }
}

/// Bounded `set_password`.
pub(crate) fn set_bounded(service: &str, user: &str, value: &str) -> Result<(), StorageError> {
    let (service, user, value) = (service.to_owned(), user.to_owned(), value.to_owned());
    run_keychain_bounded("keychain-set", DEFAULT_KEYCHAIN_OP_TIMEOUT, move || {
        entry_for(&service, &user)?
            .set_password(&value)
            .map_err(|e| StorageError::SecretStore(format!("keychain: {e}")))
    })
    .map_err(map_bounded_err)?
}

/// Bounded `get_password` (see [`map_get`] for the NoEntry contract).
pub(crate) fn get_bounded(service: &str, user: &str) -> Result<Option<String>, StorageError> {
    let (service, user) = (service.to_owned(), user.to_owned());
    run_keychain_bounded("keychain-get", DEFAULT_KEYCHAIN_OP_TIMEOUT, move || {
        Ok(entry_for(&service, &user)?.get_password()).and_then(map_get)
    })
    .map_err(map_bounded_err)?
}

/// Bounded `delete_credential` (see [`map_delete`] for the NoEntry contract).
pub(crate) fn delete_bounded(service: &str, user: &str) -> Result<(), StorageError> {
    let (service, user) = (service.to_owned(), user.to_owned());
    run_keychain_bounded("keychain-delete", DEFAULT_KEYCHAIN_OP_TIMEOUT, move || {
        Ok(entry_for(&service, &user)?.delete_credential()).and_then(map_delete)
    })
    .map_err(map_bounded_err)?
}

/// #9588 (review I4): whether a namespace participates in the legacy
/// read-through at all. ONLY the master-key namespace does (re-review N1):
/// its entries are data-dir-SHA-scoped, so a flavored profile's fallback read
/// (and forward-copy) can only ever touch that profile's OWN key. Every other
/// namespace — OAuth tokens, `identifier`/`organization_id` (PII per the
/// `KNOWN_OAUTH_KEYS` contract) — is account-shared with the unflavored
/// production app, so a fallback would read production secrets and a
/// forward-copy would duplicate them into every flavored service ever used.
/// Flavored profiles simply re-authenticate instead; only an undecryptable
/// database was the hard-break worth bridging.
fn legacy_fallback_applies(namespace: &str) -> bool {
    namespace == crate::encryption::MASTER_KEY_KEYCHAIN_NAMESPACE
}

/// #9588 (review I4): namespace-aware scoped read. For the master-key
/// namespace, a scoped miss falls back to [`LEGACY_KEYCHAIN_SERVICE`] and
/// forward-copies the value into the scoped service (best-effort — a failed
/// copy retries on the next read). The legacy item is deliberately LEFT IN
/// PLACE (other flavors may still read it); deletes stay scoped everywhere.
///
/// Returns `(value, migrated)`; `migrated` is true only when the forward-copy
/// into the scoped service succeeded (the caller then registers the key in
/// its enumeration registry).
pub(crate) fn get_bounded_for_namespace(
    service: &str,
    namespace: &str,
    user: &str,
) -> Result<(Option<String>, bool), StorageError> {
    // #9598 item 3: when a legacy read-through is possible, both reads run in
    // ONE bounded worker. Two sequential `get_bounded` calls each carried the
    // full `DEFAULT_KEYCHAIN_OP_TIMEOUT`, so a slow-but-not-yet-wedged keychain
    // cost up to 2× before this returned — and the latch could not help,
    // because it only short-circuits AFTER something has already timed out.
    // One worker means one budget for the pair.
    if !legacy_fallback_applies(namespace) || service == LEGACY_KEYCHAIN_SERVICE {
        // No read-through for this namespace: a single scoped read is the whole
        // operation, so the pre-existing single-budget path is already correct.
        return Ok((get_bounded(service, user)?, false));
    }

    let (svc, usr) = (service.to_owned(), user.to_owned());
    let (scoped, legacy) = run_keychain_bounded(
        "keychain-get-scoped-then-legacy",
        DEFAULT_KEYCHAIN_OP_TIMEOUT,
        move || -> Result<(Option<String>, Option<String>), StorageError> {
            let scoped = map_get(entry_for(&svc, &usr)?.get_password())?;
            if scoped.is_some() {
                // Hit on the scoped service — the legacy read is not just
                // unnecessary, it must not happen: reading it would be a
                // pointless extra OS touch on the common path.
                return Ok((scoped, None));
            }
            let legacy = map_get(entry_for(LEGACY_KEYCHAIN_SERVICE, &usr)?.get_password())?;
            Ok((None, legacy))
        },
    )
    .map_err(map_bounded_err)??;

    if let Some(value) = scoped {
        return Ok((Some(value), false));
    }

    match legacy {
        Some(value) => {
            let migrated = match set_bounded(service, user, &value) {
                Ok(()) => true,
                Err(e) => {
                    warn!(
                        "#9588: legacy keychain item found for '{user}' but the \
                         forward-copy into service '{service}' failed ({e}); \
                         serving the legacy value, copy retries on the next read"
                    );
                    false
                }
            };
            Ok((Some(value), migrated))
        }
        None => Ok((None, false)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // #9593 review I3: the semantic mappings the rewritten keychain ops rely
    // on, exercised WITHOUT a real OS keychain (the full-path tests over in
    // `keychain.rs` stay `#[ignore]`-gated real-backend tests).

    #[test]
    fn get_maps_no_entry_to_none() {
        assert_eq!(map_get(Err(keyring::Error::NoEntry)).unwrap(), None);
    }

    #[test]
    fn get_passes_values_through() {
        assert_eq!(
            map_get(Ok("secret".to_owned())).unwrap(),
            Some("secret".to_owned())
        );
    }

    #[test]
    fn get_wraps_other_errors_with_keychain_prefix() {
        let io = std::io::Error::other("backend down");
        let err = map_get(Err(keyring::Error::NoStorageAccess(Box::new(io)))).unwrap_err();
        match err {
            StorageError::SecretStore(msg) => {
                assert!(msg.starts_with("keychain: "), "pre-#9588 shape kept: {msg}")
            }
            other => panic!("expected SecretStore, got {other:?}"),
        }
    }

    #[test]
    fn delete_tolerates_no_entry() {
        map_delete(Err(keyring::Error::NoEntry)).unwrap();
        map_delete(Ok(())).unwrap();
    }

    #[test]
    fn delete_wraps_other_errors_with_keychain_prefix() {
        let io = std::io::Error::other("denied");
        let err = map_delete(Err(keyring::Error::NoStorageAccess(Box::new(io)))).unwrap_err();
        assert!(matches!(err, StorageError::SecretStore(_)));
    }

    #[test]
    fn legacy_fallback_is_master_key_namespace_only() {
        // Re-review N1: OAuth namespaces are account-shared with the
        // unflavored production app — a fallback there would read (and
        // forward-copy) production tokens and PII into flavored services.
        assert!(legacy_fallback_applies(
            crate::encryption::MASTER_KEY_KEYCHAIN_NAMESPACE
        ));
        assert!(!legacy_fallback_applies("openai"));
        assert!(!legacy_fallback_applies("oneshim/auth/default"));
    }

    /// #9598 item 3: the read-through pair must cost ONE timeout budget.
    ///
    /// A behavioural test would need a real, deliberately-slow OS keychain —
    /// which is exactly what this module's tests refuse to require (the
    /// real-backend cases live in `keychain.rs` behind `#[ignore]`). So the
    /// property is pinned structurally: `get_bounded_for_namespace` must issue
    /// exactly one bounded call on the read-through path.
    ///
    /// The regression this blocks is a silent one — reverting to two sequential
    /// `get_bounded` calls still passes every behavioural test and every type
    /// check, and only shows up as a doubled stall on a slow-but-not-yet-wedged
    /// keychain, where the latch cannot help because nothing has timed out yet.
    #[test]
    fn namespace_read_through_spends_a_single_bounded_budget() {
        let src = include_str!("keychain_guard.rs");
        let body = src
            .split("pub(crate) fn get_bounded_for_namespace(")
            .nth(1)
            .expect("the function must exist")
            .split("\n#[cfg(test)]")
            .next()
            .expect("body must end before the test module");

        assert_eq!(
            body.matches("run_keychain_bounded(").count(),
            1,
            "the scoped+legacy pair must run in one bounded worker — two calls \
             means two full timeouts on a slow keychain"
        );
        // …and the single-read early return must still delegate rather than
        // grow its own inline call.
        assert!(
            body.contains("get_bounded(service, user)?"),
            "the no-read-through path must keep using the shared single-read helper"
        );
    }

    #[test]
    fn bounded_timeout_maps_to_dedicated_timeout_variant() {
        // B1 core: "could not reach the store" must not collapse into the
        // generic SecretStore error that fallback paths treat as "store
        // absent".
        let timed_out = map_bounded_err(maekon_core::bounded_op::BoundedOpError::TimedOut {
            op: "keychain-get".into(),
            waited_secs: 15,
        });
        assert!(matches!(timed_out, StorageError::SecretStoreTimeout(_)));

        let wedged = map_bounded_err(maekon_core::bounded_op::BoundedOpError::Wedged {
            op: "keychain-get".into(),
        });
        assert!(matches!(wedged, StorageError::SecretStoreTimeout(_)));

        let spawn = map_bounded_err(maekon_core::bounded_op::BoundedOpError::SpawnFailed {
            op: "keychain-get".into(),
            reason: "no threads".into(),
        });
        assert!(matches!(spawn, StorageError::SecretStore(_)));
    }
}
