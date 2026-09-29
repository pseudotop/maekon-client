# Autostart native canary

The default unit suite must not register or unregister the current user. The
unsupported-platform test is cfg-limited; the native command roundtrip is ignored
and requires explicit operator acknowledgement (#12441). The source-level gate
compiles the real cfg/ignore attributes against tripwire adapters. It exercises
macOS, Windows and Linux cfg expressions, an unsupported positive control, and
guard-removal failures without importing any native adapter. This is control-flow
evidence, not native acceptance on three operating systems.

Run the OS-free regression from the client root:

```bash
cargo test -p maekon-lint --test autostart_unit_isolation_gate
cargo test -p maekon-app --lib commands::autostart::tests::canary_
```

The gate is included by `autostart::tests` for ordinary local app unit runs and
explicitly registered in the private CI lint job. The filtered private app catalog
does not imply a whole-app unit run. The frontend CI job separately runs the pure
`commands::autostart::tests::canary_` recovery fixtures and rejects zero-test output.

## Before a native run

1. Obtain separate approval for the exact OS, disposable user/VM, application
   path and build SHA. Stop other Maekon instances and concurrent autostart work.
   A test executable is not an installed app acceptance target.
2. Record capabilities and the initial logical query. If the query fails, stop;
   unknown state is never interpreted as disabled. Record a timestamp and the
   exact native registration artifacts below, including absence, ownership,
   permissions and content/hash. Keep a VM/account snapshot and an explicit
   restoration plan before allowing mutation.
3. Confirm the registration target and executable path. Do not run on a real
   account merely because no legacy plist exists: macOS SMAppService may still
   hold registration. A single boolean cannot describe the entire native state.

| Platform | Native evidence to preserve and independently read back |
|---|---|
| macOS | Installed bundle path/signature; SMAppService status through the same bundle identity and Login Items view; `~/Library/LaunchAgents/com.maekon.app.plist` bytes/metadata or absence; `launchctl print gui/<uid>/com.maekon.app` registration and loaded state. Preserve both SMAppService and legacy fallback evidence. |
| Windows | Exact `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` value `MAEKON`, registry type/data or absence, and the executable path. Preserve that value alone, without modifying unrelated startup entries. |
| Linux | Selected systemd/XDG mode; `~/.config/systemd/user/maekon.service` and `~/.config/autostart/maekon.desktop` bytes/metadata or absence; `default.target.wants/maekon.service` symlink target or absence; user-service enabled/active state and executable path. |

## Opt-in command and result

In the approved disposable environment, from the client root:

```bash
MAEKON_AUTOSTART_CANARY=disposable-account-native-snapshot-recorded \
  cargo test -p maekon-app --lib \
  commands::autostart::tests::enable_then_disable_round_trip \
  -- --exact --ignored --nocapture --test-threads=1
```

On Windows set that environment variable only for this command's process, then
remove it. The variable is operator acknowledgement; it does not collect a backup
or prove approval. Do not use an unfiltered `--ignored` run.

The test records the initial boolean, roundtrip result, restoration operation
result, and a subsequent query. It restores the initial boolean after every
returned roundtrip error, including a partially failed enable. Initial query
failure performs no writes. Assertions occur only after restoration and readback,
so primary, restoration and readback errors are all retained in the report.
Pure fixture tests cover both initial states and failures at every step.

## Recovery and completion

Logical restoration is only the first step. Re-enabling can write a different
executable path, and disabling may remove artifacts that existed before. Restore
the exact approved native snapshot (or revert the disposable VM/account) and
compare every applicable native row above, including absence and loaded/active
state. Do not reconstruct unknown pre-state or remove unrelated entries.

If restoration/readback fails, stop further tests and retain the original report,
build SHA and before/after evidence. A panic, process kill or power loss can prevent
in-process cleanup; recover from the recorded native snapshot using the same
identity and privilege, then perform a fresh independent readback. Do not treat
test exit zero, plist absence, or a logical query alone as exact restoration.

Completion evidence must name the approved target, original snapshot, operation
results, recovery actions and independent native equality check. Until this exists,
native acceptance and exact state restoration remain **NOT_RUN** or **UNVERIFIED**.
The ordinary unit gate and its passing mocks do not change that verdict.
