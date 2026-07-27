//! Test suite for `encryption/mod.rs`, split out per the ADR-013 LOC growth
//! gate (`crates/maekon-lint/adr013_loc_baseline.json` / ADR-003 Directory
//! Module Pattern) — mirrors the `crates/maekon-analysis/src/adaptive_search`
//! mod.rs+tests.rs split. Included via `#[path = "tests.rs"] mod tests;` in
//! `mod.rs`, so this file itself IS the `encryption::tests` module; `use
//! super::*` below therefore resolves against `encryption`.

use super::*;
use std::fs;
use tempfile::TempDir;

fn fixture_key(fill: u8) -> EncryptionKey {
    EncryptionKey::from_bytes(std::array::from_fn(|_| fill))
}

/// Regression for #6242: the master key must wipe its bytes on drop. This is
/// a compile-time guarantee — if the `ZeroizeOnDrop` derive is ever removed
/// from `EncryptionKey`, this bound fails to compile.
#[test]
fn master_key_is_zeroize_on_drop() {
    fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    assert_zeroize_on_drop::<EncryptionKey>();
}

/// Regression for #6242: transient plaintext / hex copies of secret material
/// are returned inside `Zeroizing` so they wipe on drop. The explicit type
/// annotations pin the wrapper — dropping it back to a bare `String`/`Vec<u8>`
/// breaks this test.
#[test]
fn secret_outputs_are_zeroizing_wrapped() {
    let key = fixture_key(0x42);

    let hex: zeroize::Zeroizing<String> = key.as_hex();
    assert_eq!(hex.len(), 64);

    let encrypted = key.encrypt(b"top secret").unwrap();
    let plaintext: zeroize::Zeroizing<Vec<u8>> = key.decrypt(&encrypted).unwrap();
    assert_eq!(&plaintext[..], b"top secret");
}

#[test]
fn generates_32_byte_key() {
    let dir = TempDir::new().unwrap();
    let key = EncryptionKey::load_or_create(dir.path()).unwrap();
    assert_eq!(key.as_bytes().len(), 32);
}

