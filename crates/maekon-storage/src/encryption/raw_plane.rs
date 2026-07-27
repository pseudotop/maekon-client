//! Raw-plane per-account AEAD subkey derivation (ADR-030 §7 + Amendment I1, #8589).
//!
//! # Why a subkey, not the master key directly
//! ADR-030 §7 requires the raw payload plane to be AEAD-encrypted with an
//! "account/install-scoped key". Amendment I1 grounds that in the ONE key
//! infrastructure that exists — the whole-database `EncryptionKey([u8; 32])` —
//! by deriving an HKDF-SHA256 subkey rather than introducing a second key
//! manager.
//!
//! # Crypto-shred (the load-bearing privacy property)
//! The v51 schema gives every raw blob row its own `key_salt BLOB`. This module
//! generates that salt with the OS CSPRNG per row and feeds it as the **HKDF
//! salt**, folding `install_id` and `account_subject_ref` into the HKDF `info`.
//! The subkey is therefore underivable without the stored `key_salt`, so
//! **deleting the row (which destroys `key_salt`, `nonce`, and `ciphertext`
//! together) crypto-shreds that content with a plain `DELETE` — no database
//! rewrite or rekey** (ADR-030 §12 "clears raw content" on revoke/erase). This
//! is strictly stronger than a fixed `salt = install_id`, which would be
//! recomputable and could not be shredded by deleting a row alone; the migration
//! author's per-row `key_salt` column is honoured as the secret it must be.
//!
//! # Deviation from Amendment I1's literal formula (flag for review)
//! Amendment I1 writes `salt = install_id`, `info = "maekon.raw-plane.v1" ||
//! account_subject_ref`. We instead use `salt = key_salt` (random, per-row) and
//! `info = "maekon.raw-plane.v1" || install_id || account_subject_ref`. The
//! account/install binding I1 asks for is preserved (now in `info`), and the
//! crypto-shred property the same amendment demands is actually achievable. The
//! v51 migration's `key_salt BLOB NOT NULL` column only makes sense under this
//! reading.

use aes_gcm::aead::{Aead, Nonce};
use aes_gcm::{Aes256Gcm, KeyInit};
use hmac::digest::KeyInit as HmacKeyInit;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::EncryptionKey;
use crate::error::StorageError;

type HmacSha256 = Hmac<Sha256>;

/// HKDF domain separation tag for the raw plane (Amendment I1 `info` prefix).
const RAW_PLANE_INFO_TAG: &[u8] = b"maekon.raw-plane.v1\0";

/// Length of the random per-row HKDF salt.
const KEY_SALT_LEN: usize = 32;

/// A raw-plane ciphertext bundle: the three secret columns of one
/// `work_context_raw_blobs` row. Deleting all three crypto-shreds the content.
pub struct RawPlaneCiphertext {
    /// Random per-row HKDF salt — the secret that makes deletion a crypto-shred.
    pub key_salt: Vec<u8>,
    /// AES-256-GCM nonce (12 bytes).
    pub nonce: Vec<u8>,
    /// Ciphertext including the 16-byte GCM auth tag.
    pub ciphertext: Vec<u8>,
}

/// Length-prefix the account binding so `(install="ab", account="c")` and
/// `(install="a", account="bc")` cannot collide into the same `info`.
fn raw_plane_info(install_id: &str, account_subject_ref: &str) -> Vec<u8> {
    let mut info = Vec::with_capacity(
        RAW_PLANE_INFO_TAG.len() + 16 + install_id.len() + account_subject_ref.len(),
    );
    info.extend_from_slice(RAW_PLANE_INFO_TAG);
    info.extend_from_slice(&(install_id.len() as u64).to_be_bytes());
    info.extend_from_slice(install_id.as_bytes());
    info.extend_from_slice(&(account_subject_ref.len() as u64).to_be_bytes());
    info.extend_from_slice(account_subject_ref.as_bytes());
    info
}

/// HKDF-SHA256 (RFC 5869) for a single 32-byte output block.
///
/// extract: `PRK = HMAC-SHA256(salt, ikm)`; expand: `OKM = HMAC-SHA256(PRK,
/// info || 0x01)[..32]`. One block suffices for a 256-bit key.
fn hkdf_sha256_32(ikm: &[u8], salt: &[u8], info: &[u8]) -> Zeroizing<[u8; 32]> {
    // extract
    let mut extract = <HmacSha256 as HmacKeyInit>::new_from_slice(salt)
        .expect("HMAC-SHA256 accepts a key of any length");
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();

    // expand (single block, counter = 0x01)
    let mut expand = <HmacSha256 as HmacKeyInit>::new_from_slice(&prk)
        .expect("HMAC-SHA256 accepts a key of any length");
    expand.update(info);
    expand.update(&[0x01]);
    let okm = expand.finalize().into_bytes();

    let mut out = [0u8; 32];
    out.copy_from_slice(&okm[..32]);
    Zeroizing::new(out)
}

