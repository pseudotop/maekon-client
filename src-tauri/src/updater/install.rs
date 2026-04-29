//! Download, decompress, binary replacement, restart.
#![allow(dead_code)] // Install helpers called from updater apply/verify paths

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use super::{UpdateError, Updater};

/// Phase 4 D11: exit code used by `execute_rollback` when it must terminate
/// the current process (either Windows helper path or any Unix error after
/// the image-replacement syscall would have fired).
///
/// `75` (EX_TEMPFAIL, from `sysexits(3)`) signals a "temporary failure;
/// try again" — appropriate for rollback: the current binary failed to
/// stay healthy, but user data / install dir is intact and the previous
/// binary was restored.
pub const ROLLBACK_EXIT_CODE: i32 = 75;

impl Updater {
    /// Apply a delta patch: read current binary, apply bsdiff patch, verify checksum.
    ///
    /// Returns the path to the patched binary (written to a temp file).
    pub async fn apply_delta_update(
        &self,
        patch_path: &Path,
        full_binary_checksum: &str,
    ) -> Result<PathBuf, UpdateError> {
        let current_binary = super::delta::current_binary_path()?;
        let old_bytes = tokio::fs::read(&current_binary)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to read current binary: {e}")))?;
        let patch_bytes = tokio::fs::read(patch_path)
            .await
            .map_err(|e| UpdateError::Install(format!("Failed to read patch file: {e}")))?;

        let new_bytes = super::delta::apply_patch(&old_bytes, &patch_bytes)?;

        // Write patched binary to temp
        let temp_dir = std::env::temp_dir();
        let unique = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let temp_path = temp_dir.join(format!("oneshim-{unique}-patched"));
        std::fs::write(&temp_path, &new_bytes)?;

        // Verify against FULL binary checksum
        let actual_hash = Self::sha256_hex(&new_bytes);
        if actual_hash != full_binary_checksum {
            let _ = std::fs::remove_file(&temp_path);
            return Err(UpdateError::Integrity(format!(
                "Patched binary checksum mismatch: expected={full_binary_checksum}, actual={actual_hash}"
            )));
        }

        tracing::info!("Delta update applied: {:?}", temp_path);
        Ok(temp_path)
    }

    pub async fn download_update(&self, download_url: &str) -> Result<PathBuf, UpdateError> {
        let validated_url = self.validate_download_url(download_url)?;
        tracing::info!("Starting update download: {}", validated_url);

        let response = self.http_client.get(validated_url.clone()).send().await?;

        if !response.status().is_success() {
            return Err(UpdateError::Download(format!(
                "Download failed: HTTP {}",
                response.status()
            )));
        }

        let bytes = response.bytes().await?;

        let expected_hash = self.fetch_expected_sha256(&validated_url).await?;
        let actual_hash = Self::sha256_hex(&bytes);
        if actual_hash != expected_hash {
            return Err(UpdateError::Integrity(format!(
                "Checksum mismatch: expected={}, actual={}",
                expected_hash, actual_hash
            )));
        }

        if self.config.require_signature_verification {
            let signature = self.fetch_signature(&validated_url).await?;
            self.verify_signature(&bytes, &signature)?;
        }

        let temp_dir = std::env::temp_dir();
        let file_name = validated_url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("oneshim-update")
            .trim();
        let unique = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let temp_path = temp_dir.join(format!("oneshim-{unique}-{file_name}"));

        let mut outfile = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        outfile.write_all(&bytes)?;
        outfile.sync_all()?;

        tracing::info!("Update download completed: {:?}", temp_path);
        Ok(temp_path)
    }

    /// 실시간 진행 상황을 보고하면서 업데이트를 스트리밍 방식으로 다운로드한다.
    /// 전체 파일을 메모리에 로드하는 대신 청크 단위로 디스크에 기록한다.
    pub async fn download_update_with_progress(
        &self,
        download_url: &str,
        progress_tx: tokio::sync::watch::Sender<oneshim_api_contracts::update::DownloadProgress>,
    ) -> Result<PathBuf, UpdateError> {
        let validated_url = self.validate_download_url(download_url)?;
        tracing::info!("Starting streaming download: {}", validated_url);

        let response = self.http_client.get(validated_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Download(format!(
                "Download failed: HTTP {}",
                response.status()
            )));
        }