#[test]
fn hex_is_64_chars() {
    let dir = TempDir::new().unwrap();
    let key = EncryptionKey::load_or_create(dir.path()).unwrap();
    assert_eq!(key.as_hex().len(), 64);
    assert!(key.as_hex().chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn load_returns_same_key_as_generated() {
    let dir = TempDir::new().unwrap();
    let key1 = EncryptionKey::load_or_create(dir.path()).unwrap();
    let key2 = EncryptionKey::load_or_create(dir.path()).unwrap();
    // `as_hex` returns `Zeroizing<String>`; compare the inner strings.
    assert_eq!(key1.as_hex().as_str(), key2.as_hex().as_str());
}

#[test]
fn key_file_created_with_correct_size() {
    let dir = TempDir::new().unwrap();
    EncryptionKey::load_or_create(dir.path()).unwrap();
    let content = fs::read(dir.path().join(".db_key")).unwrap();
    assert_eq!(content.len(), 32);
}

#[test]
fn debug_does_not_leak_key_bytes() {
    let key = fixture_key(0xAB);
    let debug_str = format!("{key:?}");
    assert!(!debug_str.contains("AB"));
    assert!(debug_str.contains("redacted"));
}

#[test]
fn encrypt_decrypt_round_trip() {
    let key = fixture_key(0x42);
    let plaintext = b"Hello, MAEKON frame data!";

    let encrypted = key.encrypt(plaintext).unwrap();
    // encrypted = 12-byte nonce + ciphertext + 16-byte auth tag
    assert!(encrypted.len() > plaintext.len());
    assert_ne!(&encrypted[12..], plaintext);

    let decrypted = key.decrypt(&encrypted).unwrap();
    // `decrypted` is `Zeroizing<Vec<u8>>`; compare as slices.
    assert_eq!(&decrypted[..], &plaintext[..]);
}

#[test]
fn encrypt_produces_different_ciphertexts() {
    let key = fixture_key(0x42);
    let plaintext = b"same input";

    let enc1 = key.encrypt(plaintext).unwrap();
    let enc2 = key.encrypt(plaintext).unwrap();
    // Different random nonces produce different ciphertexts
    assert_ne!(enc1, enc2);

    // Both decrypt to the same plaintext (decrypt yields Zeroizing<Vec<u8>>).
    assert_eq!(&key.decrypt(&enc1).unwrap()[..], &plaintext[..]);
    assert_eq!(&key.decrypt(&enc2).unwrap()[..], &plaintext[..]);
}

#[test]
fn decrypt_with_wrong_key_fails() {
    let key1 = fixture_key(0x42);
    let key2 = fixture_key(0x43);
    let plaintext = b"secret data";

    let encrypted = key1.encrypt(plaintext).unwrap();
    assert!(
        matches!(
            key2.decrypt(&encrypted).unwrap_err(),
            StorageError::Encryption(_)
        ),
        "wrong key must yield StorageError::Encryption (AES-GCM auth failure)"
    );
}

#[test]
fn decrypt_too_short_data_fails() {
    let key = fixture_key(0x42);
    assert!(
        matches!(
            key.decrypt(&[0u8; 5]).unwrap_err(),
            StorageError::Encryption(_)
        ),
        "ciphertext shorter than nonce must yield StorageError::Encryption"
    );
}

#[test]
fn decrypt_corrupted_data_fails() {
    let key = fixture_key(0x42);
    let mut encrypted = key.encrypt(b"test data").unwrap();
    // Corrupt a byte in the ciphertext region
    if encrypted.len() > 15 {
        encrypted[15] ^= 0xFF;
    }
    assert!(
        matches!(
            key.decrypt(&encrypted).unwrap_err(),
            StorageError::Encryption(_)
        ),
        "corrupted ciphertext must yield StorageError::Encryption (auth tag mismatch)"
    );
}

#[test]
fn encrypt_empty_data() {
    let key = fixture_key(0x42);
    let encrypted = key.encrypt(b"").unwrap();
    // 12 nonce + 16 auth tag = 28 bytes minimum
    assert_eq!(encrypted.len(), 28);
    let decrypted = key.decrypt(&encrypted).unwrap();
    assert!(decrypted.is_empty());
}

#[test]
fn encrypt_large_data() {
    let key = fixture_key(0x42);
    let plaintext = vec![0xAB_u8; 1024 * 1024]; // 1 MB

    let encrypted = key.encrypt(&plaintext).unwrap();
    let decrypted = key.decrypt(&encrypted).unwrap();
    // `decrypted` is `Zeroizing<Vec<u8>>`; compare as slices.
    assert_eq!(&decrypted[..], &plaintext[..]);
}

/// Verify that the key file is created with mode 0o600 (owner read/write only),
/// with no world-readable window at any point (TOCTOU fix, issue #5991).
#[cfg(unix)]
#[test]
fn key_file_created_with_mode_0o600() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    EncryptionKey::load_or_create(dir.path()).unwrap();

    let key_path = dir.path().join(".db_key");
    let metadata = fs::metadata(&key_path).unwrap();
    // Mask to the permission bits only (drop file type bits).
    let mode = metadata.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "key file must be created with mode 0o600, got 0o{mode:o}"
    );
}

/// Verify that `save_to_file` recovers when the target file already exists
/// (the `AlreadyExists` branch of the atomic `create_new` path) and the
/// recreated file still has mode 0o600 — exercised by calling `save_to_file`
/// directly twice on the same path (`load_or_create` cannot reach this branch
/// because it short-circuits on `key_path.exists()`).
#[cfg(unix)]
#[test]
fn key_file_save_over_existing_keeps_mode_0o600() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".db_key");

    // First write creates the file via create_new(true).
    EncryptionKey::from_bytes([1u8; 32])
        .save_to_file(&path)
        .unwrap();

    // Second write finds the file present -> hits the AlreadyExists branch
    // (remove + recreate). It must still land mode 0o600.
    EncryptionKey::from_bytes([2u8; 32])
        .save_to_file(&path)
        .unwrap();

    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "key file must keep mode 0o600 after the AlreadyExists recreate branch, got 0o{mode:o}"
    );
}

/// #7101: the storage `set_owner_only_dacl` shim must succeed by delegating to
/// the single canonical primitive in `maekon-core` (no duplicate copy here).
/// CI-verified on the Windows runner; it does not compile on Unix.
#[cfg(windows)]
#[test]
fn set_owner_only_dacl_delegates_to_core() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("dacl_target.tmp");
    std::fs::File::create(&path).unwrap();
    set_owner_only_dacl(&path)
        .expect("storage shim must delegate to core and apply the owner-only DACL");
}

