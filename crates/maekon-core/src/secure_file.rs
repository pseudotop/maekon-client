//! #6942: owner-only file-permission primitive — the Windows counterpart of
//! Unix `chmod 0o600` (owner-only DACL).
//!
//! Placed in the contract crate (`maekon-core`) so that every layer writing
//! files that hold plaintext secrets shares a **single SSOT**:
//! - `config_manager::persistence` / `consent` (core) — config.json / consent.json
//!   (includes plaintext secrets such as inline API keys and integration auth tokens, #6942 F7)
//! - `telemetry::instance_id` (src-tauri) — instance_id (#6942 F8)
//! - (when delegated) the secret stores in `maekon-storage`
//!
//! Previously this same DACL logic existed only in `maekon-storage` as
//! `pub(crate)`, so the lower hex layer `maekon-core` could not reach it
//! (adapter→core one-way dependency). Pulling it up into core lets the Windows
//! write path for config/consent receive owner-only protection equivalent to
//! Unix `0o600`.
//!
//! This module exposes two SSOTs:
//! - `write_owner_only` — the cross-platform "create-empty → harden → write" tmp
//!   helper shared by every core writer that persists privacy-sensitive plaintext
//!   (config.json inline secrets, consent.json identity/consent metadata). It is
//!   the single source so the consent and config write paths cannot diverge
//!   again (#7074).
//! - `set_owner_only_dacl` — the Windows-only owner-only DACL primitive (it does
//!   not compile on Unix; the Unix path uses `O_CREAT|O_EXCL|mode(0o600)`).

use std::path::Path;

/// Write `bytes` to `path` so the file is owner-only the instant any bytes exist
/// on disk, then leave it ready for an atomic rename into place.
///
/// This is the single SSOT for the "create-empty → harden → write" tmp pattern
/// shared by the core writers that persist privacy-sensitive plaintext
/// (`config_manager::persistence` / `consent`). Hoisting it here prevents those
/// two write paths from diverging again (#7074: the config path previously used
/// the weaker write-then-tighten order with no fail-closed cleanup while the
/// consent path was already hardened).
///
/// `path` is removed first so a stale tmp from a previously aborted write is not
/// re-opened with inherited/too-open permissions.
///
/// - **Unix**: the file is created with `O_CREAT | O_EXCL` + mode `0o600` in a
///   single `open(2)`, so there is never a window in which the secrets-bearing
///   file is world/group-readable. On a write failure the partial file is removed.
/// - **Windows**: the file is created EMPTY, tightened to an owner-only DACL while
///   it still holds no bytes, and only THEN written — so the sensitive bytes never
///   rest on a file carrying the inherited (potentially world-readable)
///   parent-directory ACL. FAIL-CLOSED: on DACL failure the empty file is removed
///   and the error surfaced; on a write failure the (already-hardened) file is
///   removed. Mirrors `EncryptionKey::save_to_file`.
/// - **Other**: best-effort plain write (no umask/DACL model).
///
/// Returns `io::Result` so each caller maps the failure into its own error type
/// (config → `CoreError::Config`, consent → `CoreError::Io`). A Windows DACL
/// failure is surfaced as `io::Error::other`.
pub fn write_owner_only(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Remove any orphaned tmp from a previously aborted write so the create below
    // starts clean (no stale contents / inherited permissions).
    let _ = std::fs::remove_file(path);

    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        // O_CREAT | O_EXCL | mode 0o600 in one syscall — atomically restrictive.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes).inspect_err(|_| {
            let _ = std::fs::remove_file(path);
        })?;
    }

    #[cfg(windows)]
    {
        use std::io::Write as _;
        // Create the tmp EMPTY (and drop the handle), apply the owner-only DACL
        // while the file holds no sensitive bytes, then reopen and write — so the
        // sensitive bytes never rest on a file carrying the inherited parent-dir
        // ACL. FAIL-CLOSED: a DACL failure removes the empty tmp and surfaces the
        // error rather than writing the payload world-readable.
        std::fs::File::create(path)?;
        set_owner_only_dacl(path).map_err(|e| {
            let _ = std::fs::remove_file(path);
            std::io::Error::other(e.to_string())
        })?;
        let mut file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.write_all(bytes).inspect_err(|_| {
            let _ = std::fs::remove_file(path);
        })?;
    }

    #[cfg(all(not(unix), not(windows)))]
    {
        std::fs::write(path, bytes)?;
    }

    Ok(())
}

