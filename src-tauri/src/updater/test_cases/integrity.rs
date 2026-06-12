use super::*;

#[test]
fn parse_sha256_manifest_validates_format() {
    let hash = Updater::parse_sha256_manifest(
        "8f434346648f6b96df89dda901c5176b10a6d83961fca6f18e40f9f0f84f2304  maekon.tar.gz",
    )
    .unwrap();
    assert_eq!(
        hash,
        "8f434346648f6b96df89dda901c5176b10a6d83961fca6f18e40f9f0f84f2304"
    );
}

#[test]
fn parse_sha256_manifest_rejects_invalid_hash() {
    let err = Updater::parse_sha256_manifest("not-a-valid-hash  maekon.tar.gz");
    assert!(matches!(err, Err(UpdateError::Integrity(_))));
}

#[test]
fn validate_download_url_rejects_http_and_unknown_host() {
    let updater = Updater::new(test_config());

    assert!(
        matches!(
            updater.validate_download_url("http://github.com/file.tar.gz").unwrap_err(),
            UpdateError::Download(_)
        ),
        "http URL must be rejected with UpdateError::Download"
    );

    assert!(
        matches!(
            updater.validate_download_url("https://evil.example.com/file.tar.gz").unwrap_err(),
            UpdateError::Download(_)
        ),
        "disallowed host must be rejected with UpdateError::Download"
    );
}

#[test]
fn extract_zip_rejects_path_traversal_entries() {
    use std::io::Write;

    let updater = Updater::new(test_config());
    let dir = tempdir().unwrap();
    let zip_path = dir.path().join("malicious.zip");

    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer.start_file("../../outside", options).unwrap();
        writer.write_all(b"malicious").unwrap();
        writer.finish().unwrap();
    }

    let result = updater.extract_zip(&zip_path);
    assert!(matches!(result, Err(UpdateError::Install(_))));
}

#[test]
fn verify_signature_accepts_valid_ed25519_signature() {
    use ed25519_dalek::{Signer, SigningKey};

    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let verifying_key = signing_key.verifying_key();

    let mut config = test_config();
    config.require_signature_verification = true;
    config.signature_public_key = BASE64.encode(verifying_key.as_bytes());
    let updater = Updater::new(config);

    let payload = b"maekon-release-artifact";
    let signature = signing_key.sign(payload);

    // verify_signature returns Ok(()) — the unit type — when ed25519 verification
    // passes.  There is no content to pin beyond the absence of an error.
    // The complementary reject test below pins the Err(Integrity) branch.
    // (#5594: ok-only IS the full contract for a passing signature)
    updater
        .verify_signature(payload, signature.to_bytes().as_slice())
        .expect("valid ed25519 signature over the exact payload must verify without error");
}

#[test]
fn verify_signature_rejects_invalid_signature() {
    use ed25519_dalek::{Signer, SigningKey};

    let signing_key = SigningKey::from_bytes(&[9u8; 32]);
    let verifying_key = signing_key.verifying_key();

    let mut config = test_config();
    config.require_signature_verification = true;
    config.signature_public_key = BASE64.encode(verifying_key.as_bytes());
    let updater = Updater::new(config);

    let payload = b"artifact-A";
    let signature = signing_key.sign(payload);

    let result = updater.verify_signature(b"artifact-B", signature.to_bytes().as_slice());
    assert!(matches!(result, Err(UpdateError::Integrity(_))));
}

// ── D9 multi-key trust tests ──────────────────────────────────────

#[test]
fn verify_signature_accepts_builtin_key() {
    use ed25519_dalek::{Signer, SigningKey};

    let signing_key = SigningKey::from_bytes(&[11u8; 32]);
    let builtin_key = BASE64.encode(signing_key.verifying_key().as_bytes());

    let payload = b"builtin-release-artifact";
    let signature = signing_key.sign(payload);

    // Inject a single-entry trusted array; no configured key override.
    let trusted = [builtin_key.as_str()];
    let result = Updater::verify_signature_with_keys(
        &trusted,
        None,
        payload,
        signature.to_bytes().as_slice(),
    );
    // verify_signature_with_keys returns Result<()>; Ok(()) means the payload was
    // accepted by a trusted key — there is no inner value to pin. (#5594: ok-only justified)
    result.expect("builtin key must accept a signature it generated over the exact payload");
}

