//! `windows.rs` tests, split out under ADR-013 (#11218).
//!
//! The parent file's cap had been raised six times in two months, each time to
//! exactly the new line count, which returns headroom to zero — the next
//! one-line change then breaks main for every maekon-client PR. That is the
//! ratchet erosion #9636 named. The split marker in `windows.rs` pointed at this
//! file as the first candidate; this is that split, not a seventh raise.
//!
//! Attached with `#[path]` from `windows.rs`, so `super::*` still resolves to the
//! `windows` module and nothing about test visibility changes.

use super::*;
use crate::sandbox::is_permissive_noop;
// Must match its users' gate: without this the import is unused whenever
// `windows-sandbox` is off, and `-D unused` makes that fatal (#11023).
#[cfg(feature = "windows-sandbox")]
use crate::sandbox::probe_verdict::{record_probe_verdict, ProbeVerdict};
use maekon_core::config::SandboxProfile;

// build_job_limits / build_token_restrictions tests moved to the cfg-free
// `crate::sandbox::win_limits` module (#5138) so the limit/token policy is
// verified on every OS, not only a Windows runner.

#[cfg(feature = "windows-sandbox")]
#[test]
fn owned_handle_validity() {
    let null_h = OwnedHandle(std::ptr::null_mut());
    assert!(!null_h.is_valid());
    std::mem::forget(null_h);

    let invalid_h = OwnedHandle(windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
    assert!(!invalid_h.is_valid());
    std::mem::forget(invalid_h);

    let valid_h = OwnedHandle(42usize as Win32Handle);
    assert!(valid_h.is_valid());
    std::mem::forget(valid_h);
}

/// #6439 F5 — prove the Job Object resource limits actually BIND in the kernel,
/// not merely that `create_job_object` returns `Ok`. Builds the job from a Strict
/// config, then queries the object back via `QueryInformationJobObject` and asserts
/// the limit flags + values the kernel stored match what was set (a round-trip).
/// Runs on the `windows-latest` `--features windows-sandbox` CI job; `windows.rs`
/// is `#[cfg(target_os = "windows")]` so it is not compiled on Linux/macOS.
#[cfg(feature = "windows-sandbox")]
#[test]
fn job_object_limits_actually_bind() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::JobObjects::{
        JobObjectExtendedLimitInformation, QueryInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };

    let config = SandboxConfig {
        profile: SandboxProfile::Strict,
        ..Default::default()
    };
    let limits = build_job_limits(&config);
    // Strict sets a non-zero memory limit (pinned by the win_limits policy tests);
    // the round-trip below would be vacuous otherwise.
    assert!(
        limits.max_memory_bytes > 0,
        "fixture: Strict must configure a memory limit"
    );

    let job = create_job_object(&limits).expect("create_job_object must succeed");

    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    let mut returned: u32 = 0;
    let ok = unsafe {
        QueryInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &mut info as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            &mut returned,
        )
    };
    assert_ne!(
        ok,
        0,
        "QueryInformationJobObject failed: error {}",
        unsafe { GetLastError() }
    );

    let flags = info.BasicLimitInformation.LimitFlags;
    // KILL_ON_JOB_CLOSE is always set (orphan prevention) — must be bound.
    assert_ne!(
        flags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        0,
        "KILL_ON_JOB_CLOSE must be bound in the kernel"
    );
    // The memory limit must be bound, with the exact configured value.
    assert_ne!(
        flags & JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        0,
        "PROCESS_MEMORY limit flag must be bound"
    );
    assert_eq!(
        info.ProcessMemoryLimit, limits.max_memory_bytes as usize,
        "kernel-stored memory limit must equal the configured value"
    );
    // When a process-count cap is configured it too must round-trip.
    if limits.max_processes > 0 {
        assert_ne!(
            flags & JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            0,
            "ACTIVE_PROCESS limit flag must be bound"
        );
        assert_eq!(
            info.BasicLimitInformation.ActiveProcessLimit, limits.max_processes,
            "kernel-stored active-process limit must equal the configured value"
        );
    }
    // `job` (OwnedHandle) drops here → CloseHandle, terminating the empty job.
}

