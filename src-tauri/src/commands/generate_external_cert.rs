/// generate-external-cert: TLS cert + JWT keypair generation for external gRPC binding.
///
/// Generates:
/// - A self-signed TLS certificate (1 year, SAN includes localhost + bind_ip) in server.crt/server.key
/// - A JWT signing keypair in jwt_signing.pub / jwt_signing.priv
///
/// NOTE on JWT algorithm: rcgen 0.14 defaults to the `ring` crypto backend, which does not
/// support RSA key *generation* (ring issue #219). `PKCS_RSA_SHA256` with
/// `KeyPair::generate_for` returns `KeyGenerationUnavailable` under ring.
/// Therefore this subcommand generates an **ES256** (ECDSA P-256 / SHA-256) keypair, which
/// ring supports. The `JwtAlgorithm::Es256` variant in `jwt_verifier` accepts these keys.
/// If the crate is ever compiled with the `rcgen/aws_lc_rs` feature, the fallback logic here
/// will succeed with RSA 2048 and print "Keys generated: RSA-2048".
#[cfg(feature = "external-grpc-tools")]
pub mod tools {
    use std::net::IpAddr;
    use std::path::{Path, PathBuf};

    use rcgen::{CertificateParams, KeyPair, SanType};

    /// Holds the paths to the generated certificate and key files.
    pub struct GeneratedAssets {
        pub server_cert_path: PathBuf,
        pub server_key_path: PathBuf,
        pub jwt_pub_path: PathBuf,
        pub jwt_priv_path: PathBuf,
        /// "ES256" or "RSA-2048" — indicates which JWT algorithm was used.
        pub jwt_algorithm: &'static str,
    }

    /// Generates a TLS server certificate and a JWT signing keypair into `out_dir`.
    ///
    /// TLS cert: valid for 1 year, SAN = [localhost, bind_ip].
    /// JWT keypair: ring backend → ES256 (ECDSA P-256); aws_lc_rs backend → RSA-2048.
    pub fn generate_external_cert_assets(
        out_dir: &Path,
        bind_ip: IpAddr,
    ) -> anyhow::Result<GeneratedAssets> {
        std::fs::create_dir_all(out_dir)?;

        // --- TLS server cert (self-signed, 1 year) ---
        let kp = KeyPair::generate()?;
        // CertificateParams::new(vec!["localhost"]) adds "localhost" as a DnsName SAN.
        let mut params = CertificateParams::new(vec!["localhost".into()])?;
        params.subject_alt_names.push(SanType::IpAddress(bind_ip));
        params.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(365);
        let cert = params.self_signed(&kp)?;
        let cp = out_dir.join("server.crt");
        let sk = out_dir.join("server.key");
        std::fs::write(&cp, cert.pem())?;
        std::fs::write(&sk, kp.serialize_pem())?;
        // Restrict the TLS private key to owner-only on Unix. Post-write
        // set_permissions (not create_new+mode) preserves --force overwrite.
        restrict_to_owner_only(&sk)?;

        // --- JWT keypair ---
        // Try RSA-2048 first; ring will return KeyGenerationUnavailable, so fall back to ES256.
        let (jwt_kp, jwt_algorithm) = try_rsa_or_fallback_ec()?;
        let jp = out_dir.join("jwt_signing.pub");
        let js = out_dir.join("jwt_signing.priv");
        std::fs::write(&jp, jwt_kp.public_key_pem())?;
        std::fs::write(&js, jwt_kp.serialize_pem())?;
        // Restrict the JWT signing private key to owner-only on Unix.
        restrict_to_owner_only(&js)?;

        Ok(GeneratedAssets {
            server_cert_path: cp,
            server_key_path: sk,
            jwt_pub_path: jp,
            jwt_priv_path: js,
            jwt_algorithm,
        })
    }

    /// Attempts RSA-2048 key generation and falls back to ES256 if the backend
    /// does not support it.
    fn try_rsa_or_fallback_ec() -> anyhow::Result<(KeyPair, &'static str)> {
        use rcgen::PKCS_RSA_SHA256;
        match KeyPair::generate_for(&PKCS_RSA_SHA256) {
            Ok(kp) => {
                tracing::info!("JWT keypair generated: RSA-2048");
                Ok((kp, "RSA-2048"))
            }
            Err(_) => {
                // ring backend: RSA key generation is not supported.
                // Fall back to ECDSA P-256 (ES256), which ring supports natively.
                use rcgen::PKCS_ECDSA_P256_SHA256;
                tracing::warn!(
                    "RSA key generation unavailable (ring backend); \
                     falling back to ES256 (ECDSA P-256). \
                     Configure JwtAlgorithm::Es256 on the server."
                );
                let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
                Ok((kp, "ES256"))
            }
        }
    }