#[test]
fn verify_signature_accepts_second_trusted_key_when_first_inactive() {
    use ed25519_dalek::{Signer, SigningKey};

    // First key in array is one we do NOT sign with.
    let first_key_unused = SigningKey::from_bytes(&[0u8; 32]).verifying_key();
    // Second key is the one that signs the payload.
    let second_key = SigningKey::from_bytes(&[12u8; 32]);

    let trusted_first = BASE64.encode(first_key_unused.as_bytes());
    let trusted_second = BASE64.encode(second_key.verifying_key().as_bytes());

    let payload = b"mid-rotation-artifact";
    let signature = second_key.sign(payload);

    let trusted = [trusted_first.as_str(), trusted_second.as_str()];
    let result = Updater::verify_signature_with_keys(
        &trusted,
        None,
        payload,
        signature.to_bytes().as_slice(),
    );
    // The second key in the trusted array must accept the signature even when the
    // first key does not match. (#5594: ok-only justified — Result<()> unit return)
    result.expect("second trusted key must validate mid-rotation payload even when first key is inactive");
}

#[test]
fn verify_signature_fallback_to_configured_key_when_not_in_array() {
    use ed25519_dalek::{Signer, SigningKey};

    let builtin = SigningKey::from_bytes(&[13u8; 32]).verifying_key();
    let configured = SigningKey::from_bytes(&[14u8; 32]);

    let trusted_only = BASE64.encode(builtin.as_bytes());
    let configured_b64 = BASE64.encode(configured.verifying_key().as_bytes());

    let payload = b"user-override-artifact";
    let signature = configured.sign(payload);

    // configured key is NOT in the trusted list → fallback should hit.
    let trusted = [trusted_only.as_str()];
    let result = Updater::verify_signature_with_keys(
        &trusted,
        Some(configured_b64.as_str()),
        payload,
        signature.to_bytes().as_slice(),
    );
    // The configured (user-override) key must succeed via the fallback path when it
    // is not in the builtin trusted array. (#5594: ok-only justified — Result<()>)
    result.expect("configured override key must validate payload via the D9 fallback path");
}

#[test]
fn verify_signature_rejects_payload_when_no_key_matches() {
    use ed25519_dalek::{Signer, SigningKey};

    let unknown = SigningKey::from_bytes(&[15u8; 32]);
    let payload = b"untrusted-artifact";
    let signature = unknown.sign(payload);

    // Provide a trusted list that does NOT include the unknown key, and
    // no configured override.
    let other = SigningKey::from_bytes(&[16u8; 32]).verifying_key();
    let trusted_entry = BASE64.encode(other.as_bytes());
    let trusted = [trusted_entry.as_str()];

    let result = Updater::verify_signature_with_keys(
        &trusted,
        None,
        payload,
        signature.to_bytes().as_slice(),
    );
    assert!(matches!(result, Err(UpdateError::Integrity(_))));
}

#[test]
fn validate_integrity_policy_allows_empty_public_key() {
    use maekon_core::config::UpdateConfig;

    let mut config = UpdateConfig {
        enabled: true,
        repo_owner: "pseudotop".to_string(),
        repo_name: "maekon-client".to_string(),
        check_interval_hours: 24,
        channel: UpdateChannel::default(),
        include_prerelease: false,
        auto_install: false,
        installation_id: Some("test-install-id".to_string()),
        require_signature_verification: true,
        signature_public_key: String::new(), // empty override — should NOT error
        min_allowed_version: None,
        max_allowed_version: None,
    };

    // validate_integrity_policy returns Result<()>; Ok(()) is the full success contract.
    // (#5594: ok-only justified — unit return type; complementary Err branch is pinned below)
    config
        .validate_integrity_policy()
        .expect("empty signature_public_key with updates enabled must be OK — D9 array is authoritative");

    // Also confirm a malformed non-empty override still errors.
    config.signature_public_key = "not-valid-base64!!!".to_string();
    config.validate_integrity_policy()
        .expect_err("malformed signature_public_key must return an Err from validate_integrity_policy");
}