        let total_bytes = response.content_length().unwrap_or(0);
        let mut downloaded: u64 = 0;

        // 임시 파일 생성 (동기 I/O — 기존 패턴 유지)
        let temp_dir = std::env::temp_dir();
        let file_name = validated_url
            .path_segments()
            .and_then(|mut s| s.next_back())
            .unwrap_or("oneshim-update")
            .trim();
        let unique = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let temp_path = temp_dir.join(format!("oneshim-{unique}-{file_name}"));

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;

        // 스트리밍 청크 수신 및 디스크 기록
        let mut stream = response.bytes_stream();
        while let Some(chunk_result) = stream.next().await {
            let chunk =
                chunk_result.map_err(|e| UpdateError::Download(format!("Stream error: {e}")))?;
            file.write_all(&chunk)?;
            downloaded += chunk.len() as u64;
            let percent = if total_bytes > 0 {
                (downloaded as f32 / total_bytes as f32) * 100.0
            } else {
                0.0
            };
            let _ = progress_tx.send(oneshim_api_contracts::update::DownloadProgress {
                bytes_downloaded: downloaded,
                total_bytes,
                percent,
            });
        }
        file.sync_all()?;
        drop(file);

        // 검증을 위해 파일 재읽기
        let bytes = std::fs::read(&temp_path)?;

        // 체크섬 검증
        let expected_hash = self.fetch_expected_sha256(&validated_url).await?;
        let actual_hash = Self::sha256_hex(&bytes);
        if actual_hash != expected_hash {
            let _ = std::fs::remove_file(&temp_path);
            return Err(UpdateError::Integrity(format!(
                "Checksum mismatch: expected={expected_hash}, actual={actual_hash}"
            )));
        }

        // 서명 검증
        if self.config.require_signature_verification {
            let signature = self.fetch_signature(&validated_url).await?;
            self.verify_signature(&bytes, &signature)?;
        }

