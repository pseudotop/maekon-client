# Windows Rollback Mechanism — Spike Deliverable

**Spec**: internal updater hardening design record
**Scope**: D11 post-install rollback path for Windows, where a running executable cannot be replaced in place.

---

## Executive summary

The Unix rollback path (`src-tauri/src/updater/install.rs::execute_rollback`) atomically renames the backup binary into the current-executable position and spawns the replacement — straightforward because Unix files held open by inode remain valid after rename.

**Windows cannot do this.** A running `.exe` is exclusively locked by the OS; `MoveFile` / `rename` returns `ERROR_SHARING_VIOLATION` (32). The Phase 4 implementation currently stubs the Windows branch with `UpdateError::Install("...§4.8 spike pending")` so the build compiles while this spike's output lands separately.

This doc is the Phase 4 recommendation. The full Windows implementation will land under a follow-up patch because it requires a dedicated Windows runner + signed-helper decision that is out of scope for the main Phase 4 PR.

---

## Options evaluated

### Option A — `cmd.exe` helper ("ping-and-swap")

```
cmd.exe /c "timeout /t 3 /nobreak >nul && move /Y {backup} {current} && start {current}"
```

Flow:
1. Parent process spawns `cmd.exe` detached.
2. Parent process exits immediately with `ROLLBACK_EXIT_CODE`.
3. `timeout /t 3` — cmd child waits 3 seconds for parent to fully exit + release file lock.
4. `move /Y {backup} {current}` — replaces the executable (works once parent is gone).
5. `start {current}` — launches the restored binary as a new detached process.

**Pros**:
- No additional files to ship.
- Fast: ~3 seconds gap between current process exit and restored process start.
- Tested pattern — the `self_update` crate uses a similar approach.

**Cons**:
- Depends on `cmd.exe` in the PATH (always present on Windows, but non-Core editions might differ).
- `timeout` fixed at 3s; doesn't adapt to slow shutdowns (rare in practice).
- No error surface if `move /Y` fails — child exits, user sees no binary launch.
- Windows Defender may flag a detached `cmd.exe` as suspicious on first run (heuristic).

### Option B — Bundled helper `.exe` (`oneshim-rollback-helper.exe`)

Ship a tiny Rust binary (~200 KB) in the installer alongside the main app. Rollback flow:
1. Parent copies helper to a temp location (not install dir, to avoid the same lock).
2. Parent spawns helper with args `{backup} {current} {pid}`.
3. Parent exits with `ROLLBACK_EXIT_CODE`.
4. Helper waits on `pid` via Windows `OpenProcess` + `WaitForSingleObject` — deterministic, unlike `timeout /t 3`.
5. Helper does the swap + launches restored binary.

**Pros**:
- Deterministic wait (no fixed-delay race).
- Own signed helper — no AV heuristic flag.
- Robust error reporting (helper can log to a file visible to the next launch).

**Cons**:
- New binary to sign + notarize + maintain.
- Installer size +~200 KB.
- More code to review.

### Option C — `MoveFileEx(..., MOVEFILE_DELAY_UNTIL_REBOOT)`

Windows API call that schedules a rename to happen at the next OS reboot. Flow:
1. `MoveFileExW({backup}, {current}, MOVEFILE_REPLACE_EXISTING | MOVEFILE_DELAY_UNTIL_REBOOT)`.
2. Windows records entry in registry `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\PendingFileRenameOperations`.
3. Parent displays "reboot required to complete rollback" dialog and exits.

**Pros**:
- OS-managed — no race, no helper process, no AV concerns.
- Robust: survives reboots, power failures, etc.

**Cons**:
- **UX cost**: user must reboot to complete rollback. Unacceptable for the primary path.
- **Requires admin rights** to write to `HKLM` — incompatible with Tauri v2 default per-user install.

---

## Recommendation

**Option A (cmd.exe helper)** for the initial Phase 4 landing. Rationale:
- No new binary to ship — keeps installer size + signing scope unchanged.
- 3s delay is reliable in practice (self_update crate uses similar).
- Error handling limitation is acceptable for a **rollback** path — the user already saw the failed startup, so any helper failure just means they're left on the failing version (next boot retries the probe).

**Fallback to Option C** when Option A detects one of:
- UAC prompt interception (install in Program Files — Tauri v2 default is `%LOCALAPPDATA%`, so this shouldn't trigger, but guard for enterprise deployments).
- `cmd.exe` missing (e.g., Windows Core SKUs — rare but real).

**Option B** revisited in a post-Phase-4 PR if Option A reveals specific failure modes we can't paper over.

---

## Implementation sketch (for follow-up PR)

```rust
// src-tauri/src/updater/install.rs, inside execute_rollback:
#[cfg(windows)]
{
    use std::process::Command;
    // Spawn detached cmd.exe that waits 3s then swaps + relaunches.
    let cmd = format!(
        r#"timeout /t 3 /nobreak >nul && move /Y "{backup}" "{current}" && start "" "{current}""#,
        backup = backup_path.display(),
        current = current_exe_path.display(),
    );
    Command::new("cmd.exe")
        .args(&["/c", &cmd])
        .spawn()
        .map_err(|e| UpdateError::Install(format!("Windows rollback helper spawn failed: {e}")))?;
    std::process::exit(ROLLBACK_EXIT_CODE);
}
```

**Edge cases to handle in the follow-up**:
- Escape path quoting (backup/current paths may contain spaces).
- Detached spawn flags (`CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS`) to prevent the child from inheriting parent's stdio.
- Fallback to `MoveFileExW(MOVEFILE_DELAY_UNTIL_REBOOT)` when `CommandSpawn` returns `ERROR_FILE_NOT_FOUND` on `cmd.exe` lookup.

---

## CI acceptance

The release-reliability-smoke.ps1 row asserts the **precondition** for rollback, not rollback completion:
- `.install_pending_{VERSION}` + `.boot_count_pid_{VERSION}_*` per-PID markers read correctly.
- `execute_rollback` returns `Err("...§4.8 spike pending")` (current state) OR returns the Infallible-success path (when the follow-up PR lands).
- Registry `PendingFileRenameOperations` is written on the MoveFileEx fallback path.

Full end-to-end verification (binary actually swapped + restored binary running) requires a real OS reboot, which is outside the GitHub Actions runner lifetime. Manual verification is a release-checklist item.

---

## References

- Implementation record: internal updater hardening design and rollback plan
- Unix implementation: `src-tauri/src/updater/install.rs::execute_rollback` (Unix branch)
- Current Windows stub: same file, `#[cfg(windows)]` branch returns `Err("...spike pending")`
- Related: [Microsoft Learn — MoveFileExA documentation](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexa)
- Related: [self_update crate's Windows helper pattern](https://docs.rs/self_update/latest/self_update/)
