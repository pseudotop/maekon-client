//! Tests: error display, sha256, signature verification, URL allowlist, archive safety.
use crate::updater::*;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use maekon_core::config::{UpdateChannel, UpdateConfig};
use tempfile::tempdir;

fn test_config() -> UpdateConfig {
    UpdateConfig {
        enabled: true,
        repo_owner: "test-owner".to_string(),
        repo_name: "test-repo".to_string(),
        check_interval_hours: 24,
        channel: UpdateChannel::default(),
        include_prerelease: false,
        auto_install: false,
        installation_id: Some("test-install-00000000-0000-0000-0000-000000000000".to_string()),
        require_signature_verification: false,
        signature_public_key: String::new(),
        min_allowed_version: None,
        max_allowed_version: None,
    }
}

#[test]
fn error_display_messages() {
    let errors = vec![
        UpdateError::Disabled,
        UpdateError::AlreadyLatest,
        UpdateError::NoSuitableAsset,
        UpdateError::UnsupportedPlatform("test".to_string()),
        UpdateError::ParseResponse("test".to_string()),
        UpdateError::Download("test".to_string()),
        UpdateError::Install("test".to_string()),
    ];
    for error in errors {
        let msg = format!("{}", error);
        assert!(!msg.is_empty());
    }
}

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
            updater
                .validate_download_url("http://github.com/file.tar.gz")
                .unwrap_err(),
            UpdateError::Download(_)
        ),
        "http URL must be rejected with UpdateError::Download"
    );
    assert!(
        matches!(
            updater
                .validate_download_url("https://evil.example.com/file.tar.gz")
                .unwrap_err(),
            UpdateError::Download(_)
        ),
        "disallowed host must be rejected with UpdateError::Download"
    );
}

#[test]
fn validate_metadata_base_url_rejects_non_https_and_unknown_host() {
    let updater = Updater::new(test_config());
    updater
        .validate_metadata_base_url("https://api.github.com")
        .expect("GitHub API HTTPS metadata base URL must be accepted");
    updater
        .validate_metadata_base_url("http://127.0.0.1:12345")
        .expect("localhost HTTP metadata base URL must be accepted only in tests");

    assert!(matches!(
        updater
            .validate_metadata_base_url("http://api.github.com")
            .unwrap_err(),
        UpdateError::Download(_)
    ));
    assert!(matches!(
        updater
            .validate_metadata_base_url("https://evil.example.com")
            .unwrap_err(),
        UpdateError::Download(_)
    ));
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
    // verify_signature returns Result<()>; Ok(()) means ed25519 verification passed
    // for the exact payload. No inner value to pin. (#5594: ok-only justified)
    updater
        .verify_signature(payload, signature.to_bytes().as_slice())
        .expect("valid ed25519 signature over the configured public key's payload must verify");
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

#[test]
fn verify_signature_accepts_builtin_key() {
    use ed25519_dalek::{Signer, SigningKey};
    let signing_key = SigningKey::from_bytes(&[11u8; 32]);
    let builtin_key = BASE64.encode(signing_key.verifying_key().as_bytes());
    let payload = b"builtin-release-artifact";
    let signature = signing_key.sign(payload);
    let trusted = [builtin_key.as_str()];
    let source = Updater::verify_signature_source_with_keys(
        &trusted,
        None,
        payload,
        signature.to_bytes().as_slice(),
    )
    .expect("builtin key must report its key-source for audit");
    assert_eq!(source, SignatureKeySource::BuiltInTrusted { index: 0 });

    let result = Updater::verify_signature_with_keys(
        &trusted,
        None,
        payload,
        signature.to_bytes().as_slice(),
    );
    // Result<()> — Ok(()) is the full contract; the reject path is pinned in the
    // sibling _rejects_ test. (#5594: ok-only justified — unit return type)
    result.expect("builtin key must accept a signature it generated over the exact payload");
}

#[test]
fn verify_signature_accepts_second_trusted_key_when_first_inactive() {
    use ed25519_dalek::{Signer, SigningKey};
    let first_key_unused = SigningKey::from_bytes(&[0u8; 32]).verifying_key();
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
    // The second key in the array must accept the signature even when the first key
    // does not match — critical for key rotation correctness. (#5594: ok-only justified)
    result.expect(
        "second trusted key must validate mid-rotation payload even when first key is inactive",
    );
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
    let trusted = [trusted_only.as_str()];
    let source = Updater::verify_signature_source_with_keys(
        &trusted,
        Some(configured_b64.as_str()),
        payload,
        signature.to_bytes().as_slice(),
    )
    .expect("configured override must report its key-source for audit");
    assert_eq!(source, SignatureKeySource::ConfiguredOverride);

    let result = Updater::verify_signature_with_keys(
        &trusted,
        Some(configured_b64.as_str()),
        payload,
        signature.to_bytes().as_slice(),
    );
    // The user-configured key must succeed via the D9 fallback path when not in the
    // builtin array. (#5594: ok-only justified — Result<()> unit return)
    result.expect("configured override key must validate payload via the D9 fallback path");
}

#[test]
fn verify_signature_prefers_builtin_source_when_configured_key_duplicates() {
    use ed25519_dalek::{Signer, SigningKey};
    let signing_key = SigningKey::from_bytes(&[18u8; 32]);
    let key_b64 = BASE64.encode(signing_key.verifying_key().as_bytes());
    let trusted = [key_b64.as_str()];
    let payload = b"duplicate-configured-key-artifact";
    let signature = signing_key.sign(payload);

    let source = Updater::verify_signature_source_with_keys(
        &trusted,
        Some(key_b64.as_str()),
        payload,
        signature.to_bytes().as_slice(),
    )
    .expect("duplicate configured key must still verify through the builtin path");

    assert_eq!(source, SignatureKeySource::BuiltInTrusted { index: 0 });
}

#[test]
fn verify_signature_rejects_payload_when_no_key_matches() {
    use ed25519_dalek::{Signer, SigningKey};
    let unknown = SigningKey::from_bytes(&[15u8; 32]);
    let payload = b"untrusted-artifact";
    let signature = unknown.sign(payload);
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
        signature_public_key: String::new(),
        min_allowed_version: None,
        max_allowed_version: None,
    };
    // validate_integrity_policy returns Result<()>; Ok(()) is the full success contract.
    // (#5594: ok-only justified — unit return; complementary Err variant pinned below)
    config
        .validate_integrity_policy()
        .expect("empty signature_public_key must be OK — D9 builtin array is authoritative");
    config.signature_public_key = "not-valid-base64!!!".to_string();
    config.validate_integrity_policy().expect_err(
        "malformed signature_public_key must return an Err from validate_integrity_policy",
    );
}

