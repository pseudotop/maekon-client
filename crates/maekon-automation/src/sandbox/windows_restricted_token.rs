use super::{OwnedHandle, Win32Handle};
use crate::error::AutomationError;
use crate::sandbox::win_limits::TokenRestrictions;

/// Create a restricted token from the current process token.
///
/// Demotes the `BUILTIN\Administrators` group to DENY-ONLY via `SidsToDisable`
/// whenever the policy sets `disable_admin_sid` (every profile), and adds
/// `DISABLE_MAX_PRIVILEGE` when the profile strips privileges (removing dangerous
/// privileges such as SeDebugPrivilege/SeTcbPrivilege). Before #7071 the
/// `disable_admin_sid` policy was only logged — `SidsToDisable` was always null,
/// so `CreateRestrictedToken` returned a token with the parent's full group set
/// and the advertised admin-SID drop was a no-op. The resulting token is a PRIMARY
/// token (the source is opened with `TOKEN_ASSIGN_PRIMARY`), so it can be handed to
/// `CreateProcessAsUserW` to actually launch the worker under it (see
/// `spawn_process_with_token`).
///
/// When `disable_most_sids` is set, the token uses `WRITE_RESTRICTED` and a
/// minimal `SidsToRestrict` set: Write Restricted Code, the interactive logon SID,
/// and the current user SID. This removes group-derived write grants while keeping
/// DLL loading, HKCU access, and session initialization viable. Standard/Strict
/// still fail closed before spawn for the remaining unavailable containment layers.
pub(super) fn create_restricted_token(
    restrictions: &TokenRestrictions,
) -> Result<OwnedHandle, AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut process_token: Win32Handle = std::ptr::null_mut();
    let ret = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut process_token,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "OpenProcessToken failed: error {err}"
        )));
    }
    let _process_token = OwnedHandle(process_token);

    let mut flags: u32 = 0;
    if restrictions.remove_privileges {
        flags |= DISABLE_MAX_PRIVILEGE;
    }
    if restrictions.disable_most_sids {
        // A full Restricted Code token prevents ordinary system DLLs from
        // initializing (STATUS_DLL_INIT_FAILED) unless every dependency DACL is
        // rewritten for the restricted SID. WRITE_RESTRICTED is the native
        // Windows mode for preserving reads while requiring the restricting SID
        // on every write access check.
        flags |= WRITE_RESTRICTED;
    }

    // Build the deny-only SID list. When `disable_admin_sid` is set we list the
    // BUILTIN\Administrators SID in `SidsToDisable`, which marks it
    // SE_GROUP_USE_FOR_DENY_ONLY in the new token: it can no longer GRANT access
    // (only contribute to deny ACEs), so a compromised worker cannot use the
    // parent's Administrators membership. The SID buffer and the
    // `SID_AND_ATTRIBUTES` array must both outlive the `CreateRestrictedToken`
    // call below, so they are bound here in the function scope.
    let admin_sid_buffer = if restrictions.disable_admin_sid {
        Some(build_administrators_sid()?)
    } else {
        None
    };
    let mut sids_to_disable: Vec<SID_AND_ATTRIBUTES> = Vec::new();
    if let Some(buffer) = admin_sid_buffer.as_ref() {
        sids_to_disable.push(SID_AND_ATTRIBUTES {
            // `SidsToDisable` reads only the SID pointer; `Attributes` is ignored.
            Sid: buffer.as_ptr() as *mut core::ffi::c_void,
            Attributes: 0,
        });
    }
    let (disable_sid_count, disable_sid_ptr) = if sids_to_disable.is_empty() {
        (0u32, std::ptr::null())
    } else {
        (sids_to_disable.len() as u32, sids_to_disable.as_ptr())
    };

    // Standard/Strict carry the well-known Write Restricted Code SID (S-1-5-33)
    // in `SidsToRestrict`. Together with WRITE_RESTRICTED this is an allow-list
    // for a second write-access check. The dedicated Windows SID is preferable to
    // an ad-hoc Everyone/Users list, which would preserve broad group grants and
    // only simulate containment.
    let write_restricted_code_sid_buffer = if restrictions.disable_most_sids {
        Some(build_write_restricted_code_sid()?)
    } else {
        None
    };
    // Window stations and desktops grant access to the session's logon SID.
    // Keeping only S-1-5-33 makes CreateProcessAsUserW start the image but the
    // child fails during DLL/desktop initialization with STATUS_DLL_INIT_FAILED.
    // Adding the exact logon SID preserves only the current interactive session
    // access; it does not restore arbitrary user/group SIDs.
    //
    // That covers the window station and desktop, and only those. It does not
    // make a console-allocating child work on an administrator account — the
    // logon SID does not carry that grant (#11197, table below).
    let logon_sid_buffer = if restrictions.disable_most_sids {
        Some(build_logon_sid(process_token)?)
    } else {
        None
    };
    let user_sid_buffer = if restrictions.disable_most_sids {
        Some(build_user_sid(process_token)?)
    } else {
        None
    };
    // ── Do NOT add BUILTIN\Administrators to this list (#11197) ─────────────
    //
    // A console-allocating child (`CREATE_NO_WINDOW`) dies at 0xC0000142 under
    // this token whenever the account is an administrator, and the one edit that
    // fixes it is putting S-1-5-32-544 here. Measured on Windows 11 26200.9168,
    // elevated, console flag and spawn path held fixed, list as the only variable:
    //
    //   [WRC, logon, user]              0xC0000142    [WRC, logon, user, ADMIN]  exit 7
    //   [WRC, logon, user, RESTRICTED]  0xC0000142    [ADMIN] alone              exit 7
    //   [RESTRICTED] alone              0xC0000142
    //   [logon, user]                   0xC0000142
    //   [WRC] alone                     0xC0000142
    //
    // Every list without that SID fails; every list with it passes, `[ADMIN]`
    // alone included. The access the console path needs is granted through
    // Administrators and nothing else — which is exactly the grant
    // `disable_admin_sid` exists to remove (#7071). Restoring it here would let
    // the worker write through admin-derived access again and quietly undo the
    // containment, in exchange for a capability this worker does not need: its
    // three std handles are pipes, so it never wants a console.
    //
    // `DETACHED_PROCESS` in `spawn_process_with_token` is the resolution, and it
    // is the design rather than a workaround — on an administrator account a
    // properly restricted token cannot allocate a console at all. Adding
    // S-1-5-12 does not substitute; it was measured and it fails.
    let mut sids_to_restrict: Vec<SID_AND_ATTRIBUTES> = Vec::new();
    if let Some(buffer) = write_restricted_code_sid_buffer.as_ref() {
        sids_to_restrict.push(SID_AND_ATTRIBUTES {
            Sid: buffer.as_ptr() as *mut core::ffi::c_void,
            // CreateRestrictedToken requires zero attributes for restricting SIDs.
            Attributes: 0,
        });
    }
    if let Some(buffer) = logon_sid_buffer.as_ref() {
        sids_to_restrict.push(SID_AND_ATTRIBUTES {
            Sid: buffer.as_ptr() as *mut core::ffi::c_void,
            Attributes: 0,
        });
    }
    if let Some(buffer) = user_sid_buffer.as_ref() {
        sids_to_restrict.push(SID_AND_ATTRIBUTES {
            Sid: buffer.as_ptr() as *mut core::ffi::c_void,
            Attributes: 0,
        });
    }
    let (restricted_sid_count, restricted_sid_ptr) = if sids_to_restrict.is_empty() {
        (0u32, std::ptr::null())
    } else {
        (sids_to_restrict.len() as u32, sids_to_restrict.as_ptr())
    };

    let mut restricted_token: Win32Handle = std::ptr::null_mut();
    let ret = unsafe {
        CreateRestrictedToken(
            process_token,
            flags,
            disable_sid_count,
            disable_sid_ptr,
            0,
            std::ptr::null(), // delete privileges
            restricted_sid_count,
            restricted_sid_ptr,
            &mut restricted_token,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateRestrictedToken failed: error {err}"
        )));
    }

    tracing::debug!(
        disable_admin = restrictions.disable_admin_sid,
        remove_privs = restrictions.remove_privileges,
        restrict_most_sids = restrictions.disable_most_sids,
        restricting_sids = restricted_sid_count,
        deny_only_sids = disable_sid_count,
        "Restricted token created and ready for worker launch via CreateProcessAsUserW"
    );

    // The caller passes this PRIMARY restricted token to `CreateProcessAsUserW`
    // (see `spawn_process_with_token`) so the spawned worker actually runs under
    // it. The `OwnedHandle` keeps the token alive until the launch completes and
    // closes it on drop.
    Ok(OwnedHandle(restricted_token))
}