/// End-to-end runtime proof of the restricted-token LAUNCH path: token apply
/// (`CreateProcessAsUserW`) + Job Object assignment + pipe wiring + bounded
/// wait + exit-code/stdout capture. `windows.rs` only compiles on Windows, so
/// this is the sole place the FFI launch can be exercised at runtime; it runs
/// on the `windows-latest` `--features windows-sandbox` CI leg.
///
/// Uses `cmd.exe` (a guaranteed system binary) as a stand-in for the worker.
/// The deny-only Administrators policy is pinned by the neighboring token
/// group test; this launch probe avoids that CI-sensitive desktop/DLL-init
/// side effect so it can focus on the raw CreateProcessAsUserW launch,
/// inherited stdio pipes, Job Object assignment, and bounded wait mechanics.
/// `clear_env = false` here so cmd can resolve its dependencies from the
/// inherited environment; the production worker path uses `clear_env = true`.
#[cfg(feature = "windows-sandbox")]
#[test]
fn restricted_token_launch_runs_child_and_captures_stdout() {
    use std::os::windows::ffi::OsStrExt;

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let system32_dir = format!("{system_root}\\System32");
    let cmd_path = format!("{system32_dir}\\cmd.exe");

    // Exercise the complete Standard/Strict token policy together. The
    // neighboring tests inspect the individual SID attributes and ACL
    // behavior; this launch probe catches combinations that produce a valid
    // token handle but fail later during DLL or desktop initialization.
    let config = SandboxConfig {
        profile: SandboxProfile::Permissive,
        ..Default::default()
    };
    let job = create_job_object(&build_job_limits(&config)).expect("create_job_object");
    let launch_restrictions = TokenRestrictions {
        disable_admin_sid: true,
        // Exercise #7979 end-to-end: the write-restricting SID must still let
        // a system binary initialize and use the inherited stdout pipe.
        disable_most_sids: true,
        remove_privileges: true,
    };
    let token = create_restricted_token(&launch_restrictions).expect("restricted token");

    let application_name: Vec<u16> = std::ffi::OsStr::new(&cmd_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let current_directory: Vec<u16> = std::ffi::OsStr::new(&system32_dir)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let probe = "maekon_sandbox_token_probe_4242";
    let mut command_line: Vec<u16> =
        std::ffi::OsStr::new(&format!("\"{cmd_path}\" /c echo {probe}"))
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

    let outcome = spawn_process_with_token(
        &application_name,
        // `as_mut_slice()` yields `&mut [u16]` directly (no Vec→slice coercion
        // through `Option`, which would not type-check).
        Some(command_line.as_mut_slice()),
        Some(current_directory.as_slice()),
        &job,
        &token,
        b"",
        30_000,
        false,
    )
    .expect("CreateProcessAsUserW restricted-token launch must succeed");

    assert!(!outcome.timed_out, "child must not time out");
    // #10288 previously skipped 0xC0000142 here on GitHub-hosted CI, on the
    // diagnosis that the runner image was at fault. The diagnosis was wrong
    // — the cause was CREATE_NO_WINDOW allocating a console the restricted
    // token cannot complete — and the skip is gone with it. This asserts at
    // full strength on every environment now, so a console-allocation
    // regression fails the column instead of being swallowed by it.
    assert_eq!(
        outcome.exit_code,
        0,
        "cmd /c echo must exit 0; stderr: {}",
        String::from_utf8_lossy(&outcome.stderr)
    );
    // Recorded on the passing branch too (#10959): a line that appears only
    // on failure makes silence ambiguous.
    record_probe_verdict(ProbeVerdict::Passed);
    let stdout = String::from_utf8_lossy(&outcome.stdout);
    assert!(
        stdout.contains(probe),
        "captured stdout must contain the probe, got: {stdout:?}"
    );
}

/// #7979 enforcement proof — a normal token can write through its
/// Authenticated Users group, but that group is intentionally absent from the
/// restricting SID set. The write-restricted child must therefore fail the
/// kernel's second access check even though its normal access check succeeds.
#[cfg(feature = "windows-sandbox")]
#[test]
fn restricted_token_blocks_group_only_file_write() {
    use std::os::windows::ffi::OsStrExt;
    use std::process::Command;

    let temp = tempfile::tempdir().expect("temporary ACL fixture directory");
    let target = temp.path().join("group-only-write.txt");
    std::fs::write(&target, b"unchanged\n").expect("create ACL fixture");

    // S-1-5-11 is Authenticated Users. Remove inherited ACEs, then grant
    // Modify only through that group. The parent remains able to write, which
    // proves the first access check permits the operation.
    let acl = Command::new("icacls.exe")
        .arg(&target)
        .args(["/inheritance:r", "/grant:r", "*S-1-5-11:(M)"])
        .output()
        .expect("run icacls");
    assert!(
        acl.status.success(),
        "icacls failed: stdout={} stderr={}",
        String::from_utf8_lossy(&acl.stdout),
        String::from_utf8_lossy(&acl.stderr)
    );
    // #11213 — `/inheritance:r /grant:r` does NOT leave Authenticated Users as
    // the only principal. Measured on a hosted runner, the DACL after the call
    // above still read:
    //
    //     NT AUTHORITY\Authenticated Users:(M)
    //     NT AUTHORITY\SYSTEM:(F)
    //     BUILTIN\Administrators:(F)
    //     runnervmk2qs2\runneradmin:(F)     <- the invoking user itself
    //
    // The last one is fatal to this probe. The invoking user's SID is one of the
    // three restricting SIDs bound into the token (write-restricted code, logon,
    // user — see `restricted_token_contains_write_restricted_code_sid`), so that
    // single ACE satisfies the second access check on its own. The probe then
    // measures nothing: the write is permitted by design, not by a containment
    // failure, and the test read that as "containment is broken".
    //
    // SYSTEM and Administrators may stay. Neither is in the restricting set, so
    // neither can satisfy the second check. Only the user ACE has to go.
    let whoami = Command::new("whoami.exe").output().expect("run whoami");
    let me = String::from_utf8_lossy(&whoami.stdout).trim().to_string();
    assert!(
        !me.is_empty(),
        "whoami must report the invoking account: {:?}",
        String::from_utf8_lossy(&whoami.stderr)
    );
    let removed = Command::new("icacls.exe")
        .arg(&target)
        .args(["/remove:g", &me])
        .output()
        .expect("run icacls /remove:g");
    assert!(
        removed.status.success(),
        "icacls /remove:g failed: stdout={} stderr={}",
        String::from_utf8_lossy(&removed.stdout),
        String::from_utf8_lossy(&removed.stderr)
    );

    // The fixture precondition, asserted before anything is measured.
    //
    // "Containment is broken" and "the fixture was never built" are different
    // facts, and until now the probe could not tell them apart — it assumed the
    // DACL was what it asked for and reported the difference as a containment
    // failure. Read the DACL back and require the intended shape first, so a
    // setup that does not take fails as a setup problem.
    let dacl = Command::new("icacls.exe")
        .arg(&target)
        .output()
        .expect("read back target DACL");
    let dacl_text = format!(
        "{}{}",
        String::from_utf8_lossy(&dacl.stdout),
        String::from_utf8_lossy(&dacl.stderr)
    );
    // Check the SID as well as the name. `icacls` prints an account name when it
    // can resolve one and a raw SID when it cannot, so a name-only check passes
    // silently in exactly the environment where the name does not resolve.
    let sid_out = Command::new("whoami.exe")
        .args(["/user"])
        .output()
        .expect("run whoami /user");
    let sid_text = String::from_utf8_lossy(&sid_out.stdout).to_string();
    let my_sid = sid_text
        .split_whitespace()
        .find(|token| token.starts_with("S-1-"))
        .unwrap_or_default()
        .to_string();
    assert!(
        !my_sid.is_empty(),
        "whoami /user must report a SID; without it this precondition cannot be \
         checked in SID form.\n{sid_text}"
    );
    assert!(
        !dacl_text.contains(&me) && !dacl_text.contains(&my_sid),
        "ACL fixture not established: {me} ({my_sid}) still appears in the target DACL, \
         and that SID is one of the token's restricting SIDs — the probe would measure \
         the ACE rather than the restriction.\n{dacl_text}"
    );
    assert!(
        dacl_text.contains("Authenticated Users"),
        "ACL fixture not established: Authenticated Users must retain the grant that \
         lets the first access check succeed.\n{dacl_text}"
    );

    std::fs::write(&target, b"parent-can-write\n")
        .expect("normal token must write through Authenticated Users");

    // #11213 diagnostics. The first honest run of this probe came back red:
    // the restricted child wrote the file (`file_unchanged = false`). That
    // rules out the old vacuous pass but leaves two explanations standing,
    // and the data could not separate them.
    //
    //   1. The restriction does not deny this write.
    //   2. The fixture's DACL was never what this test assumes — the probe
    //      only checked `icacls`'s exit code, never the resulting ACEs.
    //
    // The sibling test `restricted_token_contains_write_restricted_code_sid`
    // passes in the same run, so the token side is already established: the
    // kernel token carries exactly the write-restricted code, logon, and user
    // SIDs. What is not established is the object side, so capture it.
    //
    // The containing directory is captured too, and is not an afterthought:
    // the temp directory inherits from %TEMP%, which normally grants the
    // invoking user Full Control, and the user SID is one of the three
    // restricting SIDs. An access path satisfied through the directory would
    // therefore pass the second access check while the file's own ACEs look
    // exactly as intended.
    //
    // Captured here — after setup, before the spawn — so the message records
    // the state actually under test rather than one re-read after the child
    // may have altered it.
    let describe = |label: &str, out: std::io::Result<std::process::Output>| -> String {
        match out {
            Ok(o) => format!(
                "{label}:\n{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(err) => format!("{label}: <could not run: {err}>\n"),
        }
    };
    let acl_state = format!(
        "{}{}{}",
        describe(
            "icacls <target>",
            Command::new("icacls.exe").arg(&target).output()
        ),
        describe(
            "icacls <tempdir>",
            Command::new("icacls.exe").arg(temp.path()).output()
        ),
        describe(
            "whoami /user",
            Command::new("whoami.exe").args(["/user"]).output()
        ),
    );

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let system32_dir = format!("{system_root}\\System32");
    let cmd_path = format!("{system32_dir}\\cmd.exe");
    let application_name: Vec<u16> = std::ffi::OsStr::new(&cmd_path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let current_directory: Vec<u16> = std::ffi::OsStr::new(&system32_dir)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut command_line: Vec<u16> = std::ffi::OsStr::new(&format!(
        "\"{cmd_path}\" /c echo restricted-write > \"{}\"",
        target.display()
    ))
    .encode_wide()
    .chain(std::iter::once(0))
    .collect();

    let config = SandboxConfig {
        profile: SandboxProfile::Permissive,
        ..Default::default()
    };
    let job = create_job_object(&build_job_limits(&config)).expect("create_job_object");
    let token = create_restricted_token(&TokenRestrictions {
        disable_admin_sid: false,
        disable_most_sids: true,
        remove_privileges: true,
    })
    .expect("write-restricted token");

    let outcome = spawn_process_with_token(
        &application_name,
        Some(command_line.as_mut_slice()),
        Some(current_directory.as_slice()),
        &job,
        &token,
        b"",
        30_000,
        false,
    )
    .expect("launch write-restricted child");

    assert!(!outcome.timed_out, "ACL probe must not time out");

    // #11213 — this probe used to assert `exit_code != 0` first and read the
    // file only afterwards. Under CREATE_NO_WINDOW the restricted child died
    // during initialization with STATUS_DLL_INIT_FAILED, so `exit_code != 0`
    // held for a reason that has nothing to do with access checks, and the
    // decisive assertion was never reached. The probe passed for months while
    // proving nothing. #11191's DETACHED_PROCESS fix let the child actually
    // run, and the first honest measurement came back red.
    //
    // Two changes follow from that.
    //
    // 1. A positive control. "The file is unchanged" passes trivially when the
    //    child never ran, so the probe first proves the child got far enough
    //    for its access checks to be exercised at all.
    // 2. One combined verdict. The three facts are gathered before anything
    //    panics, so a single run reports all of them — an early panic is what
    //    hid the decisive one last time.
    //
    // The exit code stays in the verdict but is no longer the invariant. A
    // denied redirect must surface as a non-zero errorlevel from cmd; if that
    // ever disagrees with the file contents, the disagreement is itself the
    // finding and belongs in the failure message rather than in whichever
    // assertion happened to run first.
    const STATUS_DLL_INIT_FAILED: u32 = 0xC000_0142;

    let after = std::fs::read(&target).expect("read ACL fixture");
    let child_ran = outcome.exit_code != STATUS_DLL_INIT_FAILED;
    let file_unchanged = after == b"parent-can-write\n";
    let write_reported_denied = outcome.exit_code != 0;

    assert!(
        child_ran && file_unchanged && write_reported_denied,
        "restricted-token ACL containment probe failed (#11213)\n  \
         child_ran (exit != STATUS_DLL_INIT_FAILED) = {child_ran}\n  \
         file_unchanged                             = {file_unchanged}\n  \
         write_reported_denied (exit != 0)          = {write_reported_denied}\n  \
         exit   = {:#010x}\n  \
         after  = {:?}\n  \
         stdout = {:?}\n  \
         stderr = {:?}\n\
         ---- object side, captured before the spawn (#11213) ----\n{}",
        outcome.exit_code,
        String::from_utf8_lossy(&after),
        String::from_utf8_lossy(&outcome.stdout),
        String::from_utf8_lossy(&outcome.stderr),
        acl_state,
    );
}

/// #7979 regression — prove the policy bit is represented in the kernel
/// token, rather than only in Rust policy state or tracing. `IsTokenRestricted`
/// verifies that a restricting SID list exists and `TokenRestrictedSids`
/// verifies that the exact Windows Write Restricted Code SID (S-1-5-33) was
/// bound.
#[cfg(feature = "windows-sandbox")]
#[test]
fn restricted_token_contains_write_restricted_code_sid() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, IsTokenRestricted, TokenRestrictedSids, SID_AND_ATTRIBUTES,
        TOKEN_GROUPS, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let restrictions = TokenRestrictions {
        disable_admin_sid: false,
        disable_most_sids: true,
        remove_privileges: true,
    };
    let token = create_restricted_token(&restrictions).expect("restricted token");

    assert_ne!(
        unsafe { IsTokenRestricted(token.0) },
        0,
        "token must contain a restricting SID list"
    );

    let mut needed: u32 = 0;
    unsafe {
        GetTokenInformation(
            token.0,
            TokenRestrictedSids,
            std::ptr::null_mut(),
            0,
            &mut needed,
        );
    }
    assert!(needed > 0, "TokenRestrictedSids must report a buffer size");

    let words = (needed as usize) / 8 + 1;
    let mut buffer = vec![0u64; words];
    let capacity = (buffer.len() * 8) as u32;
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenRestrictedSids,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            capacity,
            &mut needed,
        )
    };
    assert_ne!(
        ok,
        0,
        "GetTokenInformation(TokenRestrictedSids) failed: {}",
        unsafe { GetLastError() }
    );

    // SAFETY: the u64 storage is suitably aligned and contains the complete
    // TOKEN_GROUPS value written by GetTokenInformation above.
    let groups = unsafe { &*(buffer.as_ptr() as *const TOKEN_GROUPS) };
    let entries: &[SID_AND_ATTRIBUTES] =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize) };
    assert_eq!(
        entries.len(),
        3,
        "write-restricted code, logon, and user SIDs are expected"
    );

    let mut expected = build_write_restricted_code_sid().expect("Write Restricted Code SID");
    assert!(
        entries.iter().any(|entry| unsafe {
            EqualSid(entry.Sid, expected.as_mut_ptr() as *mut core::ffi::c_void) != 0
        }),
        "kernel token must contain the Windows Write Restricted Code SID"
    );

    let mut process_token: Win32Handle = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process_token) };
    assert_ne!(ok, 0, "OpenProcessToken failed: {}", unsafe {
        GetLastError()
    });
    let process_token = OwnedHandle(process_token);
    let mut expected_logon = build_logon_sid(process_token.0).expect("current logon SID");
    assert!(
        entries.iter().any(|entry| unsafe {
            EqualSid(
                entry.Sid,
                expected_logon.as_mut_ptr() as *mut core::ffi::c_void,
            ) != 0
        }),
        "kernel token must retain only the current session logon SID for desktop access"
    );

    let mut expected_user = build_user_sid(process_token.0).expect("current user SID");
    assert!(
        entries.iter().any(|entry| unsafe {
            EqualSid(
                entry.Sid,
                expected_user.as_mut_ptr() as *mut core::ffi::c_void,
            ) != 0
        }),
        "kernel token must retain the current user for session initialization"
    );
}