/// Applies an owner-only DACL to a file or directory (the Windows counterpart of
/// Unix `chmod 0o600`).
///
/// Builds an ACL that grants the current user a single `GENERIC_ALL` ACE and
/// applies it as a **protected DACL** that blocks inheritance from the parent.
/// Because the target must already exist (called after the write), this is a
/// defense-in-depth control that layers owner-only protection on top of the
/// default user-profile ACL of `%LOCALAPPDATA%`/`%APPDATA%`.
///
/// This is the single canonical owner-only DACL primitive for the whole client
/// workspace (#7101). `maekon-storage` no longer keeps its own copy: its
/// `pub(crate)` `encryption::set_owner_only_dacl` is now a thin shim that calls
/// this function and maps `CoreError` into `StorageError`, so the two
/// implementations can never diverge again.
#[cfg(windows)]
pub fn set_owner_only_dacl(path: &Path) -> Result<(), crate::error::CoreError> {
    // windows-sys 0.61: `OpenProcessToken` lives in `Win32::System::Threading`,
    // while `GENERIC_ALL`/`HANDLE` live in `Win32::Foundation`; since `HANDLE`
    // is `*mut c_void`, initialize it with `null_mut()`.
    use windows_sys::Win32::Foundation::{LocalFree, GENERIC_ALL, HANDLE};
    use windows_sys::Win32::Security::Authorization::{SetNamedSecurityInfoW, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AddAccessAllowedAce, GetTokenInformation, InitializeAcl, TokenUser, ACL as WIN_ACL,
        ACL_REVISION, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn err(message: impl Into<String>) -> crate::error::CoreError {
        crate::error::CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: message.into(),
        }
    }

    let wide_path: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        // 1. Look up the current user's SID
        let mut token_handle: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle) == 0 {
            return Err(err("OpenProcessToken failed"));
        }

        let mut needed: u32 = 0;
        GetTokenInformation(
            token_handle,
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 || needed > 4096 {
            windows_sys::Win32::Foundation::CloseHandle(token_handle);
            return Err(err(format!("unexpected token info size: {needed} bytes")));
        }
        let mut user_buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token_handle,
            TokenUser,
            user_buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            windows_sys::Win32::Foundation::CloseHandle(token_handle);
            return Err(err("GetTokenInformation failed"));
        }
        windows_sys::Win32::Foundation::CloseHandle(token_handle);

        let token_user = &*(user_buf.as_ptr() as *const TOKEN_USER);
        let user_sid = token_user.User.Sid;

        // 2. Build the ACL with a single owner-only ACE
        let sid_len = windows_sys::Win32::Security::GetLengthSid(user_sid);
        // The `SidStart` field of `ACCESS_ALLOWED_ACE` is already counted once
        // in the struct size, so subtract `sizeof(u32)` to avoid double-counting.
        if (sid_len as usize) < std::mem::size_of::<u32>() {
            return Err(err(format!("SID length too small: {sid_len} bytes")));
        }
        let acl_size = std::mem::size_of::<WIN_ACL>() as u32
            + std::mem::size_of::<windows_sys::Win32::Security::ACCESS_ALLOWED_ACE>() as u32
            + sid_len
            - std::mem::size_of::<u32>() as u32;
        let mut acl_buf = vec![0u8; acl_size as usize];
        let acl_ptr = acl_buf.as_mut_ptr() as *mut WIN_ACL;

        if InitializeAcl(acl_ptr, acl_size, ACL_REVISION) == 0 {
            return Err(err("InitializeAcl failed"));
        }

        if AddAccessAllowedAce(acl_ptr, ACL_REVISION, GENERIC_ALL, user_sid) == 0 {
            return Err(err("AddAccessAllowedAce failed"));
        }

        // 3. Apply as a protected DACL (block parent inheritance)
        let result = SetNamedSecurityInfoW(
            wide_path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl_ptr,
            std::ptr::null_mut(),
        );

        // acl_buf is a stack (Vec) allocation, so LocalFree is unnecessary.
        let _ = LocalFree; // suppress unused-import warning

        if result != 0 {
            return Err(err(format!(
                "SetNamedSecurityInfoW failed with error {result}"
            )));
        }

        tracing::debug!("file DACL set to owner-only: {:?}", path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shared helper writes the payload exactly and (on Unix) lands the file
    /// owner-only (0o600), with no world/group-readable window. The Windows DACL
    /// is exercised on CI via the `#[cfg(windows)]` path in `write_owner_only`.
    #[test]
    fn write_owner_only_writes_payload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.tmp");
        write_owner_only(&path, b"top-secret-bytes").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"top-secret-bytes");
    }

    /// Regression for #7074: the helper must overwrite a stale tmp left by a
    /// previously aborted write (it removes the file first), rather than failing
    /// the atomic `create_new` or inheriting the stale file's permissions.
    #[test]
    fn write_owner_only_overwrites_stale_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.tmp");
        // Pre-seed a stale tmp with different (longer) contents.
        std::fs::write(&path, b"STALE LEFTOVER CONTENTS").unwrap();
        write_owner_only(&path, b"fresh").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
    }

    #[cfg(unix)]
    #[test]
    fn write_owner_only_is_0o600_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secret.tmp");
        write_owner_only(&path, b"x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "owner-only tmp must be 0o600, got 0o{mode:o}");
    }

    /// #7101: the canonical primitive applies an owner-only DACL to an existing
    /// file. This is the single implementation `maekon-storage` now delegates to,
    /// so exercising it here covers both crates' DACL behavior. CI-verified on the
    /// Windows runner; it does not compile on Unix.
    #[cfg(windows)]
    #[test]
    fn set_owner_only_dacl_succeeds_on_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dacl_target.tmp");
        std::fs::File::create(&path).unwrap();
        set_owner_only_dacl(&path).expect("owner-only DACL must apply to an existing file");
    }
}