#[test]
fn sha256_verification_correct_file() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("artifact.bin");
    let content = b"maekon release artifact payload v42";
    std::fs::write(&file_path, content).unwrap();
    let file_bytes = std::fs::read(&file_path).unwrap();
    let computed_hash = Updater::sha256_hex(&file_bytes);
    assert_eq!(computed_hash.len(), 64);
    assert!(computed_hash.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(computed_hash, Updater::sha256_hex(&file_bytes));
}

#[test]
fn sha256_verification_detects_corruption() {
    let hash_original = Updater::sha256_hex(b"genuine release artifact");
    let hash_corrupted = Updater::sha256_hex(b"corrupted release artifact");
    assert_ne!(
        hash_original, hash_corrupted,
        "different content must produce different hashes"
    );
}

#[test]
fn safe_archive_path_rejects_traversal() {
    use std::path::Path;
    assert!(!Updater::is_safe_archive_path(Path::new(
        "../../../etc/passwd"
    )));
    assert!(!Updater::is_safe_archive_path(Path::new("foo/../../bar")));
    assert!(!Updater::is_safe_archive_path(Path::new("../outside")));
    assert!(Updater::is_safe_archive_path(Path::new("bin/maekon")));
    assert!(Updater::is_safe_archive_path(Path::new("maekon")));
    assert!(Updater::is_safe_archive_path(Path::new("./maekon")));
    assert!(Updater::is_safe_archive_path(Path::new(
        "release/bin/maekon"
    )));
}

#[test]
fn url_allowlist_accepts_github_rejects_unknown() {
    assert!(Updater::is_allowed_download_host("github.com"));
    assert!(Updater::is_allowed_download_host("api.github.com"));
    assert!(Updater::is_allowed_download_host(
        "objects.githubusercontent.com"
    ));
    assert!(Updater::is_allowed_download_host("githubusercontent.com"));
    assert!(!Updater::is_allowed_download_host("evil.com"));
    assert!(!Updater::is_allowed_download_host("not-github.com"));
    assert!(!Updater::is_allowed_download_host("github.com.evil.net"));
    assert!(!Updater::is_allowed_download_host("malicious.example.org"));
}

#[test]
fn url_allowlist_full_url_validation() {
    let updater = Updater::new(test_config());
    // validate_download_url returns Result<()>; Ok(()) is the full allowlist-pass contract.
    // (#5594: ok-only justified — unit return; reject variants are pinned below)
    updater
        .validate_download_url(
            "https://github.com/pseudotop/maekon-client/releases/download/v1.0.0/asset.tar.gz",
        )
        .expect("github.com HTTPS release URL must pass the download URL allowlist");
    updater
        .validate_download_url("https://objects.githubusercontent.com/github-releases/asset.tar.gz")
        .expect("objects.githubusercontent.com HTTPS URL must pass the download URL allowlist");
    assert!(matches!(
        updater
            .validate_download_url("https://evil.com/malware.tar.gz")
            .unwrap_err(),
        UpdateError::Download(_)
    ));
    assert!(matches!(
        updater
            .validate_download_url("https://not-github.com/fake.tar.gz")
            .unwrap_err(),
        UpdateError::Download(_)
    ));
    assert!(matches!(
        updater
            .validate_download_url("http://github.com/asset.tar.gz")
            .unwrap_err(),
        UpdateError::Download(_)
    ));
}