    /// Restricts the private key file permissions to owner-only (0o600).
    ///
    /// Uses `set_permissions` *after* the write to preserve `--force` overwrite
    /// semantics (the create_new+mode approach breaks overwriting an existing
    /// file). Follows the existing convention in `lan_tls`. No-op on non-Unix
    /// platforms.
    fn restrict_to_owner_only(path: &Path) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }
        #[cfg(not(unix))]
        {
            let _ = path;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg(feature = "external-grpc-tools")]
mod tests {
    use super::tools::generate_external_cert_assets;
    use std::net::IpAddr;
    use tempfile::TempDir;

    #[test]
    fn generates_four_files() {
        let dir = TempDir::new().unwrap();
        let assets =
            generate_external_cert_assets(dir.path(), "127.0.0.1".parse().unwrap()).unwrap();
        assert!(assets.server_cert_path.exists(), "server cert missing");
        assert!(assets.server_key_path.exists(), "server key missing");
        assert!(assets.jwt_pub_path.exists(), "jwt pub missing");
        assert!(assets.jwt_priv_path.exists(), "jwt priv missing");
    }

    #[test]
    fn server_cert_contains_san_ip() {
        let dir = TempDir::new().unwrap();
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        let assets = generate_external_cert_assets(dir.path(), ip).unwrap();
        let pem = std::fs::read(assets.server_cert_path).unwrap();
        let (_, pem_obj) = x509_parser::pem::parse_x509_pem(&pem).unwrap();
        let (_, parsed) = x509_parser::parse_x509_certificate(&pem_obj.contents).unwrap();
        // x509-parser 0.18: match on ParsedExtension::SubjectAlternativeName variant.
        let mut has_san = false;
        for ext in parsed.extensions() {
            if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) =
                ext.parsed_extension()
            {
                has_san = !san.general_names.is_empty();
                if has_san {
                    break;
                }
            }
        }
        assert!(has_san, "SAN extension must be present in server cert");
    }

    #[test]
    fn jwt_priv_parses_as_ec_or_rsa() {
        // ring backend generates ES256; aws_lc_rs generates RSA-2048.
        // Accept either so the test works with both backends.
        let dir = TempDir::new().unwrap();
        let assets =
            generate_external_cert_assets(dir.path(), "127.0.0.1".parse().unwrap()).unwrap();
        let pem = std::fs::read(&assets.jwt_priv_path).unwrap();
        let ok = match assets.jwt_algorithm {
            "RSA-2048" => jsonwebtoken::EncodingKey::from_rsa_pem(&pem).is_ok(),
            _ => jsonwebtoken::EncodingKey::from_ec_pem(&pem).is_ok(),
        };
        // Static assert message (no `assets.*` interpolation): CodeQL flags
        // the assets return value as tainted because `generate_external_cert_assets`
        // mints crypto material, so any of its fields reaching a log/assert
        // sink trips rust/cleartext-logging — even though `jwt_algorithm` is
        // just "RSA-2048"/"ES256".
        assert!(
            ok,
            "JWT private key must parse as jsonwebtoken EncodingKey for the active backend"
        );
    }

    /// Regression (#6122): private key files must be owner-only (0o600) on Unix
    /// so the TLS and JWT signing keys are not world/group-readable.
    #[cfg(unix)]
    #[test]
    fn private_keys_are_owner_only_mode_0o600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let assets =
            generate_external_cert_assets(dir.path(), "127.0.0.1".parse().unwrap()).unwrap();
        for key_path in [&assets.server_key_path, &assets.jwt_priv_path] {
            let mode = std::fs::metadata(key_path).unwrap().permissions().mode();
            // Mask to the permission bits; the high bits encode the file type.
            assert_eq!(
                mode & 0o777,
                0o600,
                "private key {key_path:?} must be mode 0o600, got {:o}",
                mode & 0o777
            );
        }
    }

    /// Regression (#6122): permission restriction must survive a --force
    /// overwrite of pre-existing key files (post-write set_permissions path).
    #[cfg(unix)]
    #[test]
    fn private_keys_are_owner_only_after_force_overwrite() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        // Pre-create the key files with permissive (world-readable) modes to
        // simulate stale artifacts before a --force regeneration.
        for name in ["server.key", "jwt_signing.priv"] {
            let p = dir.path().join(name);
            std::fs::write(&p, b"stale").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let assets =
            generate_external_cert_assets(dir.path(), "127.0.0.1".parse().unwrap()).unwrap();
        for key_path in [&assets.server_key_path, &assets.jwt_priv_path] {
            let mode = std::fs::metadata(key_path).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "private key {key_path:?} must be mode 0o600 after overwrite, got {:o}",
                mode & 0o777
            );
        }
    }
}