        tracing::info!(
            "Streaming download completed: {:?} ({downloaded} bytes)",
            temp_path
        );
        Ok(temp_path)
    }

    pub(super) async fn fetch_signature(
        &self,
        download_url: &reqwest::Url,
    ) -> Result<Vec<u8>, UpdateError> {
        let sig_url = reqwest::Url::parse(&format!("{}.sig", download_url))
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse signature URL: {}", e)))?;

        self.validate_download_url(sig_url.as_str())?;

        let response = self.http_client.get(sig_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Integrity(format!(
                "Failed to download signature file: HTTP {} ({})",
                response.status(),
                sig_url
            )));
        }

        let body = response.bytes().await?;
        let body = String::from_utf8(body.to_vec()).map_err(|e| {
            UpdateError::Integrity(format!("Invalid signature file encoding: {}", e))
        })?;

        let sig_b64 = body
            .split_whitespace()
            .next()
            .ok_or_else(|| UpdateError::Integrity("Signature file is empty".to_string()))?;

        BASE64.decode(sig_b64).map_err(|e| {
            UpdateError::Integrity(format!("Failed to decode signature base64: {}", e))
        })
    }

    /// Verify the Ed25519 signature of `payload` against any trusted key.
    ///
    /// D9 multi-key trust:
    /// 1. Walk the built-in `TRUSTED_PUBLIC_KEYS` array first (primary
    ///    trust source — rotation story lives here).
    /// 2. If no built-in key validates, fall back to `config.signature_public_key`
    ///    IF it is non-empty AND different from every built-in key (a
    ///    genuine user override, e.g., dev self-signing).
    ///
    /// Returns `Integrity` error when no trusted key validates.
    pub(super) fn verify_signature(
        &self,
        payload: &[u8],
        signature_bytes: &[u8],
    ) -> Result<(), UpdateError> {
        let configured = self
            .config
            .signature_public_key
            .split_whitespace()
            .next()
            .filter(|k| !k.trim().is_empty());
        Self::verify_signature_with_keys(
            super::trusted_keys::TRUSTED_PUBLIC_KEYS,
            configured,
            payload,
            signature_bytes,
        )
    }

    /// Inner verification helper with an explicit trusted-key list + optional
    /// configured-key override. Extracted so tests can supply an arbitrary
    /// trusted list without mutating the production `const` array.
    pub(super) fn verify_signature_with_keys(
        trusted: &[&str],
        configured: Option<&str>,
        payload: &[u8],
        signature_bytes: &[u8],
    ) -> Result<(), UpdateError> {
        // Normalize signature bytes once (same across all key attempts).
        let signature_array: [u8; 64] = signature_bytes.try_into().map_err(|_| {
            UpdateError::Integrity(format!(
                "Invalid signature length: {} bytes (expected 64)",
                signature_bytes.len()
            ))
        })?;
        let signature = Signature::from_bytes(&signature_array);

        // (1) Try every built-in trusted key.
        for (idx, key_b64) in trusted.iter().enumerate() {
            if Self::try_verify_with_key_b64(key_b64, payload, &signature).is_ok() {
                if idx > 0 {
                    tracing::info!(
                        "signature validated by trusted key #{idx} (rotation in progress)"
                    );
                }
                return Ok(());
            }
        }

        // (2) Fall back to the user-configured key if present AND
        //     genuinely distinct from any built-in key.
        if let Some(configured_key) = configured {
            let already_tried = trusted.contains(&configured_key);
            if !already_tried
                && Self::try_verify_with_key_b64(configured_key, payload, &signature).is_ok()
            {
                tracing::warn!("signature validated via user-configured key (override)");
                return Ok(());
            }
        }

        Err(UpdateError::Integrity(
            "no trusted key validated the signature".into(),
        ))
    }

    /// Try a single base64-encoded 32-byte public key. Returns Ok on successful
    /// verification; any parse/validation failure is an Err but callers treat
    /// it as "next key please" — only the absence of any successful key is
    /// surfaced as an integrity error (by the caller).
    fn try_verify_with_key_b64(
        key_b64: &str,
        payload: &[u8],
        signature: &Signature,
    ) -> Result<(), UpdateError> {
        let key_bytes = BASE64.decode(key_b64).map_err(|e| {
            UpdateError::Integrity(format!("Failed to decode public key base64: {}", e))
        })?;
        let key_len = key_bytes.len();
        let key_array: [u8; 32] = key_bytes.try_into().map_err(|_| {
            UpdateError::Integrity(format!(
                "Invalid public key length: {} bytes (expected 32)",
                key_len
            ))
        })?;
        let public_key = VerifyingKey::from_bytes(&key_array)
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse public key: {}", e)))?;
        public_key
            .verify(payload, signature)
            .map_err(|e| UpdateError::Integrity(format!("Signature verification failed: {}", e)))
    }

    pub(super) async fn fetch_expected_sha256(
        &self,
        download_url: &reqwest::Url,
    ) -> Result<String, UpdateError> {
        let checksum_url = reqwest::Url::parse(&format!("{}.sha256", download_url))
            .map_err(|e| UpdateError::Integrity(format!("Failed to parse checksum URL: {}", e)))?;

        self.validate_download_url(checksum_url.as_str())?;

        let response = self.http_client.get(checksum_url.clone()).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Integrity(format!(
                "Failed to download checksum file: HTTP {} ({})",
                response.status(),
                checksum_url
            )));
        }

        let body = response.bytes().await?;
        let body = String::from_utf8(body.to_vec()).map_err(|e| {
            UpdateError::Integrity(format!("Invalid checksum file encoding: {}", e))
        })?;

        Self::parse_sha256_manifest(&body)
    }

    pub(super) fn parse_sha256_manifest(content: &str) -> Result<String, UpdateError> {
        let hash = content
            .split_whitespace()
            .next()
            .ok_or_else(|| UpdateError::Integrity("Checksum file is empty".to_string()))?
            .to_ascii_lowercase();

        let is_hex = hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit());
        if !is_hex {
            return Err(UpdateError::Integrity(format!(
                "Invalid SHA-256 format: {}",
                hash
            )));
        }

        Ok(hash)
    }

    pub(super) fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher.finalize().iter().fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }

    pub(super) fn validate_download_url(&self, url: &str) -> Result<reqwest::Url, UpdateError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| UpdateError::Download(format!("Failed to parse download URL: {}", e)))?;

        let Some(host) = parsed.host_str() else {
            return Err(UpdateError::Download(
                "Download URL host is missing".to_string(),
            ));
        };

        if parsed.scheme() != "https" {
            #[cfg(test)]
            if parsed.scheme() == "http" && matches!(host, "localhost" | "127.0.0.1") {
                // Local test server is allowed for deterministic updater tests.
            } else {
                return Err(UpdateError::Download(format!(
                    "Only HTTPS download URLs are allowed: {}",
                    parsed
                )));
            }

            #[cfg(not(test))]
            return Err(UpdateError::Download(format!(
                "Only HTTPS download URLs are allowed: {}",
                parsed
            )));
        }

        if !Self::is_allowed_download_host(host) {
            return Err(UpdateError::Download(format!(
                "Disallowed download host: {}",
                host
            )));
        }

        Ok(parsed)
    }

    pub(super) fn is_allowed_download_host(host: &str) -> bool {
        let allowlisted = Self::ALLOWED_DOWNLOAD_HOSTS.iter().any(|allowed_host| {
            host == *allowed_host || host.ends_with(&format!(".{}", allowed_host))
        });
        if allowlisted {
            return true;
        }

        #[cfg(test)]
        {
            matches!(host, "localhost" | "127.0.0.1")
        }

        #[cfg(not(test))]
        {
            false
        }
    }

    pub(super) fn is_safe_archive_path(path: &Path) -> bool {
        path.components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
    }

    pub(super) fn backup_path_for(current_exe: &Path) -> Result<PathBuf, UpdateError> {
        let parent = current_exe.parent().ok_or_else(|| {
            UpdateError::Install(
                "Failed to locate parent directory of current executable".to_string(),
            )
        })?;

        let file_name = current_exe
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("oneshim")
            .to_string();
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        Ok(parent.join(format!("{}.rollback.{}", file_name, ts)))
    }

    pub(super) fn install_and_restart_with_ops<FReplace, FRestart>(
        &self,
        downloaded_path: &Path,
        current_exe: &Path,
        new_version: Option<&str>,
        mut replace_binary: FReplace,
        mut restart_app: FRestart,
    ) -> Result<(), UpdateError>
    where
        FReplace: FnMut(&Path) -> Result<(), UpdateError>,
        FRestart: FnMut() -> Result<(), UpdateError>,
    {
        tracing::info!("Starting update installation: {:?}", downloaded_path);

        let backup_path = Self::backup_path_for(current_exe)?;
        std::fs::copy(current_exe, &backup_path)?;

        let file_name = downloaded_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        let binary_path = match Self::extract_if_archive(self, downloaded_path, file_name) {
            Ok(p) => p,
            Err(e) => {
                // Task 6 D11 (Phase 4): orphan-backup cleanup. Earlier-step
                // failures (extract) leave `{binary}.rollback.{ts}` unused —
                // remove it before returning the original error.
                let _ = std::fs::remove_file(&backup_path);
                return Err(e);
            }
        };

        if let Err(e) = replace_binary(&binary_path) {
            let _ = std::fs::remove_file(&backup_path);
            return Err(e);
        }

        // Task 6 D11: write .install_pending_{NEW_VERSION} immediately after
        // replace_binary succeeds and BEFORE restart_app, so the probe on the
        // next boot has a deterministic backup_path + previous_version.
        if let Some(new_ver) = new_version {
            let current_exe_parent = current_exe.parent().ok_or_else(|| {
                UpdateError::Install(
                    "current_exe has no parent directory for install_pending".to_string(),
                )
            })?;
            if let Err(e) = Self::write_install_pending(
                current_exe_parent,
                new_ver,
                super::CURRENT_VERSION,
                &backup_path,
            ) {
                tracing::error!("write_install_pending failed: {e}");
                // On pending-write failure, attempt restoration using the same
                // platform mechanism as execute_rollback (Unix rename; Windows
                // spike deliverable — Task 12 stubs it). The backup still
                // exists since replace_binary succeeded. We leave the user on
                // the new binary for now; the probe will NOT trigger rollback
                // (no .install_pending_ file), but user-triggered manual
                // downgrade remains available.
                tracing::warn!(
                    "D11 probe for this install is disabled (pending marker absent). \
                     Backup retained at {:?}",
                    backup_path
                );
                return Err(e);
            }
        }

        tracing::info!("Update installation completed, restarting application...");

        match restart_app() {
            Ok(()) => Ok(()),
            Err(restart_err) => {
                tracing::error!(
                    "Restart failed, attempting rollback: backup={:?}, error={}",
                    backup_path,
                    restart_err
                );

                match replace_binary(&backup_path) {
                    Ok(()) => Err(UpdateError::Install(format!(
                        "Rollback completed after restart failure: {}",
                        restart_err
                    ))),
                    Err(rollback_err) => Err(UpdateError::Install(format!(
                        "Restart failed and rollback failed: restart={}, rollback={}",
                        restart_err, rollback_err
                    ))),
                }
            }
        }
    }

    /// Decompress archive (tar.gz / zip) or return path as-is for loose binaries.
    /// Factored out so call sites can match against archive-extraction failures
    /// and clean up the orphan backup before returning the error.
    fn extract_if_archive(
        updater: &Self,
        downloaded_path: &Path,
        file_name: &str,
    ) -> Result<PathBuf, UpdateError> {
        if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
            updater.extract_tar_gz(downloaded_path)
        } else if file_name.ends_with(".zip") {
            updater.extract_zip(downloaded_path)
        } else {
            Ok(downloaded_path.to_path_buf())
        }
    }

    /// Write `.install_pending_{NEW_VERSION}` JSON marker in the install
    /// directory. Read by the D11 health probe on the next startup to
    /// determine rollback eligibility + backup selection.
    ///
    /// Returns an error if the write fails; caller decides whether to abort
    /// the install or proceed without the probe marker.
    pub(super) fn write_install_pending(
        install_dir: &Path,
        new_version: &str,
        previous_version: &str,
        backup_path: &Path,
    ) -> Result<(), UpdateError> {
        let marker_path = install_dir.join(format!(".install_pending_{new_version}"));
        let payload = serde_json::json!({
            "installed_at": chrono::Utc::now().to_rfc3339(),
            "previous_version": previous_version,
            "backup_path": backup_path,
        });
        let bytes = serde_json::to_vec(&payload).map_err(|e| {
            UpdateError::Install(format!("Failed to serialize install_pending: {}", e))
        })?;
        std::fs::write(&marker_path, bytes)
            .map_err(|e| UpdateError::Install(format!("Failed to write install_pending: {}", e)))?;
        tracing::info!(
            "install_pending written: version={new_version}, previous={previous_version}"
        );
        Ok(())
    }

    /// Phase 4 D11 rollback execution.
    ///
    /// Contract:
    /// - Returns `Result<Infallible, UpdateError>` — the success path does
    ///   not return (spawns replacement binary, terminates current process
    ///   with `ROLLBACK_EXIT_CODE`).
    /// - Caller supplies `rollback_event` which is invoked BEFORE the swap
    ///   so subscribers (update_coordinator, UI) can observe the rollback
    ///   even if the subsequent restart races with shutdown.
    ///
    /// Windows: deferred to §4.8 spike deliverable (Task 12). Current stub
    /// returns `UpdateError::Install` with a spike-pending message.
    pub fn execute_rollback<F>(
        backup_path: &Path,
        current_exe_path: &Path,
        from_version: &str,
        to_version: &str,
        reason: oneshim_api_contracts::update::RollbackReason,
        rollback_event: F,
    ) -> Result<std::convert::Infallible, UpdateError>
    where
        F: FnOnce(&oneshim_api_contracts::update::RollbackInfo),
    {
        Self::execute_rollback_swap_only(
            backup_path,
            current_exe_path,
            from_version,
            to_version,
            reason,
            rollback_event,
        )?;

        // Spawn the restored binary as a child process and terminate the
        // current process. spawn + exit gives simpler semantics than image
        // replacement while still achieving the rollback outcome — user sees
        // a brief flicker as the new PID starts.
        #[cfg(unix)]
        {
            std::process::Command::new(current_exe_path)
                .spawn()
                .map_err(|e| {
                    UpdateError::Install(format!("rollback spawn of restored binary failed: {e}"))
                })?;
            std::process::exit(ROLLBACK_EXIT_CODE);
        }

        // Windows: image replacement + spawn semantics are delegated to the
        // Task 12 spike output (docs/guides/updater-rollback-windows.md).
        // The detection path (health probe) still fires on Windows — this
        // branch makes the swap a documented no-op that support can surface
        // in logs (holistic-review Q8).
        #[cfg(windows)]
        {
            let _ = current_exe_path;
            tracing::warn!(
                "rollback swap not implemented on Windows (Task 12 pending); \
                 user remains on failing binary until manual reinstall — \
                 see docs/guides/updater-rollback-windows.md"
            );
            return Err(UpdateError::Install(
                "Windows rollback helper pending (§4.8 spike — Task 12)".to_string(),
            ));
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = current_exe_path;
            return Err(UpdateError::Install(
                "rollback not implemented for this platform".to_string(),
            ));
        }
    }

    /// Test-mode + Windows-spike shared core: verify backup, broadcast event,
    /// rename backup into current_exe position. Does NOT perform the
    /// replacement-binary spawn — production `execute_rollback` does that
    /// after this helper returns Ok.
    ///
    /// Extracted so the integration test can observe the file-swap +
    /// event-broadcast portion without the test harness itself being
    /// terminated by the production path's `std::process::exit` call.
    pub(crate) fn execute_rollback_swap_only<F>(
        backup_path: &Path,
        current_exe_path: &Path,
        from_version: &str,
        to_version: &str,
        reason: oneshim_api_contracts::update::RollbackReason,
        rollback_event: F,
    ) -> Result<(), UpdateError>
    where
        F: FnOnce(&oneshim_api_contracts::update::RollbackInfo),
    {
        // 1. Verify backup exists and is a regular file.
        if !backup_path.exists() {
            return Err(UpdateError::Install(format!(
                "rollback backup not found: {:?}",
                backup_path
            )));
        }
        let backup_meta = std::fs::metadata(backup_path)
            .map_err(|e| UpdateError::Install(format!("stat backup failed: {e}")))?;
        if !backup_meta.is_file() {
            return Err(UpdateError::Install(format!(
                "rollback backup is not a regular file: {:?}",
                backup_path
            )));
        }

        // 2. Emit the event BEFORE the swap — subscribers observe the
        //    rollback even if the subsequent rename races with shutdown.
        let info = oneshim_api_contracts::update::RollbackInfo {
            from_version: from_version.to_string(),
            from_published_at: None, // populated upstream if caller has it
            to_version: to_version.to_string(),
            to_published_at: None,
            reason,
            rolled_back_at: chrono::Utc::now().to_rfc3339(),
        };
        rollback_event(&info);

        // Persist a rollback notification file so the restored binary can
        // surface the RolledBack state in the UI on its next boot. The
        // current (failing) process terminates immediately after the image
        // replacement; without a persisted marker the user would see the
        // version number decrement silently.
        //
        // Filename is version-scoped (`.rolled_back_notification_{to_version}`)
        // so the consumer on next boot can match its own running version and
        // sweep stale markers from earlier rollback cycles whose consumer did
        // not complete (holistic-review I-2).
        if let Some(install_dir) = current_exe_path.parent() {
            let notif_path = install_dir.join(format!(".rolled_back_notification_{to_version}"));
            match serde_json::to_vec(&info) {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&notif_path, bytes) {
                        tracing::warn!("rolled_back_notification write failed (non-fatal): {e}");
                    }
                }
                Err(e) => {
                    tracing::warn!("rolled_back_notification serialize failed (non-fatal): {e}")
                }
            }
        }

        // 3. Atomic rename: backup → current_exe. On Unix, this works even
        //    while the current binary is running (files held open by inode).
        //    On Windows, this would fail for a running executable — the
        //    production path short-circuits before reaching here on Windows
        //    (Task 12 replaces with a spike-derived mechanism).
        #[cfg(unix)]
        {
            std::fs::rename(backup_path, current_exe_path)
                .map_err(|e| UpdateError::Install(format!("rollback rename failed: {e}")))?;
        }

        #[cfg(not(unix))]
        {
            // Non-Unix test-harness only path: copy + remove simulates the swap
            // without depending on a helper binary. Production Windows hits the
            // §4.8-pending error branch in execute_rollback before reaching here.
            std::fs::copy(backup_path, current_exe_path).map_err(|e| {
                UpdateError::Install(format!("rollback copy (non-unix) failed: {e}"))
            })?;
            let _ = std::fs::remove_file(backup_path);
        }

        tracing::warn!(
            "rollback executed: from={from_version} -> to={to_version} (reason: {:?})",
            reason
        );
        Ok(())
    }

    /// # Safety
    pub fn install_and_restart(&self, downloaded_path: &Path) -> Result<(), UpdateError> {
        self.install_and_restart_versioned(downloaded_path, None)
    }

    /// Install-and-restart variant that additionally writes
    /// `.install_pending_{new_version}` for the D11 health probe.
    ///
    /// Callers in production should invoke this with `Some(new_version)` so
    /// rollback is armed; passing `None` disables the D11 probe for this
    /// install (rolling back becomes manual).
    pub fn install_and_restart_versioned(
        &self,
        downloaded_path: &Path,
        new_version: Option<&str>,
    ) -> Result<(), UpdateError> {
        let current_exe = std::env::current_exe()?;
        self.install_and_restart_with_ops(
            downloaded_path,
            &current_exe,
            new_version,
            |candidate| {
                self_replace::self_replace(candidate)
                    .map_err(|e| UpdateError::Install(format!("Failed to replace binary: {}", e)))
            },
            || self.restart_app(),
        )
    }

    pub(super) fn extract_tar_gz(&self, archive_path: &Path) -> Result<PathBuf, UpdateError> {
        use flate2::read::GzDecoder;
        use std::fs::File;

        let file = File::open(archive_path)?;
        let decoder = GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let extract_dir = archive_path
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"));
        for entry in archive.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?;

            if !Self::is_safe_archive_path(&entry_path) {
                return Err(UpdateError::Install(format!(
                    "Unsafe tar entry path: {}",
                    entry_path.display()
                )));
            }

            let outpath = extract_dir.join(entry_path.as_ref());

            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let entry_type = entry.header().entry_type();
            if entry_type.is_dir() {
                std::fs::create_dir_all(&outpath)?;
                continue;
            }

            if !entry_type.is_file() {
                return Err(UpdateError::Install(format!(
                    "Unsupported tar entry type: {}",
                    entry_path.display()
                )));
            }

            entry.unpack(&outpath)?;
        }

        self.find_binary_in_dir(extract_dir)
    }

    pub(super) fn extract_zip(&self, archive_path: &Path) -> Result<PathBuf, UpdateError> {
        use std::fs::File;

        let file = File::open(archive_path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| UpdateError::Install(format!("Failed to open ZIP archive: {}", e)))?;

        let extract_dir = archive_path
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"));

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| UpdateError::Install(format!("Failed to read ZIP entry: {}", e)))?;

            let relative_path = file.enclosed_name().ok_or_else(|| {
                UpdateError::Install(format!("Unsafe ZIP entry path: {}", file.name()))
            })?;

            if !Self::is_safe_archive_path(&relative_path) {
                return Err(UpdateError::Install(format!(
                    "Unsafe ZIP entry path: {}",
                    file.name()
                )));
            }

            let outpath = extract_dir.join(relative_path);

            if file.name().ends_with('/') {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p)?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut outfile)?;
            }
        }

        self.find_binary_in_dir(extract_dir)
    }

    pub(super) fn find_binary_in_dir(&self, dir: &std::path::Path) -> Result<PathBuf, UpdateError> {
        let binary_name = if cfg!(windows) {
            "oneshim.exe"
        } else {
            "oneshim"
        };

        let direct_path = dir.join(binary_name);
        if direct_path.exists() {
            return Ok(direct_path);
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let sub_binary = path.join(binary_name);
                if sub_binary.exists() {
                    return Ok(sub_binary);
                }
            } else if path.file_name().is_some_and(|n| n == binary_name) {
                return Ok(path);
            }
        }

        Err(UpdateError::Install(format!(
            "Binary '{}' not found",
            binary_name
        )))
    }

    pub(super) fn restart_app(&self) -> Result<(), UpdateError> {
        let current_exe = std::env::current_exe()?;
        let args: Vec<String> = std::env::args().skip(1).collect();

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&current_exe).args(&args).exec();
            Err(UpdateError::Install(format!("Restart failed: {}", err)))
        }

        #[cfg(windows)]
        {
            std::process::Command::new(&current_exe)
                .args(&args)
                .spawn()
                .map_err(|e| UpdateError::Install(format!("Restart failed: {}", e)))?;
            std::process::exit(0);
        }

        #[cfg(not(any(unix, windows)))]
        {
            Err(UpdateError::UnsupportedPlatform(
                "Restart is not supported on this platform".to_string(),
            ))
        }
    }
}

