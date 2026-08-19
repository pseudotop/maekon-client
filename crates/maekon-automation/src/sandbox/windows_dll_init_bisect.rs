//! #10288 diagnostic — which token restriction actually kills user32 init?
//!
//! Kept out of `windows.rs` deliberately. This is a bounded experiment tied to
//! one open issue, not part of the sandbox contract: when #10288 closes, the
//! whole thing is one file to delete rather than a block to find and unpick.
//! (`windows.rs` was also exactly at its ADR-013 cap, and raising a cap to hold
//! a temporary experiment spends the gate's purpose on the wrong thing.)
//!
//! A child module of `windows`, so it reaches that module's private launch
//! helpers through `super::` — same `#[path]` layout as
//! `windows_restricted_token.rs`.

// Explicit, not `use super::*`. A glob would rely on how the parent happens to
// have imported each name, and this file cannot be compiled on the development
// host (no Windows target for this crate's `windows-sandbox` feature) — so the
// form that fails loudly at the type-check gate beats the form that might
// silently resolve to nothing.
use super::{create_job_object, create_restricted_token, spawn_process_with_token};
use crate::sandbox::win_limits::{build_job_limits, TokenRestrictions};
use maekon_core::config::{SandboxConfig, SandboxProfile};

/// #10288 bisect — which token restriction actually kills user32 init?
///
/// Two independent failures share exactly one setting. The launch probe
/// above uses the Standard/Strict shape (`disable_most_sids: true`,
/// `remove_privileges: true`); the desktop-smoke lane (run 31616440563)
/// fails on `Permissive`, where both of those are `false`. The only thing
/// they have in common is `disable_admin_sid: true`.
///
/// Microsoft attributes 0xC0000142 under `CreateProcessAsUser` to window
/// station / desktop permissions rather than the loader ("when moving
/// between users or desktops … you have to ensure that the launched process
/// has the appropriate permissions", KB184802). The hosted runner user is an
/// administrator, so if WinSta0 and its Default desktop grant access chiefly
/// through Administrators, making that one SID deny-only is enough on its
/// own — no write-restriction required.
///
/// So this launches the same child twice, differing in exactly that
/// variable:
///
/// | leg | admin deny | other restrictions | reading |
/// |-----|-----------|--------------------|---------|
/// | a   | off       | full               | pass ⇒ admin deny is the cause |
/// | b   | on        | none               | fail ⇒ admin deny alone suffices |
///
/// `a` pass + `b` fail makes it necessary and sufficient. Both failing
/// points away from token shape entirely (desktop heap, image state).
///
/// **This measures; it does not gate.** It asserts only that both launches
/// started — asserting an expected exit code would bake in the answer the
/// experiment exists to obtain, and a red here would say nothing the exit
/// codes do not already say.
#[cfg(feature = "windows-sandbox")]
#[test]
fn admin_sid_deny_only_bisect_for_dll_init() {
    use std::os::windows::ffi::OsStrExt;

    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
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

    let legs = [
        (
            "a: admin deny OFF, everything else restricted",
            TokenRestrictions {
                disable_admin_sid: false,
                disable_most_sids: true,
                remove_privileges: true,
            },
        ),
        (
            "b: admin deny ON only (= Permissive, the desktop-smoke shape)",
            TokenRestrictions {
                disable_admin_sid: true,
                disable_most_sids: false,
                remove_privileges: false,
            },
        ),
        (
            // Run 32036978998 measured (a) and (b) both failing with
            // 0xC0000142, which removes "some particular restriction" as the
            // factor. This leg still goes through `CreateRestrictedToken` but
            // asks it for nothing, separating "the token object is a restricted
            // token" from "a restriction is in effect".
            //
            // Run 32038554238: PASSES on both 20348 and 26100. So the plumbing
            // — `CreateRestrictedToken`, `CreateProcessAsUserW`, the Job Object,
            // `lpDesktop = NULL` — is not the cause. Kept as the standing
            // negative control: if this ever starts failing, the mechanism moved
            // and every reading below is void.
            "c: CreateRestrictedToken with no restrictions requested",
            TokenRestrictions {
                disable_admin_sid: false,
                disable_most_sids: false,
                remove_privileges: false,
            },
        ),
        // (a) turns two restrictions on at once, so its failure does not say
        // which. These split it. (b) already measured admin-deny alone → fails.
        // With a1/a2 each restriction is tested on its own, and the answer says
        // which axis has to be relaxed for the sandbox to work at all.
        (
            "a1: write-restricted SIDs only",
            TokenRestrictions {
                disable_admin_sid: false,
                disable_most_sids: true,
                remove_privileges: false,
            },
        ),
        (
            "a2: privileges removed only",
            TokenRestrictions {
                disable_admin_sid: false,
                disable_most_sids: false,
                remove_privileges: true,
            },
        ),
    ];

    let config = SandboxConfig {
        profile: SandboxProfile::Permissive,
        ..Default::default()
    };

    for (label, restrictions) in legs {
        let job = create_job_object(&build_job_limits(&config)).expect("create_job_object");
        let token = create_restricted_token(&restrictions).expect("restricted token");
        let probe = "maekon_sandbox_bisect_4242";
        let mut command_line: Vec<u16> =
            std::ffi::OsStr::new(&format!("\"{cmd_path}\" /c echo {probe}"))
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();

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
        .expect("CreateProcessAsUserW must at least start the child");

        // `eprintln!` is captured for a passing test, so the dispatch lane
        // passes `--nocapture`. Without it a silent pass and a silent
        // measurement look identical (#10959).
        eprintln!(
            "#10288 bisect [{label}]: exit=0x{:08X} timed_out={} stdout_has_probe={}",
            outcome.exit_code,
            outcome.timed_out,
            String::from_utf8_lossy(&outcome.stdout).contains(probe),
        );
    }

    // ── which SIDs does the restricting set actually contain? ───────────────
    //
    // `windows_restricted_token.rs` already carries the history: keeping only
    // S-1-5-33 "makes CreateProcessAsUserW start the image but the child fails
    // during DLL/desktop initialization with STATUS_DLL_INIT_FAILED", and the
    // session logon SID was added to fix exactly that. So the mechanism is
    // understood — and yet a1 fails today *with* the logon SID in the set.
    //
    // The station and desktop DACLs dumped in this same job grant the logon SID
    // full access (`S-1-5-5-0-589424`, station 0xF037F / desktop 0xF01FF). If
    // the SID we put in `SidsToRestrict` is not byte-identical to that one, the
    // second access check cannot be satisfied through it — and the fix from back
    // then would have quietly stopped applying.
    //
    // Print it rather than reason about it: comparing these strings against the
    // DACL dump is a direct answer, where "the logon SID is in the list" is only
    // an answer if the two logon SIDs are the same SID.
    {
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{
            GetTokenInformation, TokenRestrictedSids, SID_AND_ATTRIBUTES, TOKEN_GROUPS,
        };

        let restrictions = TokenRestrictions {
            disable_admin_sid: false,
            disable_most_sids: true,
            remove_privileges: false,
        };
        let token = create_restricted_token(&restrictions).expect("restricted token");

        let mut needed: u32 = 0;
        // SAFETY: a null buffer with zero length is the documented sizing call.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenRestrictedSids,
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        // SAFETY: `buffer` is u64-aligned and at least `needed` bytes long.
        let ok = unsafe {
            GetTokenInformation(
                token.0,
                TokenRestrictedSids,
                buffer.as_mut_ptr() as *mut core::ffi::c_void,
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            eprintln!("#10288 sids: GetTokenInformation(TokenRestrictedSids) failed");
        } else {
            // SAFETY: the buffer holds the TOKEN_GROUPS written just above.
            let groups = unsafe { &*(buffer.as_ptr() as *const TOKEN_GROUPS) };
            let entries: &[SID_AND_ATTRIBUTES] = unsafe {
                std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize)
            };
            for entry in entries {
                let mut out: *mut u16 = std::ptr::null_mut();
                // SAFETY: `entry.Sid` came from the kernel and stays alive here.
                let converted = unsafe { ConvertSidToStringSidW(entry.Sid, &mut out) };
                if converted != 0 && !out.is_null() {
                    let mut len = 0usize;
                    // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string.
                    while unsafe { *out.add(len) } != 0 {
                        len += 1;
                    }
                    let s =
                        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
                    eprintln!("#10288 sids [SidsToRestrict]: {s}");
                    // Deliberately not calling `LocalFree`: a few bytes per SID
                    // in a diagnostic test that exits immediately, against an
                    // FFI signature that has moved between windows-sys versions
                    // and cannot be type-checked on this development host.
                }
            }
        }
    }

    // ── positive control ────────────────────────────────────────────────────
    //
    // Run 32036978998 measured every token shape failing, and that result is
    // **uninterpretable on its own**: "everything fails" reads the same whether
    // the token is the factor or whether no child process can start here at all.
    // Nothing in that run ever observed a successful child, so there was no
    // condition under which the harness was known to be able to see success.
    //
    // This leg supplies it: plain `std::process::Command`, no token, no job
    // object, no `CreateProcessAsUserW`.
    //
    // - control passes → children do start here, so the token path is the
    //   factor and `lpDesktop`/dedicated-desktop is the axis to pursue.
    // - control **fails** → the token is not the factor at all; our spawn path
    //   or the runner environment is, and #10288's whole framing moves.
    let control = std::process::Command::new(&cmd_path)
        .args(["/c", "echo", "maekon_sandbox_control_4242"])
        .current_dir(&system32_dir)
        .output();
    match control {
        Ok(out) => eprintln!(
            "#10288 control [plain Command, no restricted token]: exit={:?} \
             stdout_has_probe={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).contains("maekon_sandbox_control_4242"),
        ),
        Err(e) => eprintln!("#10288 control [plain Command]: spawn failed: {e}"),
    }
}