/// Build the `BUILTIN\Administrators` group SID (S-1-5-32-544) into a caller-owned
/// buffer.
///
/// `CreateWellKnownSid` writes the SID into the fixed-size array; keeping it in a
/// caller-owned buffer (rather than `AllocateAndInitializeSid` + `FreeSid`) means
/// there is no heap SID to free and the buffer's pointer stays valid for the whole
/// `CreateRestrictedToken` call that consumes it. The 68-byte size is
/// `SECURITY_MAX_SID_SIZE`, the documented upper bound for any SID.
fn build_administrators_sid() -> Result<[u8; 68], AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::{CreateWellKnownSid, WinBuiltinAdministratorsSid};

    let mut buffer = [0u8; 68];
    let mut size = buffer.len() as u32;
    // SAFETY: `buffer`/`size` are valid out-pointers; `buffer` is sized at
    // SECURITY_MAX_SID_SIZE so CreateWellKnownSid (which writes at most `size`
    // bytes and updates `size`) cannot overflow it. The domain SID is null, which
    // is required for a built-in well-known SID.
    let ret = unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            &mut size,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateWellKnownSid(BUILTIN\\Administrators) failed: error {err}"
        )));
    }
    Ok(buffer)
}

/// Build the Windows Write Restricted Code SID (S-1-5-33) into a caller-owned
/// buffer.
///
/// The buffer must remain alive until `CreateRestrictedToken` consumes the
/// corresponding `SID_AND_ATTRIBUTES` entry. Windows defines this SID expressly
/// for processes running in a restricted security context.
pub(super) fn build_write_restricted_code_sid() -> Result<[u8; 68], AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::{CreateWellKnownSid, WinWriteRestrictedCodeSid};

    let mut buffer = [0u8; 68];
    let mut size = buffer.len() as u32;
    // SAFETY: `buffer` is SECURITY_MAX_SID_SIZE bytes and remains alive in the
    // caller until CreateRestrictedToken returns. Restricted Code has no domain SID.
    let ret = unsafe {
        CreateWellKnownSid(
            WinWriteRestrictedCodeSid,
            std::ptr::null_mut(),
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            &mut size,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateWellKnownSid(Write Restricted Code) failed: error {err}"
        )));
    }
    Ok(buffer)
}