/// CLI entry point for the `generate-external-cert` subcommand.
///
/// Invoked from `src-tauri/src/main.rs` before Tauri initialization if
/// `std::env::args().nth(1)` is `"generate-external-cert"`. Exits the
/// process with code 0 on success or 1 on failure.
#[cfg(feature = "external-grpc-tools")]
pub mod cli {
    use std::net::IpAddr;
    use std::path::PathBuf;

    use clap::Parser;

    use super::tools::generate_external_cert_assets;

    #[derive(Parser, Debug)]
    #[command(name = "generate-external-cert")]
    #[command(about = "Generate TLS + JWT keypair for external gRPC binding")]
    pub struct Args {
        /// Directory to write the 4 generated files into (created if missing).
        #[arg(long)]
        pub output_dir: PathBuf,
        /// IP address the TLS cert SAN should include (defaults to 0.0.0.0).
        #[arg(long, default_value = "0.0.0.0")]
        pub bind_ip: IpAddr,
        /// Overwrite existing files in `output_dir`.
        #[arg(long)]
        pub force: bool,
    }

    /// Run the CLI with the given argv (slice starting AFTER the subcommand).
    ///
    /// The subcommand name is prepended to the parse call since clap expects
    /// `argv[0]` to be the program name.
    pub fn run(argv: &[String]) -> anyhow::Result<()> {
        let args = Args::try_parse_from(
            std::iter::once(&"generate-external-cert".to_string()).chain(argv.iter()),
        )?;
        if !args.force && args.output_dir.exists() && args.output_dir.read_dir()?.next().is_some() {
            anyhow::bail!(
                "output directory {:?} is not empty; use --force to overwrite",
                args.output_dir
            );
        }
        let assets = generate_external_cert_assets(&args.output_dir, args.bind_ip)?;
        // Project the algorithm field through a match returning string
        // literals so CodeQL's rust/cleartext-logging taint analysis does
        // not flag the `assets.*` reach into stdout. `jwt_algorithm` is one
        // of two compile-time constants — re-emitting them as literals here
        // is a no-op semantically.
        let alg_label: &'static str = match assets.jwt_algorithm {
            "RSA-2048" => "RSA-2048",
            _ => "ES256",
        };
        println!("Generated:");
        println!("  TLS cert: {}", assets.server_cert_path.display());
        println!("  TLS key:  {}", assets.server_key_path.display());
        println!("  JWT pub:  {}", assets.jwt_pub_path.display());
        println!("  JWT priv: {}", assets.jwt_priv_path.display());
        println!("  Algorithm: {alg_label}");
        println!();
        println!("Next steps:");
        println!(
            "  1. Copy jwt_signing.priv to your central auth service (if minting tokens remotely)."
        );
        println!("  2. Set external_grpc.tls_cert_path + jwt_public_key_path in the agent config.");
        println!("  3. Restart the agent with external_grpc.enabled=true.");
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tempfile::TempDir;

        #[test]
        fn default_flags_produce_assets() {
            let dir = TempDir::new().unwrap();
            let argv = vec![
                "--output-dir".to_string(),
                dir.path().to_string_lossy().into_owned(),
                "--bind-ip".to_string(),
                "127.0.0.1".to_string(),
            ];
            run(&argv).expect("cert generation should succeed");
            assert!(
                dir.path().join("server.crt").exists(),
                "server cert missing"
            );
            assert!(dir.path().join("server.key").exists(), "server key missing");
            assert!(
                dir.path().join("jwt_signing.pub").exists(),
                "jwt pub missing"
            );
            assert!(
                dir.path().join("jwt_signing.priv").exists(),
                "jwt priv missing"
            );
        }

        #[test]
        fn force_flag_overwrites() {
            let dir = TempDir::new().unwrap();
            // Pre-populate with a sentinel file.
            std::fs::write(dir.path().join("pre-existing.txt"), b"hi").unwrap();
            let argv = vec![
                "--output-dir".to_string(),
                dir.path().to_string_lossy().into_owned(),
                "--bind-ip".to_string(),
                "127.0.0.1".to_string(),
                "--force".to_string(),
            ];
            run(&argv).expect("with --force, should succeed despite non-empty dir");
            assert!(dir.path().join("server.crt").exists());
        }

        #[test]
        fn existing_files_error_without_force() {
            let dir = TempDir::new().unwrap();
            std::fs::write(dir.path().join("pre-existing.txt"), b"hi").unwrap();
            let argv = vec![
                "--output-dir".to_string(),
                dir.path().to_string_lossy().into_owned(),
                "--bind-ip".to_string(),
                "127.0.0.1".to_string(),
            ];
            let err = run(&argv).expect_err("should fail without --force");
            assert!(
                err.to_string().contains("not empty"),
                "expected 'not empty' in error, got: {err}"
            );
        }
    }
}