impl EncryptionKey {
    /// Encrypt a raw payload for one account with a freshly minted per-row salt
    /// (ADR-030 §7 + Amendment I1, #8589).
    ///
    /// Returns the three secret columns to persist. The subkey is derived,
    /// used, and zeroized inside this call — it is never stored.
    pub fn encrypt_raw_plane(
        &self,
        plaintext: &[u8],
        install_id: &str,
        account_subject_ref: &str,
    ) -> Result<RawPlaneCiphertext, StorageError> {
        let mut key_salt = vec![0u8; KEY_SALT_LEN];
        getrandom::fill(&mut key_salt)
            .map_err(|e| StorageError::Encryption(format!("raw-plane salt generation: {e}")))?;

        let info = raw_plane_info(install_id, account_subject_ref);
        let subkey = hkdf_sha256_32(self.as_bytes(), &key_salt, &info);

        let cipher = Aes256Gcm::new_from_slice(subkey.as_slice())
            .map_err(|e| StorageError::Encryption(format!("raw-plane cipher init: {e}")))?;

        let mut nonce_bytes = [0u8; 12];
        getrandom::fill(&mut nonce_bytes)
            .map_err(|e| StorageError::Encryption(format!("raw-plane nonce generation: {e}")))?;
        let nonce: Nonce<Aes256Gcm> = nonce_bytes.into();

        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|e| StorageError::Encryption(format!("raw-plane encrypt: {e}")))?;

        Ok(RawPlaneCiphertext {
            key_salt,
            nonce: nonce_bytes.to_vec(),
            ciphertext,
        })
    }

    /// Decrypt a raw payload previously produced by [`Self::encrypt_raw_plane`].
    ///
    /// The plaintext is returned inside `Zeroizing` so the recovered raw content
    /// is wiped from the heap when dropped. Fails closed if `key_salt` is absent
    /// (crypto-shredded) or the ciphertext does not authenticate.
    pub fn decrypt_raw_plane(
        &self,
        bundle: &RawPlaneCiphertext,
        install_id: &str,
        account_subject_ref: &str,
    ) -> Result<Zeroizing<Vec<u8>>, StorageError> {
        if bundle.nonce.len() != 12 {
            return Err(StorageError::Encryption(
                "raw-plane nonce must be 12 bytes".into(),
            ));
        }
        let info = raw_plane_info(install_id, account_subject_ref);
        let subkey = hkdf_sha256_32(self.as_bytes(), &bundle.key_salt, &info);

        let cipher = Aes256Gcm::new_from_slice(subkey.as_slice())
            .map_err(|e| StorageError::Encryption(format!("raw-plane cipher init: {e}")))?;
        let nonce = <&Nonce<Aes256Gcm>>::try_from(bundle.nonce.as_slice())
            .map_err(|e| StorageError::Encryption(format!("raw-plane nonce parse: {e}")))?;

        cipher
            .decrypt(nonce, bundle.ciphertext.as_slice())
            .map(Zeroizing::new)
            .map_err(|e| StorageError::Encryption(format!("raw-plane decrypt: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> EncryptionKey {
        EncryptionKey::from_bytes([7u8; 32])
    }

    #[test]
    fn round_trips_the_plaintext() {
        let k = key();
        let bundle = k
            .encrypt_raw_plane(b"secret provider body", "inst_1", "acct_1")
            .unwrap();
        let out = k.decrypt_raw_plane(&bundle, "inst_1", "acct_1").unwrap();
        assert_eq!(&out[..], b"secret provider body");
    }

    #[test]
    fn each_row_gets_a_distinct_random_salt_and_nonce() {
        let k = key();
        let a = k.encrypt_raw_plane(b"x", "inst_1", "acct_1").unwrap();
        let b = k.encrypt_raw_plane(b"x", "inst_1", "acct_1").unwrap();
        // Same plaintext, same account — but salts, nonces, and ciphertexts differ.
        assert_ne!(a.key_salt, b.key_salt);
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn destroying_the_salt_makes_the_ciphertext_undecryptable() {
        // Crypto-shred: without the stored key_salt the subkey is underivable, so
        // even the same master key + ciphertext + nonce cannot recover the content.
        let k = key();
        let mut bundle = k
            .encrypt_raw_plane(b"top secret", "inst_1", "acct_1")
            .unwrap();
        // Simulate the salt being gone (a leaked ciphertext copy without its row).
        bundle.key_salt = vec![0u8; KEY_SALT_LEN];
        let err = k
            .decrypt_raw_plane(&bundle, "inst_1", "acct_1")
            .unwrap_err();
        assert!(
            matches!(err, StorageError::Encryption(_)),
            "shredded-salt decrypt must fail closed, got {err:?}"
        );
    }

    #[test]
    fn a_different_account_binding_cannot_decrypt() {
        // info folds install_id + account_subject_ref, so account B cannot read
        // account A's blob even with the same master key and salt.
        let k = key();
        let bundle = k
            .encrypt_raw_plane(b"acct A only", "inst_1", "acct_A")
            .unwrap();
        let err = k
            .decrypt_raw_plane(&bundle, "inst_1", "acct_B")
            .unwrap_err();
        assert!(matches!(err, StorageError::Encryption(_)));
    }

    #[test]
    fn a_different_master_key_cannot_decrypt() {
        let a = EncryptionKey::from_bytes([1u8; 32]);
        let b = EncryptionKey::from_bytes([2u8; 32]);
        let bundle = a.encrypt_raw_plane(b"payload", "inst_1", "acct_1").unwrap();
        let err = b
            .decrypt_raw_plane(&bundle, "inst_1", "acct_1")
            .unwrap_err();
        assert!(matches!(err, StorageError::Encryption(_)));
    }

    #[test]
    fn length_prefixed_info_has_no_boundary_collision() {
        // ("ab","c") and ("a","bc") must derive different subkeys.
        let k = key();
        let salt = [9u8; KEY_SALT_LEN];
        let s1 = hkdf_sha256_32(k.as_bytes(), &salt, &raw_plane_info("ab", "c"));
        let s2 = hkdf_sha256_32(k.as_bytes(), &salt, &raw_plane_info("a", "bc"));
        assert_ne!(s1.as_slice(), s2.as_slice());
    }
}