/// Copy the token's user SID for user-profile and HKCU initialization writes.
///
/// Retaining the user SID is narrower than retaining every enabled group: writes
/// granted only through domain/local group membership still fail the second
/// access check, while the worker can initialize inside the signed-in session.
pub(super) fn build_user_sid(token: Win32Handle) -> Result<[u8; 68], AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::{CopySid, GetTokenInformation, TokenUser, TOKEN_USER};

    let mut needed = 0u32;
    unsafe {
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "GetTokenInformation(TokenUser) sizing failed: error {err}"
        )));
    }

    let words = (needed as usize) / 8 + 1;
    let mut token_user_buffer = vec![0u64; words];
    let capacity = (token_user_buffer.len() * 8) as u32;
    let ret = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            token_user_buffer.as_mut_ptr() as *mut core::ffi::c_void,
            capacity,
            &mut needed,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "GetTokenInformation(TokenUser) failed: error {err}"
        )));
    }

    // SAFETY: GetTokenInformation initialized the aligned buffer as TOKEN_USER.
    let token_user = unsafe { &*(token_user_buffer.as_ptr() as *const TOKEN_USER) };
    let mut buffer = [0u8; 68];
    let ret = unsafe {
        CopySid(
            buffer.len() as u32,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            token_user.User.Sid,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CopySid(user SID) failed: error {err}"
        )));
    }
    Ok(buffer)
}

/// Copy the current session's logon SID into a caller-owned SID buffer.
///
/// The logon SID is the token group marked with `SE_GROUP_LOGON_ID`. It is the
/// narrowly scoped identity used by the interactive window station and desktop,
/// so retaining it avoids restoring broad `Everyone` or `BUILTIN\Users` grants.
pub(super) fn build_logon_sid(token: Win32Handle) -> Result<[u8; 68], AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::{
        CopySid, GetTokenInformation, TokenGroups, SID_AND_ATTRIBUTES, TOKEN_GROUPS,
    };

    const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

    let mut needed = 0u32;
    unsafe {
        GetTokenInformation(token, TokenGroups, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "GetTokenInformation(TokenGroups) sizing failed: error {err}"
        )));
    }

    // TOKEN_GROUPS includes pointer-sized fields, so use u64 storage to preserve
    // the alignment required when reading the trailing SID_AND_ATTRIBUTES array.
    let words = (needed as usize) / 8 + 1;
    let mut token_groups_buffer = vec![0u64; words];
    let capacity = (token_groups_buffer.len() * 8) as u32;
    let ret = unsafe {
        GetTokenInformation(
            token,
            TokenGroups,
            token_groups_buffer.as_mut_ptr() as *mut core::ffi::c_void,
            capacity,
            &mut needed,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "GetTokenInformation(TokenGroups) failed: error {err}"
        )));
    }

    // SAFETY: GetTokenInformation initialized the aligned buffer and GroupCount
    // bounds the inline array.
    let groups = unsafe { &*(token_groups_buffer.as_ptr() as *const TOKEN_GROUPS) };
    let entries: &[SID_AND_ATTRIBUTES] =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    let logon_sid = entries
        .iter()
        .find(|entry| entry.Attributes & SE_GROUP_LOGON_ID == SE_GROUP_LOGON_ID)
        .ok_or_else(|| {
            AutomationError::SandboxEnforcement(
                "Current process token does not contain a logon SID".to_string(),
            )
        })?;

    let mut buffer = [0u8; 68];
    let ret = unsafe {
        CopySid(
            buffer.len() as u32,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            logon_sid.Sid,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CopySid(logon SID) failed: error {err}"
        )));
    }
    Ok(buffer)
}