/// #8040: `EncryptionKey::load_or_create_sealed`'s migration/verification/
/// fallback decision tree, driven against a manual in-memory `MasterKeyVault`
/// fake (ADR-001 §5: no mockall) — never a real OS keychain (see
/// `MasterKeyVault`'s doc comment for why: `cargo test` must not depend on an
/// interactive OS keychain prompt or an unlocked login session). The real
/// `KeychainOps` backend is covered by `keychain.rs`'s `#[ignore]`d
/// integration tests, which this module deliberately mirrors in policy.
#[cfg(test)]
mod sealed_key_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Manual in-memory fake `MasterKeyVault`. `force_store_err` /
    /// `force_retrieve_err` simulate "keychain unavailable" (headless
    /// Linux/CI, locked keychain, transient backend failure, etc.) without a
    /// real backend. `corrupt_readback` simulates a write that "succeeds" but
    /// whose readback does not match — the case `seal()`'s round-trip check
    /// must catch.
    #[derive(Default)]
    struct FakeVault {
        entries: Mutex<HashMap<(String, String), String>>,
        force_store_err: bool,
        force_retrieve_err: bool,
        corrupt_readback: bool,
    }

    impl MasterKeyVault for FakeVault {
        fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError> {
            if self.force_store_err {
                return Err(StorageError::SecretStore("fake store failure".into()));
            }
            self.entries
                .lock()
                .unwrap()
                .insert((namespace.to_string(), key.to_string()), value.to_string());
            Ok(())
        }

        fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, StorageError> {
            if self.force_retrieve_err {
                return Err(StorageError::SecretStore("fake retrieve failure".into()));
            }
            let stored = self
                .entries
                .lock()
                .unwrap()
                .get(&(namespace.to_string(), key.to_string()))
                .cloned();
            if self.corrupt_readback {
                // Only corrupt a readback for an entry that was actually
                // written — an entry-doesn't-exist-yet `None` must stay
                // `None` so the "no keychain entry" branch is still reached
                // (only the post-`store()` verification read inside `seal()`
                // should observe a mismatched, well-formed (64 hex chars)
                // value).
                return Ok(stored.map(|_| "ab".repeat(32)));
            }
            Ok(stored)
        }

        fn delete(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
            self.entries
                .lock()
                .unwrap()
                .remove(&(namespace.to_string(), key.to_string()));
            Ok(())
        }
    }

    #[test]
    fn fresh_install_seals_new_key_in_keychain_no_file_written() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault::default();

        let key = EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();

        assert!(
            !dir.path().join(".db_key").exists(),
            "no plaintext key file should be written when the keychain succeeds"
        );
        let entry = master_key_keychain_entry(dir.path());
        let stored = vault
            .retrieve(MASTER_KEY_KEYCHAIN_NAMESPACE, &entry)
            .unwrap()
            .unwrap();
        assert_eq!(stored.as_str(), key.as_hex().as_str());
    }

    #[test]
    fn existing_keychain_entry_is_loaded_and_used() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault::default();
        let entry = master_key_keychain_entry(dir.path());
        let seeded = EncryptionKey::from_bytes([7u8; 32]);
        vault
            .store(
                MASTER_KEY_KEYCHAIN_NAMESPACE,
                &entry,
                seeded.as_hex().as_str(),
            )
            .unwrap();

        let key = EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();
        assert_eq!(key.as_hex().as_str(), seeded.as_hex().as_str());
    }

    #[test]
    fn legacy_plaintext_file_migrates_to_keychain_and_file_is_deleted() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault::default();
        // Seed a legacy plaintext .db_key (pre-#8040 scheme).
        let legacy = EncryptionKey::from_bytes([9u8; 32]);
        legacy.save_to_file(&dir.path().join(".db_key")).unwrap();
        assert!(dir.path().join(".db_key").exists());

        let key = EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();

        assert_eq!(
            key.as_hex().as_str(),
            legacy.as_hex().as_str(),
            "the migrated key must be byte-identical to the legacy file key"
        );
        assert!(
            !dir.path().join(".db_key").exists(),
            "the legacy plaintext file must be deleted after a VERIFIED migration"
        );
        let entry = master_key_keychain_entry(dir.path());
        assert_eq!(
            vault
                .retrieve(MASTER_KEY_KEYCHAIN_NAMESPACE, &entry)
                .unwrap()
                .unwrap()
                .as_str(),
            legacy.as_hex().as_str()
        );
    }

    /// R: existing user data must remain decryptable — a failed migration
    /// must NEVER delete the plaintext file (no data loss).
    #[test]
    fn migration_write_failure_keeps_plaintext_file_no_data_loss() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault {
            force_store_err: true, // simulate a keychain write failure mid-migration
            ..Default::default()
        };
        let legacy = EncryptionKey::from_bytes([3u8; 32]);
        legacy.save_to_file(&dir.path().join(".db_key")).unwrap();

        let key = EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();

        assert_eq!(
            key.as_hex().as_str(),
            legacy.as_hex().as_str(),
            "must continue decrypting with the existing file key on a failed migration"
        );
        assert!(
            dir.path().join(".db_key").exists(),
            "the legacy plaintext file must NOT be deleted when migration fails"
        );
    }

    /// A keychain write that "succeeds" but whose immediate readback does not
    /// match must be treated as a failed migration (not silently trusted).
    #[test]
    fn migration_readback_mismatch_keeps_plaintext_file() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault {
            corrupt_readback: true,
            ..Default::default()
        };
        let legacy = EncryptionKey::from_bytes([5u8; 32]);
        legacy.save_to_file(&dir.path().join(".db_key")).unwrap();

        let key = EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();

        assert_eq!(key.as_hex().as_str(), legacy.as_hex().as_str());
        assert!(
            dir.path().join(".db_key").exists(),
            "an unverified (readback-mismatched) migration must not delete the plaintext file"
        );
    }

    #[test]
    fn keychain_unavailable_falls_back_to_plaintext_file_scheme() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault {
            force_retrieve_err: true, // simulate "no keychain backend" (headless Linux/CI)
            ..Default::default()
        };

        let key = EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();

        assert!(
            dir.path().join(".db_key").exists(),
            "the plaintext key-file scheme must be used when the keychain itself is unavailable"
        );
        let reloaded = EncryptionKey::load_or_create(dir.path()).unwrap();
        assert_eq!(key.as_hex().as_str(), reloaded.as_hex().as_str());
    }

    #[test]
    fn keychain_seal_failure_on_fresh_install_falls_back_to_file() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault {
            force_store_err: true,
            ..Default::default()
        };

        EncryptionKey::load_or_create_sealed(dir.path(), &vault).unwrap();

        assert!(
            dir.path().join(".db_key").exists(),
            "a fresh install must fall back to the plaintext key file when sealing fails"
        );
    }

    #[test]
    fn keychain_entry_is_scoped_per_data_dir() {
        let dir_a = TempDir::new().unwrap();
        let dir_b = TempDir::new().unwrap();
        assert_ne!(
            master_key_keychain_entry(dir_a.path()),
            master_key_keychain_entry(dir_b.path()),
            "distinct data dirs must not collide on the same keychain entry"
        );
    }

    /// A corrupted (malformed hex or wrong length) keychain entry must fail
    /// closed, not silently regenerate a fresh key that would desynchronize
    /// from the SQLCipher database / encrypted frame files already keyed on
    /// the original master key.
    #[test]
    fn corrupt_keychain_entry_fails_closed() {
        let dir = TempDir::new().unwrap();
        let vault = FakeVault::default();
        let entry = master_key_keychain_entry(dir.path());
        vault
            .store(MASTER_KEY_KEYCHAIN_NAMESPACE, &entry, "not-valid-hex")
            .unwrap();

        let result = EncryptionKey::load_or_create_sealed(dir.path(), &vault);
        // #5631: `.unwrap_err()` + a variant/message assertion instead of a
        // value-blind `is_err()` hedge — pins that this fails specifically via
        // `from_hex_string`'s hex-decode rejection (StorageError::Encryption),
        // not merely "some Err arrived" (which could mask a regression that
        // silently regenerates a key through a different, unintended path).
        let err = result.unwrap_err();
        assert!(
            matches!(err, StorageError::Encryption(ref msg) if msg.contains("hex decode")),
            "a corrupt keychain entry must fail closed via the hex-decode rejection path, not \
             silently regenerate a new key; got: {err:?}"
        );
    }
}