/// #7071 regression — prove the restricted token actually demotes the
/// Administrators group to DENY-ONLY, rather than merely being created and
/// logged. Before the fix `SidsToDisable` was null, so the Administrators
/// group (when present) kept its normal "enabled" attributes; this assertion
/// would fail. After the fix the group carries `SE_GROUP_USE_FOR_DENY_ONLY`.
/// Runs on the `windows-latest` `--features windows-sandbox` CI leg
/// (`windows.rs` is `#[cfg(target_os = "windows")]`). When the test account is
/// not a member of Administrators the group is absent from the token and the
/// check is vacuously satisfied — the deny-only demotion only matters when the
/// SID is present.
#[cfg(feature = "windows-sandbox")]
#[test]
fn restricted_token_demotes_administrators_to_deny_only() {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::{
        CreateWellKnownSid, EqualSid, GetTokenInformation, TokenGroups,
        WinBuiltinAdministratorsSid, SID_AND_ATTRIBUTES, TOKEN_GROUPS,
    };

    const SE_GROUP_USE_FOR_DENY_ONLY: u32 = 0x0000_0010;

    // Permissive is the only profile that reaches the worker spawn path, and it
    // sets disable_admin_sid = true (pinned by the win_limits policy tests).
    let config = SandboxConfig {
        profile: SandboxProfile::Permissive,
        ..Default::default()
    };
    let restrictions = build_token_restrictions(&config);
    assert!(
        restrictions.disable_admin_sid,
        "fixture: Permissive must request admin-SID deny-only"
    );

    let token = create_restricted_token(&restrictions).expect("restricted token");

    // Reference Administrators SID to match against the token's groups.
    let mut admin = [0u8; 68];
    let mut admin_size = admin.len() as u32;
    let ok = unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            std::ptr::null_mut(),
            admin.as_mut_ptr() as *mut core::ffi::c_void,
            &mut admin_size,
        )
    };
    assert_ne!(ok, 0, "CreateWellKnownSid failed: {}", unsafe {
        GetLastError()
    });

    // First GetTokenInformation call sizes the buffer (it fails with
    // ERROR_INSUFFICIENT_BUFFER and writes the required byte count).
    let mut needed: u32 = 0;
    unsafe {
        GetTokenInformation(token.0, TokenGroups, std::ptr::null_mut(), 0, &mut needed);
    }
    // Allocate an 8-byte-aligned buffer (TOKEN_GROUPS contains a pointer) sized
    // to at least the required length, then read the groups back. Allocating in
    // u64 words guarantees the 8-byte alignment a TOKEN_GROUPS read requires;
    // `/ 8 + 1` rounds up (over-allocating at most one word, which is harmless).
    let words = (needed as usize) / 8 + 1;
    let mut buffer = vec![0u64; words];
    let capacity = (buffer.len() * 8) as u32;
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenGroups,
            buffer.as_mut_ptr() as *mut core::ffi::c_void,
            capacity,
            &mut needed,
        )
    };
    assert_ne!(
        ok,
        0,
        "GetTokenInformation(TokenGroups) failed: {}",
        unsafe { GetLastError() }
    );

    // SAFETY: `buffer` is 8-byte aligned and holds a valid TOKEN_GROUPS that
    // GetTokenInformation just wrote; `GroupCount` bounds the trailing array.
    let groups = unsafe { &*(buffer.as_ptr() as *const TOKEN_GROUPS) };
    let count = groups.GroupCount as usize;
    let entries: &[SID_AND_ATTRIBUTES] =
        unsafe { std::slice::from_raw_parts(groups.Groups.as_ptr(), count) };

    for entry in entries {
        let is_admin = unsafe { EqualSid(entry.Sid, admin.as_mut_ptr() as *mut core::ffi::c_void) };
        if is_admin != 0 {
            assert_ne!(
                entry.Attributes & SE_GROUP_USE_FOR_DENY_ONLY,
                0,
                "Administrators group must be SE_GROUP_USE_FOR_DENY_ONLY in the restricted token"
            );
        }
    }
}