// ── Loop 3 iter 1 fix (I-1): direct in-bin coverage of execute_rollback_swap_only ──
//
// Previously the only test exercising the swap + event + notification
// write was the src-tauri/tests/ integration test, which re-implemented
// the behavior locally. That left the production helper — including the
// new .rolled_back_notification_{to_version} write — with zero
// coverage against actual code. Regressions in the write position (e.g.,
// on the Unix path, moving it after the rename) would ship silently.

#[cfg(test)]
mod rollback_tests {
    use super::*;
    use oneshim_api_contracts::update::{RollbackInfo, RollbackReason};
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[test]
    fn execute_rollback_swap_only_swaps_binary_and_emits_event_and_writes_notification() {
        let dir = tempdir().unwrap();
        let current_exe = dir.path().join("oneshim-current");
        let backup = dir.path().join("oneshim-current.rollback.42");

        let current_content = b"CURRENT-v0.5.0".to_vec();
        let backup_content = b"BACKUP-v0.4.40".to_vec();
        std::fs::write(&current_exe, &current_content).unwrap();
        std::fs::write(&backup, &backup_content).unwrap();

        let captured: Arc<Mutex<Option<RollbackInfo>>> = Arc::new(Mutex::new(None));
        let captured_clone = Arc::clone(&captured);

        let result = Updater::execute_rollback_swap_only(
            &backup,
            &current_exe,
            "0.5.0",
            "0.4.40",
            RollbackReason::RepeatedStartupFailure,
            move |info| {
                *captured_clone.lock().unwrap() = Some(info.clone());
            },
        );
        assert!(result.is_ok(), "swap_only should succeed");

        // (a) Binary swap landed — current now holds backup bytes.
        let post = std::fs::read(&current_exe).unwrap();
        assert_eq!(post, backup_content, "binary should be replaced by backup");

        // (b) Backup path is gone after rename/copy.
        assert!(!backup.exists(), "backup should be renamed/removed");

        // (c) Event callback fired with correct fields.
        let info = captured.lock().unwrap().clone().expect("event emitted");
        assert_eq!(info.from_version, "0.5.0");
        assert_eq!(info.to_version, "0.4.40");
        assert_eq!(info.reason, RollbackReason::RepeatedStartupFailure);

        // (d) Persistent notification file was written with valid JSON — the
        //     new binary's boot consumes this to surface RolledBack in UI.
        //     Filename is version-scoped per holistic-review I-2: the
        //     restored binary only consumes markers matching its own
        //     to_version, and sweeps stragglers from prior rollback cycles.
        let notif = dir.path().join(".rolled_back_notification_0.4.40");
        assert!(
            notif.exists(),
            "version-scoped notification file should be written"
        );
        assert!(
            !dir.path().join(".rolled_back_notification").exists(),
            "legacy unversioned filename must not be used"
        );
        let bytes = std::fs::read(&notif).unwrap();
        let persisted: RollbackInfo = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(persisted.from_version, "0.5.0");
        assert_eq!(persisted.to_version, "0.4.40");
        assert_eq!(persisted.reason, RollbackReason::RepeatedStartupFailure);
    }

    #[test]
    fn execute_rollback_swap_only_fails_when_backup_missing() {
        let dir = tempdir().unwrap();
        let current_exe = dir.path().join("oneshim-current");
        let missing_backup = dir.path().join("does-not-exist.rollback.0");
        std::fs::write(&current_exe, b"current").unwrap();

        let result = Updater::execute_rollback_swap_only(
            &missing_backup,
            &current_exe,
            "0.5.0",
            "0.4.40",
            RollbackReason::RepeatedStartupFailure,
            |_| panic!("event should NOT fire when backup is missing"),
        );
        assert!(
            matches!(result, Err(UpdateError::Install(_))),
            "missing backup should error"
        );
        // current_exe content unchanged (no partial swap).
        let post = std::fs::read(&current_exe).unwrap();
        assert_eq!(post, b"current");
    }
}