#[test]
fn windows_sandbox_capabilities() {
    let sandbox = WindowsSandbox::new();
    let caps = sandbox.capabilities();
    // Feature-dependent: resource_limits, process_isolation and
    // privilege_restriction are only true when `windows-sandbox` is enabled.
    // privilege_restriction reflects the CreateProcessAsUserW restricted-token
    // launch, which only exists in the feature path.
    if cfg!(feature = "windows-sandbox") {
        assert!(caps.resource_limits);
        assert!(caps.process_isolation);
        assert!(
            caps.privilege_restriction,
            "worker is spawned under the restricted token via CreateProcessAsUserW"
        );
    } else {
        assert!(!caps.resource_limits);
        assert!(!caps.process_isolation);
        assert!(!caps.privilege_restriction);
    }
    assert!(!caps.filesystem_isolation);
    assert!(!caps.syscall_filtering);
    assert!(!caps.network_isolation);
}

#[tokio::test]
async fn standard_and_strict_fail_closed_when_containment_is_unavailable() {
    let sandbox = WindowsSandbox { is_available: true };
    let action = AutomationAction::MouseMove { x: 0, y: 0 };

    for profile in [SandboxProfile::Standard, SandboxProfile::Strict] {
        let err = sandbox
            .execute_sandboxed(
                &action,
                &SandboxConfig {
                    profile,
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();

        match err {
            CoreError::SandboxUnsupported { code, message } => {
                assert_eq!(
                    code,
                    maekon_core::error_codes::SandboxCode::UnsupportedPlatform
                );
                assert!(
                    message.contains("filesystem_isolation"),
                    "error must name missing filesystem containment: {message}"
                );
                assert!(
                    message.contains("network_isolation"),
                    "error must name missing network containment: {message}"
                );
                // privilege_restriction is now ENFORCED (feature on) via the
                // CreateProcessAsUserW restricted-token launch, so it must NOT
                // be in the missing set; without the feature it remains missing.
                if cfg!(feature = "windows-sandbox") {
                    assert!(
                        !message.contains("privilege_restriction"),
                        "restricted token is applied; privilege_restriction must NOT be missing: {message}"
                    );
                } else {
                    assert!(
                        message.contains("privilege_restriction"),
                        "error must name the unenforced restricted-token gap: {message}"
                    );
                }
                assert!(
                    message.contains("Job Object resource limits only"),
                    "error must make the non-containment mode explicit: {message}"
                );
            }
            other => panic!("expected SandboxUnsupported for {profile:?}, got {other:?}"),
        }
    }
}

/// Verify that the non-`windows-sandbox` stub for `create_restricted_token`
/// returns `Ok(())` without panicking. This path runs on every OS, ensuring
/// the stub does not claim enforcement it cannot provide.
#[test]
#[cfg(not(feature = "windows-sandbox"))]
fn restricted_token_stub_does_not_panic() {
    use crate::sandbox::win_limits::build_token_restrictions;

    let config = SandboxConfig {
        profile: SandboxProfile::Standard,
        ..Default::default()
    };
    let restrictions = build_token_restrictions(&config);
    // The stub must succeed silently; it must NOT imply enforcement.
    // Unwrap panics with the AutomationError if the stub unexpectedly fails,
    // which is more diagnostic than a value-blind is_ok() assertion.
    create_restricted_token(&restrictions)
        .expect("restricted_token stub must return Ok(()) without panicking");
}

#[test]
fn permissive_no_limits_is_noop() {
    let config = SandboxConfig {
        profile: SandboxProfile::Permissive,
        max_memory_bytes: 0,
        max_cpu_time_ms: 0,
        ..Default::default()
    };
    assert!(is_permissive_noop(&config));

    // Permissive with memory limit is NOT noop
    let config_with_mem = SandboxConfig {
        profile: SandboxProfile::Permissive,
        max_memory_bytes: 1024,
        max_cpu_time_ms: 0,
        ..Default::default()
    };
    assert!(!is_permissive_noop(&config_with_mem));

    // Standard profile is NOT noop (even with zero limits)
    let config_standard = SandboxConfig {
        profile: SandboxProfile::Standard,
        max_memory_bytes: 0,
        max_cpu_time_ms: 0,
        ..Default::default()
    };
    assert!(!is_permissive_noop(&config_standard));
}

#[tokio::test]
async fn windows_sandbox_not_available_on_other_os() {
    let sandbox = WindowsSandbox::new();
    if !cfg!(target_os = "windows") {
        assert!(!sandbox.is_available());
        let action = AutomationAction::MouseMove { x: 0, y: 0 };
        let config = SandboxConfig::default();
        let err = sandbox
            .execute_sandboxed(&action, &config)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::SandboxUnsupported { .. }),
            "non-Windows OS must produce SandboxUnsupported, got: {err:?}"
        );
    }
}
